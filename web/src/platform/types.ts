/**
 * The shapes that cross between the frontend and the core, as the host
 * serializes them (`crates/notebook-app/src/dto.rs`, `editor.rs`,
 * `error.rs`). Field names stay in the host's snake_case so these types
 * read the same as the Rust they mirror. Note ids are ULID strings, times
 * Unix milliseconds.
 */

export type PhaseName =
  /** No notebook on this device: the welcome screen. */
  | 'setup'
  /** A recovery key was generated and is waiting to be acknowledged. */
  | 'pending_recovery_key'
  /** A notebook exists but its key isn't in the secure store. */
  | 'needs_recovery_key'
  /** Upgrading a locked server-backed install: the old passphrase opens it. */
  | 'needs_old_passphrase'
  | 'open'

/** The `app_state` result and the `app-phase-changed` payload. */
export interface Phase {
  phase: PhaseName
  /**
   * False when the OS has no usable secure store, so the vault key isn't
   * remembered and the recovery key is asked for at each launch.
   */
  secure_store: boolean
  /**
   * True while converting a server-backed install, so the screens can say
   * that the old recovery key no longer matters.
   */
  upgrade: boolean
}

export type View = 'notes' | 'untagged' | 'pinned' | 'archive' | 'trash'

/** A sidebar selection: a special view, or a tag path in display spelling. */
export type Selection = { view: View } | { tag: string }

export interface TagNode {
  name: string
  /** The full path in display spelling, as used in a selection. */
  path: string
  count: number
  children: TagNode[]
}

export interface ListEntry {
  id: string
  /** Empty when the note has no title line; the list shows "Untitled". */
  title: string
  preview: string
  updated: number
  pinned: boolean
  archived: boolean
  trashed: boolean
}

/** The `vault-changed` payload: ids only, so the frontend re-queries. */
export interface VaultChanged {
  changed: string[]
  tree_changed: boolean
}

export type ThemePreference = 'system' | 'light' | 'dark'

export interface Settings {
  vim_mode: boolean
  theme: ThemePreference
  source_view: boolean
  focus_mode: boolean
  /** Tag paths, lowercased, that the user collapsed in the sidebar. */
  collapsed_tags: string[]
}

/** The `export_notes` result. */
export interface ExportResult {
  /** How many notes were exported. */
  count: number
  /** Where they went: the export folder, or the downloaded zip's name. */
  path: string
}

export type BackupInterval = 'hourly' | 'daily' | 'weekly'

/** The automatic backup options (`set_backup_options`). */
export interface BackupOptions {
  auto: boolean
  interval: BackupInterval
  /** How many backups to keep in the folder, at least 1. */
  keep: number
}

/** The backup status: every backup command's result, and the event's payload. */
export interface BackupStatus extends BackupOptions {
  /** The backup folder, for display only (never sent back), or `null`. */
  folder: string | null
  /** When the last backup succeeded, if one has. */
  last: number | null
  /**
   * When the next automatic backup is due: `last` + the interval when a
   * folder is set and auto is on, otherwise `null`.
   */
  next_due: number | null
  /** Notes changed since the last successful backup. */
  changed: boolean
  /** Why the most recent attempt failed, or `null` if it didn't. */
  last_error: string | null
  /** The most recent attempt failed (a folder is set). */
  failing: boolean
  /** Auto is on, notes changed, and none succeeded for 2 intervals. */
  overdue: boolean
  /** A backup is being written right now. */
  running: boolean
}

/** The `pick_backup` result: the backup file the user picked. */
export interface PickedBackup {
  file_name: string
  /** A backup of the open notebook, so no recovery key is needed. */
  same_vault: boolean
}

/** The `editor_open` result. */
export interface EditorOpened {
  session: number
  /** The note's full state, a base64 Yjs v1 update. */
  state: string
}

/** The `editor-remote-update` payload. */
export interface EditorRemoteUpdate {
  session: number
  /** What the editor lacks, a base64 Yjs v1 update. */
  update: string
}

/** Every event the core emits, by name. */
export interface CoreEvents {
  'app-phase-changed': Phase
  'vault-changed': VaultChanged
  'editor-remote-update': EditorRemoteUpdate
  'backup-status-changed': BackupStatus
}

/** The stable error codes (`Code` in `error.rs`). */
export type ErrorCode =
  | 'recovery_key_malformed'
  | 'recovery_key_wrong'
  | 'recovery_key_required'
  | 'wrong_passphrase'
  | 'invalid_tag_name'
  | 'trashed'
  | 'not_trashed'
  | 'not_open'
  | 'not_found'
  | 'export_failed'
  | 'backup_failed'
  | 'backup_no_folder'
  | 'backup_not_a_backup'
  | 'backup_too_new'
  | 'backup_damaged'
  | 'safety_backup_failed'
  | 'internal'

/**
 * A failed command. `message` is for logs and developer tools; the UI shows
 * its own text per code (see `lib/errors.ts`). Neither holds a secret.
 */
export interface CoreError {
  code: ErrorCode
  message: string
}

export function isCoreError(error: unknown): error is CoreError {
  return (
    typeof error === 'object' &&
    error !== null &&
    typeof (error as CoreError).code === 'string' &&
    typeof (error as CoreError).message === 'string'
  )
}

/** The code of a failed command, or `internal` for anything else. */
export function errorCode(error: unknown): ErrorCode {
  return isCoreError(error) ? error.code : 'internal'
}
