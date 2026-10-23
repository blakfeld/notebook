import * as Y from 'yjs'
import { fromBase64, toBase64 } from './base64'
import type { CoreApi, Subscription } from './core'
import type {
  CoreError,
  CoreEvents,
  ErrorCode,
  ExportResult,
  ListEntry,
  Phase,
  Selection,
  Settings,
  ThemePreference,
  SyncStatus,
  TagNode,
} from './types'

/**
 * An in-memory {@link CoreApi} for tests and the browser dev page. It is a
 * stand-in, not a second core: tags are found with a rough pattern rather
 * than the core's grammar, titles are the first non-empty line, and nothing
 * is encrypted, stored, or synced.
 *
 * Tests drive it directly: seed notes, set `phase` and `syncState`, queue
 * failures with `failNext`, `emit` events, and read `calls`.
 */
export class FakeCoreApi implements CoreApi {
  phase: Phase = {
    phase: 'signed_out',
    account: null,
    credentials_remembered: true,
  }
  syncState: SyncStatus = { state: 'synced', pending: 0, attention: [] }
  settings: Settings = {
    vim_mode: false,
    theme: 'system',
    source_view: false,
    focus_mode: false,
    collapsed_tags: [],
  }
  /** Every call so far, by method name, with its arguments. */
  readonly calls: { method: string; args: unknown[] }[] = []
  /** The account `signIn` accepts; any other credentials are refused. */
  account = { username: 'alice', passphrase: 'correct horse battery' }
  /** The recovery key `recover` accepts. */
  recoveryKey = 'AAAA-BBBB-CCCC-DDDD'
  /** While true, the server counts as unreachable. */
  offline = false
  /** While true, `exportNotes` acts as if the folder picker was dismissed. */
  exportCancelled = false
  /** Where `exportNotes` says the export went. */
  exportPath = '/Users/alice/Backups/notebook-export-20261021-140305'
  /** Delays every call by this many milliseconds (0 for none). */
  latency = 0
  /**
   * While true, new subscriptions wait (their `ready` pending, events not
   * delivered) until `releaseSubscriptions`.
   */
  holdSubscriptions = false
  private held: (() => void)[] = []

  private notes = new Map<string, FakeNote>()
  private nextId = 1
  private clock = Date.now()
  private failures = new Map<string, CoreError[]>()
  private listeners = new Map<string, Set<(payload: never) => void>>()
  private sessions = new Map<number, { note: string; doc: Y.Doc }>()
  private nextSession = 1

  constructor(notes: SeedNote[] = []) {
    for (const note of notes) this.seed(note)
  }

  /** Adds a note without emitting anything. Returns its id. */
  seed(note: SeedNote): string {
    const id = note.id ?? `n${this.nextId++}`
    const doc = new Y.Doc()
    doc.getText('body').insert(0, note.body ?? '')
    this.notes.set(id, {
      doc,
      pinned: note.pinned ?? false,
      archived: note.archived ?? false,
      trashed: note.trashed ?? false,
      updated: note.updated ?? this.tick(),
    })
    return id
  }

  body(id: string): string {
    return this.note(id).doc.getText('body').toString()
  }

  has(id: string): boolean {
    return this.notes.has(id)
  }

  /** The calls made to `method`, as argument lists. */
  callsTo(method: keyof CoreApi): unknown[][] {
    return this.calls.filter((c) => c.method === method).map((c) => c.args)
  }

  /** Makes the next call to `method` reject with `code`. */
  failNext(
    method: keyof CoreApi,
    code: ErrorCode,
    message: string = code,
  ): void {
    const queue = this.failures.get(method) ?? []
    queue.push({ code, message })
    this.failures.set(method, queue)
  }

  emit<E extends keyof CoreEvents>(event: E, payload: CoreEvents[E]): void {
    for (const handler of this.listeners.get(event) ?? []) {
      ;(handler as (payload: CoreEvents[E]) => void)(payload)
    }
  }

  /** Listeners currently subscribed to `event`. */
  listenerCount(event: keyof CoreEvents): number {
    return this.listeners.get(event)?.size ?? 0
  }

  /**
   * Simulates a change from another device: edits the note, sends attached
   * editors what they lack, and reports the change.
   */
  remoteEdit(id: string, edit: (body: Y.Text) => void): void {
    const note = this.note(id)
    const before = Y.encodeStateVector(note.doc)
    note.doc.transact(() => edit(note.doc.getText('body')))
    note.updated = this.tick()
    const update = toBase64(Y.encodeStateAsUpdate(note.doc, before))
    for (const [session, open] of this.sessions) {
      if (open.note === id) {
        Y.applyUpdate(open.doc, fromBase64(update))
        this.emit('editor-remote-update', { session, update })
      }
    }
    this.changed([id])
  }

