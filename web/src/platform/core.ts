import type {
  CoreEvents,
  EditorOpened,
  ListEntry,
  Phase,
  Selection,
  Settings,
  SyncStatus,
  TagNode,
} from './types'

/**
 * Everything the UI asks of the core: one method per host command, plus
 * event subscription. The Tauri shell implements it over IPC (`tauri.ts`),
 * the browser client will implement it over WASM, and tests use
 * `FakeCoreApi`. UI code imports only this interface.
 *
 * Every method rejects with a {@link CoreError} on failure. Passphrases and
 * recovery keys go in and are never returned, except the recovery key
 * `generateRecoveryKey` shows once during registration.
 */
export interface CoreApi {
  // ---- app and account ----
  appState(): Promise<Phase>
  signIn(server: string, username: string, passphrase: string): Promise<Phase>
  /**
   * Generates the recovery key for the next `register` and returns its
   * display form. The core keeps it; `register` uses it.
   */
  generateRecoveryKey(): Promise<string>
  register(args: {
    server: string
    username: string
    passphrase: string
    confirm: string
    inviteCode: string
  }): Promise<Phase>
  recover(args: {
    server: string
    username: string
    recoveryKey: string
    passphrase: string
    confirm: string
  }): Promise<Phase>
  unlock(passphrase: string): Promise<Phase>
  lock(): Promise<Phase>
  /** Signs in again after the session ended, keeping the vault open. */
  reauthenticate(passphrase: string): Promise<void>
  changePassphrase(args: {
    current: string
    new: string
    confirm: string
  }): Promise<void>
  /**
   * Signs out and deletes this device's copy of the vault. Fails with
   * `pending_changes` when changes are unsynced and `confirm` is false.
   */
  signOut(confirm: boolean): Promise<Phase>
  pendingCount(): Promise<number>
  syncStatus(): Promise<SyncStatus>

  // ---- vault ----
  tagTree(): Promise<TagNode[]>
  listNotes(selection: Selection, query: string): Promise<ListEntry[]>
  /** Creates a note for the selection (starting with its tag) and returns its id. */
  createNote(selection: Selection): Promise<string>
  setPinned(id: string, pinned: boolean): Promise<void>
  setArchived(id: string, archived: boolean): Promise<void>
  setTrashed(id: string, trashed: boolean): Promise<void>
  /** Deletes a trashed note on the server, then here. Needs a connection. */
  deletePermanently(id: string): Promise<void>
  /** Renames or moves a tag across every note; returns how many changed. */
  renameTag(old: string, renamed: string): Promise<number>

  // ---- editor host (see `editor/bridge.ts`; updates are base64) ----
  editorOpen(note: string): Promise<EditorOpened>
  editorPush(session: number, update: string): Promise<void>
  editorClose(session: number): Promise<void>

  // ---- device settings ----
  getSettings(): Promise<Settings>
  setVimMode(on: boolean): Promise<void>
  /**
   * Stores a UI preference: `appearance.theme` (`system`, `light`, `dark`),
   * `editor.sourceView` or `editor.focusMode` (`true`, `false`).
   */
  setPreference(key: string, value: string): Promise<void>
  setTagExpanded(path: string, expanded: boolean): Promise<void>

  /**
   * Subscribes to a core event. Returns an unsubscribe function whose
   * `ready` resolves once events are being delivered (subscribing can take
   * a round trip to the host).
   */
  on<E extends keyof CoreEvents>(
    event: E,
    handler: (payload: CoreEvents[E]) => void,
  ): Subscription
}

/** An event subscription: call it to unsubscribe. */
export type Subscription = (() => void) & {
  /** Resolves once events reach the handler; rejects if subscribing failed. */
  readonly ready: Promise<void>
}
