/**
 * The shapes that cross between the frontend and the core, as the host
 * serializes them (`crates/notebook-app/src/dto.rs`, `editor.rs`,
 * `error.rs`). Field names stay in the host's snake_case so these types
 * read the same as the Rust they mirror. Note ids are ULID strings, times
 * Unix milliseconds.
 */

export type PhaseName = 'signed_out' | 'locked' | 'unlocked'

export interface Account {
  server: string
  username: string
}

/** The `app_state` result and the `app-phase-changed` payload. */
export interface Phase {
  phase: PhaseName
  account: Account | null
  /**
   * False when there is no OS secure store, so the user will have to sign
   * in again next launch.
   */
  credentials_remembered: boolean
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

export type SyncState = 'synced' | 'syncing' | 'offline'

export type Attention =
  | 'sign_in_required'
  | 'upgrade_required'
  | 'quota_full'
  | 'update_too_large'
  | 'push_blocked'
  | 'integrity_warning'

/** The `sync_status` result and the `sync-status` payload. */
export interface SyncStatus {
  state: SyncState
  /** Local changes the server hasn't acknowledged. */
  pending: number
  attention: Attention[]
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
  'sync-status': SyncStatus
  'vault-changed': VaultChanged
  'editor-remote-update': EditorRemoteUpdate
}

/** The stable error codes (`Code` in `error.rs`). */
export type ErrorCode =
  | 'invalid_server_url'
  | 'invalid_username'
  | 'passphrase_too_short'
  | 'passphrase_mismatch'
  | 'bad_credentials'
  | 'sign_in_required'
  | 'unreachable'
  | 'rate_limited'
  | 'invalid_invite'
  | 'username_taken'
  | 'recovery_key_required'
  | 'recovery_key_malformed'
  | 'recovery_key_wrong'
  | 'wrong_passphrase'
  | 'offline'
  | 'pending_changes'
  | 'invalid_tag_name'
  | 'trashed'
  | 'not_trashed'
  | 'locked'
  | 'not_found'
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