  /**
   * Simulates a permanent delete from another device: the note is gone,
   * attached editors get nothing more, and the change is reported.
   */
  remotePurge(id: string): void {
    this.note(id)
    this.notes.delete(id)
    for (const [session, open] of this.sessions) {
      if (open.note === id) this.sessions.delete(session)
    }
    this.changed([id])
  }

  // ---- CoreApi ----

  appState = () => this.call('appState', [], () => this.phase)

  signIn = (server: string, username: string, passphrase: string) =>
    this.call('signIn', [server, username, passphrase], () => {
      this.reachable()
      if (
        username !== this.account.username ||
        passphrase !== this.account.passphrase
      ) {
        throw fail('bad_credentials')
      }
      return this.setPhase('unlocked', { server, username })
    })

  generateRecoveryKey = () =>
    this.call('generateRecoveryKey', [], () => this.recoveryKey)

  register = (args: Parameters<CoreApi['register']>[0]) =>
    this.call('register', [args], () => {
      if (args.passphrase !== args.confirm) throw fail('passphrase_mismatch')
      this.reachable()
      return this.setPhase('unlocked', {
        server: args.server,
        username: args.username,
      })
    })

  recover = (args: Parameters<CoreApi['recover']>[0]) =>
    this.call('recover', [args], () => {
      if (!/^[A-Z0-9]{4}(-[A-Z0-9]{4})+$/.test(args.recoveryKey)) {
        throw fail('recovery_key_malformed')
      }
      if (args.passphrase !== args.confirm) throw fail('passphrase_mismatch')
      this.reachable()
      if (args.recoveryKey !== this.recoveryKey) {
        throw fail('recovery_key_wrong')
      }
      this.account = { username: args.username, passphrase: args.passphrase }
      return this.setPhase('unlocked', {
        server: args.server,
        username: args.username,
      })
    })

  unlock = (passphrase: string) =>
    this.call('unlock', [passphrase], () => {
      if (passphrase !== this.account.passphrase) {
        throw fail('wrong_passphrase')
      }
      return this.setPhase('unlocked', this.phase.account)
    })

  lock = () =>
    this.call('lock', [], () => {
      // Editor sessions go with the vault, as on the host.
      this.sessions.clear()
      return this.setPhase('locked', this.phase.account)
    })

  reauthenticate = (passphrase: string) =>
    this.call('reauthenticate', [passphrase], () => {
      this.reachable()
      if (passphrase !== this.account.passphrase) {
        throw fail('wrong_passphrase')
      }
      this.setSync({
        ...this.syncState,
        attention: this.syncState.attention.filter(
          (a) => a !== 'sign_in_required',
        ),
      })
    })

  changePassphrase = (args: Parameters<CoreApi['changePassphrase']>[0]) =>
    this.call('changePassphrase', [args], () => {
      if (args.new !== args.confirm) throw fail('passphrase_mismatch')
      if (this.offline) throw fail('offline')
      if (args.current !== this.account.passphrase) {
        throw fail('wrong_passphrase')
      }
      this.account = { ...this.account, passphrase: args.new }
    })

  signOut = (confirm: boolean) =>
    this.call('signOut', [confirm], () => {
      if (this.syncState.pending > 0 && !confirm) {
        throw fail('pending_changes')
      }
      this.notes.clear()
      return this.setPhase('signed_out', null)
    })

  pendingCount = () =>
    this.call('pendingCount', [], () => this.syncState.pending)

  syncStatus = () =>
    this.call('syncStatus', [], () => {
      this.unlocked()
      return this.syncState
    })

  tagTree = () =>
    this.call('tagTree', [], () => {
      this.unlocked()
      return this.tree()
    })

  listNotes = (selection: Selection, query: string) =>
    this.call('listNotes', [selection, query], () => {
      this.unlocked()
      const terms = query.toLowerCase().split(/\s+/).filter(Boolean)
      return [...this.notes.entries()]
        .filter(([, note]) => inSelection(note, selection))
        .filter(([, note]) => {
          const body = bodyOf(note).toLowerCase()
          return terms.every((term) => body.includes(term))
        })
        .sort(([, a], [, b]) => b.updated - a.updated)
        .map(([id, note]) => entryOf(id, note))
    })

  createNote = (selection: Selection) =>
    this.call('createNote', [selection], () => {
      this.unlocked()
      const id = this.seed({
        body: 'tag' in selection ? `#${selection.tag}\n` : '',
      })
      this.changed([id])
      return id
    })

  setPinned = (id: string, pinned: boolean) =>
    this.call('setPinned', [id, pinned], () => this.flag(id, { pinned }))

  setArchived = (id: string, archived: boolean) =>
    this.call('setArchived', [id, archived], () => this.flag(id, { archived }))

  setTrashed = (id: string, trashed: boolean) =>
    this.call('setTrashed', [id, trashed], () => this.flag(id, { trashed }))

