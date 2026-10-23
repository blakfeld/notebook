import * as Y from 'yjs'
import { fromBase64, toBase64 } from './base64'
import type { CoreApi, Subscription } from './core'
import type {
  BackupInterval,
  BackupOptions,
  BackupStatus,
  CoreError,
  CoreEvents,
  ErrorCode,
  ExportResult,
  ListEntry,
  Phase,
  PhaseName,
  PickedBackup,
  Selection,
  Settings,
  ThemePreference,
  TagNode,
} from './types'

/**
 * An in-memory {@link CoreApi} for tests and the browser dev page. It is a
 * stand-in, not a second core: tags are found with a rough pattern rather
 * than the core's grammar, titles are the first non-empty line, recovery
 * keys have no real checksum, and nothing is encrypted or stored.
 *
 * It follows the host's phases: `setup` → `createNotebook` →
 * `pending_recovery_key` → `acknowledgeRecoveryKey` → `open`, with
 * `needs_recovery_key` (opened with `recoveryKey`) and the upgrade's
 * `needs_old_passphrase` (opened with `oldPassphrase`, or `startOver`).
 * Tests set `phase` to start anywhere.
 *
 * Backups are kept as note snapshots in `backupsWritten`; the backup file
 * the picker returns is `backupFile`, opened with `recoveryKey` unless it is
 * `same_vault`.
 *
 * Tests drive it directly: seed notes, set `phase`, queue failures with
 * `failNext`, `emit` events, and read `calls`.
 */
