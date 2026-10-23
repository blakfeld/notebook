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
}

/** The stable error codes (`Code` in `error.rs`). */
export type ErrorCode =
  | 'recovery_key_malformed'
  | 'recovery_key_wrong'
  | 'wrong_passphrase'
  | 'invalid_tag_name'
  | 'trashed'
  | 'not_trashed'
  | 'not_open'
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