  deletePermanently = (id: string) =>
    this.call('deletePermanently', [id], () => {
      if (!this.note(id).trashed) throw fail('not_trashed')
      if (this.offline) throw fail('offline')
      this.notes.delete(id)
      this.changed([id])
    })

  renameTag = (old: string, renamed: string) =>
    this.call('renameTag', [old, renamed], () => {
      this.unlocked()
      const target = cleanPath(renamed)
      if (!target) throw fail('invalid_tag_name')
      const pattern = new RegExp(
        `(^|\\s)#${escape(cleanPath(old) ?? '')}(?=[/\\s]|$)`,
        'giu',
      )
      const changed: string[] = []
      for (const [id, note] of this.notes) {
        const text = note.doc.getText('body')
        const body = text.toString()
        const next = body.replace(pattern, `$1#${target}`)
        if (next !== body) {
          note.doc.transact(() => {
            text.delete(0, text.length)
            text.insert(0, next)
          })
          note.updated = this.tick()
          changed.push(id)
        }
      }
      this.changed(changed)
      return changed.length
    })

  exportNotes = (args: { includeTrashed: boolean }) =>
    this.call('exportNotes', [args], (): ExportResult | null => {
      this.unlocked()
      if (this.exportCancelled) return null
      const count = [...this.notes.values()].filter(
        (note) => args.includeTrashed || !note.trashed,
      ).length
      return { count, path: this.exportPath }
    })

  editorOpen = (noteId: string) =>
    this.call('editorOpen', [noteId], () => {
      this.unlocked()
      const note = this.note(noteId)
      const doc = new Y.Doc()
      Y.applyUpdate(doc, Y.encodeStateAsUpdate(note.doc))
      const session = this.nextSession++
      this.sessions.set(session, { note: noteId, doc })
      return { session, state: toBase64(Y.encodeStateAsUpdate(doc)) }
    })

  editorPush = (session: number, update: string) =>
    this.call('editorPush', [session, update], () => {
      this.unlocked()
      const open = this.sessions.get(session)
      if (!open) throw fail('not_found')
      const note = this.note(open.note)
      if (note.trashed) throw fail('trashed')
      Y.applyUpdate(open.doc, fromBase64(update))
      Y.applyUpdate(note.doc, fromBase64(update))
      note.updated = this.tick()
      this.changed([open.note])
    })

  editorClose = (session: number) =>
    this.call('editorClose', [session], () => {
      this.unlocked()
      this.sessions.delete(session)
    })

  /** Editors attached to the note. */
  attached(noteId: string): number {
    return [...this.sessions.values()].filter((s) => s.note === noteId).length
  }

  getSettings = () =>
    this.call('getSettings', [], () => ({
      ...this.settings,
      collapsed_tags: [...this.settings.collapsed_tags],
    }))

  setVimMode = (on: boolean) =>
    this.call('setVimMode', [on], () => {
      this.settings.vim_mode = on
    })

  setPreference = (key: string, value: string) =>
    this.call('setPreference', [key, value], () => {
      const on = value === 'true'
      const flag = on || value === 'false'
      if (key === 'appearance.theme' && isTheme(value)) {
        this.settings.theme = value
      } else if (key === 'editor.sourceView' && flag) {
        this.settings.source_view = on
      } else if (key === 'editor.focusMode' && flag) {
        this.settings.focus_mode = on
      } else {
        throw fail('internal')
      }
    })

  setTagExpanded = (path: string, expanded: boolean) =>
    this.call('setTagExpanded', [path, expanded], () => {
      const key = path.toLowerCase()
      const rest = this.settings.collapsed_tags.filter((p) => p !== key)
      this.settings.collapsed_tags = expanded ? rest : [...rest, key].sort()
    })

  on<E extends keyof CoreEvents>(
    event: E,
    handler: (payload: CoreEvents[E]) => void,
  ): Subscription {
    const set = this.listeners.get(event) ?? new Set()
    this.listeners.set(event, set)
    const listener = handler as (payload: never) => void
    let cancelled = false
    const attach = () => {
      if (!cancelled) set.add(listener)
    }
    // Like the host, a held subscription misses events until it is live.
    const ready = this.holdSubscriptions
      ? new Promise<void>((resolve) =>
          this.held.push(() => {
            attach()
            resolve()
          }),
        )
      : (attach(), Promise.resolve())
    const unsubscribe = () => {
      cancelled = true
      set.delete(listener)
    }
    return Object.assign(unsubscribe, { ready })
  }

  /** Makes held subscriptions live, in order. */
  releaseSubscriptions(): void {
    for (const release of this.held.splice(0)) release()
  }

  // ---- helpers ----