export class FakeCoreApi implements CoreApi {
  phase: Phase = { phase: 'setup', secure_store: true, upgrade: false }
  settings: Settings = {
    vim_mode: false,
    theme: 'system',
    source_view: false,
    focus_mode: false,
    collapsed_tags: [],
  }
  /** Every call so far, by method name, with its arguments. */
  readonly calls: { method: string; args: unknown[] }[] = []
  /**
   * The notebook's recovery key: what `createNotebook` and
   * `pendingRecoveryKey` return and `openWithRecoveryKey` accepts.
   */
  recoveryKey = 'ABCD-EFGH-JKMN-PQRS-TVWX-YZ01-2345-6789'
  /** The passphrase `upgradeWithPassphrase` accepts. */
  oldPassphrase = 'correct horse battery'
  /** What `coreVersion` returns. */
  version = '0.1.0'
  /** How many times `startOver` set the old notes aside. */
  setAside = 0
  /** While true, `exportNotes` acts as if the folder picker was dismissed. */
  exportCancelled = false
  /** Where `exportNotes` says the export went. */
  exportPath = '/Users/alice/Backups/notebook-export-20261021-140305'
  /** The backup settings and status, as `backupStatus` returns them. */
  backup: BackupStatus = {
    folder: null,
    auto: true,
    interval: 'daily',
    keep: 14,
    last: null,
    next_due: null,
    changed: false,
    last_error: null,
    failing: false,
    overdue: false,
    running: false,
  }
  /** What the folder picker returns; `null` acts as if it was dismissed. */
  backupFolderChoice: string | null = '/Users/alice/Dropbox/Notebook'
  /** What the backup file picker returns; `null` acts as if dismissed. */
  backupFile: FakeBackupFile | null = null
  /** The notes of every backup written, oldest first. */
  readonly backupsWritten: SeedNote[][] = []
  /** While set, writing a backup fails with this reason. */
  backupWriteError: string | null = null
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
  private pickedBackup: FakeBackupFile | null = null

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
   * Simulates a change the editor didn't make (another window, or the host
   * itself): edits the note, sends attached editors what they lack, and
   * reports the change.
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
   * Simulates a permanent delete the editor didn't make: the note is gone,
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

  coreVersion = () => this.call('coreVersion', [], () => this.version)

  createNotebook = () =>
    this.call('createNotebook', [], () => {
      this.requirePhase('setup')
      this.setPhase('pending_recovery_key')
      return this.recoveryKey
    })

  pendingRecoveryKey = () =>
    this.call('pendingRecoveryKey', [], () => {
      this.requirePhase('pending_recovery_key')
      return this.recoveryKey
    })

  acknowledgeRecoveryKey = () =>
    this.call('acknowledgeRecoveryKey', [], () => {
      this.requirePhase('pending_recovery_key')
      return this.setPhase('open')
    })

  openWithRecoveryKey = (recoveryKey: string) =>
    this.call('openWithRecoveryKey', [recoveryKey], () => {
      // As on the host, the key's form is checked before the phase.
      const typed = normalizeKey(recoveryKey)
      if (typed === null) throw fail('recovery_key_malformed')
      this.requirePhase('needs_recovery_key')
      if (typed !== normalizeKey(this.recoveryKey)) {
        throw fail('recovery_key_wrong')
      }
      return this.setPhase('open')
    })

  upgradeWithPassphrase = (passphrase: string) =>
    this.call('upgradeWithPassphrase', [passphrase], () => {
      this.requirePhase('needs_old_passphrase')
      if (passphrase !== this.oldPassphrase) throw fail('wrong_passphrase')
      return this.setPhase('pending_recovery_key', true)
    })

  startOver = () =>
    this.call('startOver', [], () => {
      // As on the host: also while an upgrade's key is pending, since its
      // database may only turn out unopenable at acknowledgment.
      const { phase, upgrade } = this.phase
      const allowed =
        phase === 'needs_old_passphrase' ||
        phase === 'needs_recovery_key' ||
        (phase === 'pending_recovery_key' && upgrade)
      if (!allowed) throw fail('not_open')
      // The host renames the old database; the new notebook starts empty.
      this.notes.clear()
      this.setAside++
      return this.setPhase('setup')
    })

  tagTree = () =>
    this.call('tagTree', [], () => {
      this.requireOpen()
      return this.tree()
    })

  listNotes = (selection: Selection, query: string) =>
    this.call('listNotes', [selection, query], () => {
      this.requireOpen()
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
      this.requireOpen()
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
      this.requireOpen()
      if (!this.note(id).trashed) throw fail('not_trashed')
      this.notes.delete(id)
      this.changed([id])
    })

  renameTag = (old: string, renamed: string) =>
    this.call('renameTag', [old, renamed], () => {
      this.requireOpen()
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
      this.requireOpen()
      if (this.exportCancelled) return null
      const count = [...this.notes.values()].filter(
        (note) => args.includeTrashed || !note.trashed,
      ).length
      return { count, path: this.exportPath }
    })

  backupStatus = () =>
    this.call('backupStatus', [], () => {
      this.requireOpen()
      return this.backupSnapshot()
    })

  chooseBackupFolder = () =>
    this.call('chooseBackupFolder', [], (): BackupStatus | null => {
      this.requireOpen()
      if (this.backupFolderChoice === null) return null
      const first = this.backup.folder === null
      this.backup.folder = this.backupFolderChoice
      // As on the host, the first folder gets a backup at once, and keeps
      // being the folder if that fails.
      if (first) this.tryBackup()
      return this.backupChanged()
    })

  setBackupOptions = (options: BackupOptions) =>
    this.call('setBackupOptions', [options], () => {
      this.requireOpen()
      if (!Number.isInteger(options.keep) || options.keep < 1) {
        throw fail('internal')
      }
      Object.assign(this.backup, options)
      return this.backupChanged()
    })

  backupNow = () =>
    this.call('backupNow', [], () => {
      this.requireOpen()
      if (this.backup.folder === null) throw fail('backup_no_folder')
      const error = this.tryBackup()
      this.backupChanged()
      if (error) throw { code: 'backup_failed', message: error }
      return this.backupSnapshot()
    })

  pickBackup = () =>
    this.call('pickBackup', [], (): PickedBackup | null => {
      const phase = this.phase.phase
      if (phase !== 'open' && phase !== 'setup') throw fail('not_open')
      if (this.backupFile === null) return null
      this.pickedBackup = this.backupFile
      const { file_name, same_vault } = this.backupFile
      return { file_name, same_vault }
    })

  cancelRestore = () =>
    this.call('cancelRestore', [], () => {
      this.pickedBackup = null
    })

  restoreBackup = (args: {
    recoveryKey: string | null
    skipSafetyBackup: boolean
  }) =>
    this.call('restoreBackup', [args], () => {
      const picked = this.pickedBackup
      const phase = this.phase.phase
      if (!picked || (phase !== 'open' && phase !== 'setup')) {
        throw fail('not_open')
      }
      if (!picked.same_vault) {
        if (args.recoveryKey === null) throw fail('recovery_key_required')
        // The key's form is checked before the backup is opened.
        const typed = normalizeKey(args.recoveryKey)
        if (typed === null) throw fail('recovery_key_malformed')
        if (typed !== normalizeKey(this.recoveryKey)) {
          throw fail('recovery_key_wrong')
        }
      }
      if (
        phase === 'open' &&
        this.backup.folder !== null &&
        !args.skipSafetyBackup
      ) {
        const error = this.tryBackup()
        if (error) {
          this.backupChanged()
          throw { code: 'safety_backup_failed', message: error }
        }
      }
      this.notes.clear()
      this.sessions.clear()
      for (const note of picked.notes) this.seed(note)
      this.pickedBackup = null
      return this.setPhase('open')
    })

  editorOpen = (noteId: string) =>
    this.call('editorOpen', [noteId], () => {
      this.requireOpen()
      const note = this.note(noteId)
      const doc = new Y.Doc()
      Y.applyUpdate(doc, Y.encodeStateAsUpdate(note.doc))
      const session = this.nextSession++
      this.sessions.set(session, { note: noteId, doc })
      return { session, state: toBase64(Y.encodeStateAsUpdate(doc)) }
    })

  editorPush = (session: number, update: string) =>
    this.call('editorPush', [session, update], () => {
      this.requireOpen()
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
      this.requireOpen()
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

  /**
   * Sets the phase and emits `app-phase-changed`. `upgrade` is always true
   * in `needs_old_passphrase`, as on the host.
   */
  setPhase(phase: PhaseName, upgrade = false): Phase {
    this.phase = {
      ...this.phase,
      phase,
      upgrade: upgrade || phase === 'needs_old_passphrase',
    }
    this.emit('app-phase-changed', this.phase)
    return this.phase
  }

  /**
   * Writes a backup of the notes into `backupsWritten`, or records why it
   * couldn't. Returns the failure, if any.
   */
  private tryBackup(): string | null {
    if (this.backupWriteError !== null) {
      this.backup.last_error = this.backupWriteError
      return this.backupWriteError
    }
    this.backupsWritten.push(
      [...this.notes].map(([id, note]) => ({
        id,
        body: bodyOf(note),
        pinned: note.pinned,
        archived: note.archived,
        trashed: note.trashed,
        updated: note.updated,
      })),
    )
    this.backup.last = Date.now()
    this.backup.changed = false
    this.backup.last_error = null
    return null
  }

  /** The status with its derived fields filled in. */
  private backupSnapshot(): BackupStatus {
    const status = this.backup
    const set = status.folder !== null
    const scheduled = set && status.auto && status.last !== null
    const interval = INTERVAL_MS[status.interval]
    status.next_due = scheduled ? status.last! + interval : null
    status.failing = set && status.last_error !== null
    status.overdue =
      set &&
      status.auto &&
      status.changed &&
      status.last !== null &&
      Date.now() - status.last > 2 * interval
    return { ...status }
  }

  /** Emits `backup-status-changed` and returns the status. */
  private backupChanged(): BackupStatus {
    const status = this.backupSnapshot()
    this.emit('backup-status-changed', { ...status })
    return status
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
    this.requireOpen()
    Object.assign(this.note(id), flags)
    this.changed([id])
  }

  private changed(ids: string[]): void {
    if (ids.length > 0) this.backup.changed = true
    this.emit('vault-changed', { changed: ids, tree_changed: true })
  }

  private requireOpen(): void {
    this.requirePhase('open')
  }

  /** Commands that don't fit the phase fail with `not_open`, as on the host. */
  private requirePhase(phase: PhaseName): void {
    if (this.phase.phase !== phase) throw fail('not_open')
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

/** A backup file the fake's picker returns. */
export interface FakeBackupFile {
  file_name: string
  /** A backup of this notebook; otherwise it opens with `recoveryKey`. */
  same_vault: boolean
  notes: SeedNote[]
}

const INTERVAL_MS: Record<BackupInterval, number> = {
  hourly: 3_600_000,
  daily: 86_400_000,
  weekly: 604_800_000,
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

const KEY_ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'

/**
 * A recovery key as the host reads it (case, spaces, and hyphens ignored,
 * `I`/`L` as `1` and `O` as `0`), or `null` if it isn't 32 characters of
 * its alphabet. The host also checks a checksum; the fake has none.
 */
function normalizeKey(text: string): string | null {
  const chars = [...text.toUpperCase().replace(/[\s-]/g, '')].map(
    (c) => ({ I: '1', L: '1', O: '0' })[c] ?? c,
  )
  if (chars.length !== 32 || chars.some((c) => !KEY_ALPHABET.includes(c))) {
    return null
  }
  return chars.join('')
}

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
