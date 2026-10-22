import { defaultKeymap, indentWithTab } from '@codemirror/commands'
import {
  deleteMarkupBackward,
  insertNewlineContinueMarkupCommand,
  markdown,
  markdownLanguage,
} from '@codemirror/lang-markdown'
import { defaultHighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { search, searchKeymap } from '@codemirror/search'
import { Compartment, EditorState, type Extension } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { Vim, vim } from '@replit/codemirror-vim'
import { yCollab, ySyncAnnotation } from 'y-codemirror.next'
import * as Y from 'yjs'
import type { DeviceSettings } from '../platform/settings'
import type { NoteDocBridge, OpenNoteDoc } from './bridge'
import { tagHighlighter, tagTheme } from './tags'

/** The device setting that turns vim mode on (`"true"`) or off. */
export const VIM_MODE_KEY = 'editor.vimMode'

/** Local edits are delivered after this much idle time... */
const IDLE_DELAY_MS = 300
/** ...and at most this long after the first undelivered edit. */
const MAX_DELAY_MS = 1000
/** A failed delivery is retried after this long if nothing else triggers it. */
const RETRY_DELAY_MS = 1000

/**
 * The origin of every update that came from the core. Those are never sent
 * back, and the undo manager doesn't track them.
 */
export const REMOTE = Symbol('remote')

export interface NoteEditorOptions {
  bridge: NoteDocBridge
  settings: DeviceSettings
  /** Called with failures the editor recovers from (delivery, settings). */
  onError?: (error: unknown) => void
}

export interface NoteEditorHandle {
  readonly view: EditorView
  /** The open note, or `null` if none. */
  readonly noteId: string | null
  readonly vimMode: boolean
  /** Resolves once the stored vim mode preference has been read. */
  readonly ready: Promise<void>
  /** Undoes the last local edit. Returns whether there was one. */
  undo(): boolean
  /** Redoes the last undone edit. Returns whether there was one. */
  redo(): boolean
  /**
   * Delivers pending local edits now. Resolves `true` once everything typed
   * so far has been delivered, or `false` if delivery failed (the edits are
   * kept, the failure goes to `onError`, and delivery is retried).
   */
  flush(): Promise<boolean>
  focus(): void
  /** Turns vim mode on or off, and saves the choice on this device. */
  setVimMode(on: boolean): void
  setReadOnly(on: boolean): void
  /**
   * Delivers the open note's pending edits, closes it, and opens `noteId`
   * (or nothing, for `null`).
   */
  openNote(noteId: string | null): Promise<void>
  /** Delivers pending edits, closes the note, and removes the editor. */
  destroy(): Promise<void>
}

/** One open note: its Yjs replica, its undo history, and its delivery. */
class Session {
  readonly ydoc = new Y.Doc()
  readonly ytext = this.ydoc.getText('body')
  readonly undoManager: Y.UndoManager
  private pending: Uint8Array[] = []
  private inFlight: Promise<boolean> | null = null
  private idleTimer: ReturnType<typeof setTimeout> | undefined
  private maxTimer: ReturnType<typeof setTimeout> | undefined
  private readonly unsubscribe: () => void
  /** Set by `retire`, whose loop then does the retrying. */
  private retiring = false
  private closed = false

  constructor(
    readonly noteId: string,
    private readonly doc: OpenNoteDoc,
    private readonly onError: (error: unknown) => void,
  ) {
    Y.applyUpdate(this.ydoc, doc.initialState, REMOTE)
    // The binding adds its own origin; nothing else is tracked.
    this.undoManager = new Y.UndoManager(this.ytext, {
      trackedOrigins: new Set(),
    })
    this.ydoc.on('update', (update: Uint8Array, origin: unknown) => {
      if (origin !== REMOTE) this.add(update)
    })
    this.unsubscribe = doc.onRemote((update) => {
      try {
        Y.applyUpdate(this.ydoc, update, REMOTE)
      } catch (error) {
        this.onError(error)
      }
    })
  }

  private add(update: Uint8Array) {
    if (this.closed) return
    this.pending.push(update)
    clearTimeout(this.idleTimer)
    this.idleTimer = setTimeout(() => void this.flush(), IDLE_DELAY_MS)
    this.maxTimer ??= setTimeout(() => void this.flush(), MAX_DELAY_MS)
  }

  async flush(): Promise<boolean> {
    while (this.inFlight) await this.inFlight
    clearTimeout(this.idleTimer)
    clearTimeout(this.maxTimer)
    this.idleTimer = this.maxTimer = undefined
    if (this.pending.length === 0) return true
    const update =
      this.pending.length === 1 ? this.pending[0] : Y.mergeUpdates(this.pending)
    this.pending = []
    const inFlight = this.doc.push(update).then(
      () => true,
      (error: unknown) => {
        // Kept at the head, so it merges with anything typed since.
        this.pending.unshift(update)
        this.onError(error)
        if (!this.retiring) {
          this.maxTimer ??= setTimeout(() => void this.flush(), RETRY_DELAY_MS)
        }
        return false
      },
    )
    this.inFlight = inFlight
    try {
      return await inFlight
    } finally {
      if (this.inFlight === inFlight) this.inFlight = null
    }
  }

  /** Flushes until nothing is pending, so edits made meanwhile go too. */
  private async flushAll(): Promise<boolean> {
    do {
      if (!(await this.flush())) return false
    } while (this.pending.length > 0)
    return true
  }

  /**
   * Delivers everything and detaches. Resolves after the first attempt; if
   * that fails, delivery keeps being retried in the background, and the
   * note is detached once it succeeds.
   */
  async retire(): Promise<void> {
    this.retiring = true
    if (await this.flushAll()) {
      await this.close()
      return
    }
    void (async () => {
      do {
        await new Promise((resolve) => setTimeout(resolve, RETRY_DELAY_MS))
      } while (!(await this.flushAll()))
      await this.close()
    })()
  }

  private async close() {
    this.closed = true
    clearTimeout(this.idleTimer)
    clearTimeout(this.maxTimer)
    this.unsubscribe()
    this.undoManager.destroy()
    try {
      await this.doc.close()
    } catch (error) {
      this.onError(error)
    }
  }
}

/** The editor behind each view, for the vim commands, which are global. */
const editors = new WeakMap<EditorView, Editor>()

interface Editor {
  undo(): boolean
  redo(): boolean
  flush(): Promise<boolean>
}

let vimCommandsDefined = false

/** Routes vim's undo, redo, and write commands to the note's editor. */
function defineVimCommands() {
  if (vimCommandsDefined) return
  vimCommandsDefined = true
  Vim.defineAction('noteUndo', (cm) => void editors.get(cm.cm6)?.undo())
  Vim.defineAction('noteRedo', (cm) => void editors.get(cm.cm6)?.redo())
  Vim.mapCommand('u', 'action', 'noteUndo', {}, { context: 'normal' })
  Vim.mapCommand('<C-r>', 'action', 'noteRedo', {}, { context: 'normal' })
  Vim.defineEx('write', 'w', (cm) => {
    void editors
      .get(cm.cm6)
      ?.flush()
      .then((delivered) => {
        if (!delivered) return
        const message = document.createElement('span')
        message.textContent = 'Saved'
        cm.openNotification(message, { bottom: true, duration: 2000 })
      })
  })
  // Saving is automatic and closing a note is the app's job, so these only
  // deliver pending edits.
  for (const [name, prefix] of [
    ['wq', 'wq'],
    ['xit', 'x'],
    ['quit', 'q'],
  ]) {
    Vim.defineEx(name, prefix, (cm) => void editors.get(cm.cm6)?.flush())
  }
}

const readOnlyExtension = (on: boolean): Extension =>
  on
    ? [
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
        // Commands already respect `readOnly`; this also stops anything
        // dispatched directly. Remote changes carry the sync annotation.
        EditorState.transactionFilter.of((tr) =>
          tr.docChanged && !tr.annotation(ySyncAnnotation) ? [] : tr,
        ),
      ]
    : []

// Fills the host element the editor is mounted in.
const fillHost = EditorView.theme({ '&': { height: '100%' } })

/**
 * Creates a markdown note editor in `parent`, with no note open.
 *
 * The editor lives in a shadow root on a `.note-editor-host` element. There
 * CodeMirror styles itself with constructable stylesheets, which a strict
 * `style-src` CSP allows; in the document it would add `<style>` elements,
 * which that CSP blocks. Page CSS can size the host but not reach inside, so
 * styling goes through editor themes.
 */
export function createNoteEditor(
  parent: HTMLElement,
  { bridge, settings, onError = () => {} }: NoteEditorOptions,
): NoteEditorHandle {
  defineVimCommands()
  const vimCompartment = new Compartment()
  const readOnlyCompartment = new Compartment()
  let vimOn = false
  let vimChosen = false
  let readOnly = false
  let session: Session | null = null
  let opening = 0

  const undo = () => {
    if (!session || view.state.readOnly) return false
    return session.undoManager.undo() !== null
  }
  const redo = () => {
    if (!session || view.state.readOnly) return false
    return session.undoManager.redo() !== null
  }
  const flush = () => session?.flush() ?? Promise.resolve(true)

  const stateFor = (current: Session | null): EditorState =>
    EditorState.create({
      doc: current?.ytext.toString() ?? '',
      extensions: [
        // First, so vim's keys win when it is on.
        vimCompartment.of(vimOn ? vim() : []),
        // Only `\n` breaks lines, so CodeMirror offsets match the Yjs text
        // exactly even when the body contains `\r`.
        EditorState.lineSeparator.of('\n'),
        markdown({
          base: markdownLanguage,
          codeLanguages: [],
          addKeymap: false,
        }),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        current
          ? yCollab(current.ytext, null, { undoManager: current.undoManager })
          : [],
        keymap.of([
          { key: 'Mod-z', run: undo, preventDefault: true },
          { key: 'Mod-y', mac: 'Mod-Shift-z', run: redo, preventDefault: true },
          { key: 'Mod-Shift-z', run: redo, preventDefault: true },
        ]),
        search({ top: true }),
        keymap.of(searchKeymap),
        EditorView.lineWrapping,
        // Like `markdownKeymap`, except that Enter on an empty item always
        // ends the list instead of first making it loose.
        keymap.of([
          {
            key: 'Enter',
            run: insertNewlineContinueMarkupCommand({ nonTightLists: false }),
          },
          { key: 'Backspace', run: deleteMarkupBackward },
        ]),
        tagHighlighter,
        tagTheme,
        fillHost,
        keymap.of([...defaultKeymap, indentWithTab]),
        readOnlyCompartment.of(readOnlyExtension(readOnly || !current)),
      ],
    })

  const host = document.createElement('div')
  host.className = 'note-editor-host'
  parent.append(host)
  const root = host.attachShadow({ mode: 'open' })
  const view = new EditorView({ parent: root, root, state: stateFor(null) })

  const applyVim = (on: boolean) => {
    vimOn = on
    view.dispatch({ effects: vimCompartment.reconfigure(on ? vim() : []) })
  }

  const ready = settings.get(VIM_MODE_KEY).then(
    (value) => {
      if (!vimChosen && value === 'true') applyVim(true)
    },
    // Unreadable means the default.
    () => {},
  )

  const flushNow = () => void flush()
  const onVisibility = () => {
    if (document.visibilityState === 'hidden') flushNow()
  }
  document.addEventListener('visibilitychange', onVisibility)
  window.addEventListener('pagehide', flushNow)

  const editor: Editor = { undo, redo, flush }
  editors.set(view, editor)

  return {
    view,
    get noteId() {
      return session?.noteId ?? null
    },
    get vimMode() {
      return vimOn
    },
    ready,
    undo,
    redo,
    flush,
    focus: () => view.focus(),

    setVimMode(on) {
      vimChosen = true
      if (on !== vimOn) applyVim(on)
      settings.set(VIM_MODE_KEY, String(on)).catch(onError)
    },

    setReadOnly(on) {
      readOnly = on
      view.dispatch({
        effects: readOnlyCompartment.reconfigure(
          readOnlyExtension(on || !session),
        ),
      })
    },

    async openNote(noteId) {
      const attempt = ++opening
      const previous = session
      session = null
      // Nothing is editable until the next note is open: typing now would
      // go to the old note after it is delivered and closed.
      view.setState(stateFor(null))
      await previous?.retire()
      if (noteId === null) return
      const doc = await bridge.open(noteId)
      if (attempt !== opening) {
        // Another note was opened meanwhile.
        await doc.close()
        return
      }
      session = new Session(noteId, doc, onError)
      view.setState(stateFor(session))
    },

    async destroy() {
      opening++
      document.removeEventListener('visibilitychange', onVisibility)
      window.removeEventListener('pagehide', flushNow)
      const previous = session
      session = null
      editors.delete(view)
      view.destroy()
      host.remove()
      await previous?.retire()
    },
  }
}