  /** Sets the phase and emits `app-phase-changed`. */
  setPhase(phase: Phase['phase'], account: Phase['account']): Phase {
    this.phase = { ...this.phase, phase, account }
    this.emit('app-phase-changed', this.phase)
    return this.phase
  }

  /** Sets the sync status and emits `sync-status`. */
  setSync(status: SyncStatus): void {
    this.syncState = status
    this.emit('sync-status', status)
  }

  private async call<T>(
    method: string,
    args: unknown[],
    run: () => T,
  ): Promise<T> {
    this.calls.push({ method, args })
    if (this.latency > 0) {
      await new Promise((resolve) => setTimeout(resolve, this.latency))
    }
    const failure = this.failures.get(method)?.shift()
    if (failure) throw failure
    return run()
  }

  private note(id: string): FakeNote {
    const note = this.notes.get(id)
    if (!note) throw fail('not_found')
    return note
  }

  private flag(id: string, flags: Partial<FakeNote>): void {
    this.unlocked()
    Object.assign(this.note(id), flags)
    this.changed([id])
  }

  private changed(ids: string[]): void {
    this.emit('vault-changed', { changed: ids, tree_changed: true })
  }

  private unlocked(): void {
    if (this.phase.phase !== 'unlocked') throw fail('locked')
  }

  private reachable(): void {
    if (this.offline) throw fail('unreachable')
  }

  private tick(): number {
    return (this.clock += 1000)
  }

  private tree(): TagNode[] {
    const roots: TagNode[] = []
    for (const note of this.notes.values()) {
      if (note.archived || note.trashed) continue
      const counted = new Set<TagNode>()
      for (const tag of tagsOf(bodyOf(note))) {
        let level = roots
        let path = ''
        for (const name of tag.split('/')) {
          path = path ? `${path}/${name}` : name
          let node = level.find(
            (n) => n.name.toLowerCase() === name.toLowerCase(),
          )
          if (!node) {
            node = { name, path, count: 0, children: [] }
            level.push(node)
          }
          if (!counted.has(node)) {
            counted.add(node)
            node.count++
          }
          level = node.children
        }
      }
    }
    const sort = (nodes: TagNode[]) => {
      nodes.sort((a, b) =>
        a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
      )
      nodes.forEach((n) => sort(n.children))
    }
    sort(roots)
    return roots
  }
}

export interface SeedNote {
  id?: string
  body?: string
  pinned?: boolean
  archived?: boolean
  trashed?: boolean
  updated?: number
}

interface FakeNote {
  doc: Y.Doc
  pinned: boolean
  archived: boolean
  trashed: boolean
  updated: number
}

function fail(code: ErrorCode): CoreError {
  return { code, message: code }
}

const isTheme = (value: string): value is ThemePreference =>
  value === 'system' || value === 'light' || value === 'dark'

function bodyOf(note: FakeNote): string {
  return note.doc.getText('body').toString()
}

/** `#word` and `#a/b` tags, roughly: no multi-word tags, no code spans. */
function tagsOf(body: string): string[] {
  const tags = new Map<string, string>()
  for (const match of body.matchAll(/(?:^|\s)#([\p{L}\p{N}_\-/]+)/gu)) {
    const path = cleanPath(match[1])
    if (path && !tags.has(path.toLowerCase()))
      tags.set(path.toLowerCase(), path)
  }
  return [...tags.values()]
}

/** The path with empty segments dropped, or null if it isn't a tag. */
function cleanPath(path: string): string | null {
  const clean = path.split('/').filter(Boolean).join('/')
  if (!clean || /^\d+$/.test(clean) || /[#\s]/.test(clean)) return null
  return clean
}

function escape(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function inSelection(note: FakeNote, selection: Selection): boolean {
  if ('tag' in selection) {
    if (note.archived || note.trashed) return false
    const tag = selection.tag.toLowerCase()
    return tagsOf(bodyOf(note)).some((t) => {
      const lower = t.toLowerCase()
      return lower === tag || lower.startsWith(`${tag}/`)
    })
  }
  const active = !note.archived && !note.trashed
  switch (selection.view) {
    case 'notes':
      return active
    case 'untagged':
      return active && tagsOf(bodyOf(note)).length === 0
    case 'pinned':
      return active && note.pinned
    case 'archive':
      return note.archived && !note.trashed
    case 'trash':
      return note.trashed
  }
}

function entryOf(id: string, note: FakeNote): ListEntry {
  const lines = bodyOf(note).split('\n')
  const index = lines.findIndex((line) => line.trim() !== '')
  const title = index < 0 ? '' : lines[index].trim().replace(/^#{1,6}\s+/, '')
  const preview =
    index < 0
      ? ''
      : lines
          .slice(index + 1)
          .join(' ')
          .trim()
  return {
    id,
    title,
    preview,
    updated: note.updated,
    pinned: note.pinned,
    archived: note.archived,
    trashed: note.trashed,
  }
}
