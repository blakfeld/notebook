import type {
  BackupOptions,
  BackupStatus,
  CoreEvents,
  EditorOpened,
  ExportResult,
  ListEntry,
  Phase,
  PickedBackup,
  Selection,
  Settings,
  TagNode,
} from './types'

/**
 * Everything the UI asks of the core: one method per host command, plus
 * event subscription. The Tauri shell implements it over IPC (`tauri.ts`),
 * the browser client will implement it over WASM, and tests use
 * `FakeCoreApi`. UI code imports only this interface.
 *
 * Every method rejects with a {@link CoreError} on failure. Passphrases and
 * recovery keys go in and are never returned, except the new recovery key,
 * shown until the user acknowledges it.
 */
export interface CoreApi {
  // ---- app and notebook ----
  appState(): Promise<Phase>
  /** The app's version (the workspace's, shared by core and host). */
  coreVersion(): Promise<string>
  /**
   * Generates a new notebook in memory, moves to `pending_recovery_key`,
   * and returns the recovery key's display form. Only valid in `setup`.
   */
  createNotebook(): Promise<string>
  /**
   * The recovery key waiting to be acknowledged, for when the screen shows
   * without one in hand (an upgrade generates it at launch). Only valid in
   * `pending_recovery_key`.
   */
  pendingRecoveryKey(): Promise<string>
  /** The user saved the recovery key: writes the notebook and opens it. */
  acknowledgeRecoveryKey(): Promise<Phase>
  /**
   * Opens the notebook when its key isn't in the secure store. Fails with
   * `recovery_key_malformed` for a mistyped key and `recovery_key_wrong`
   * for one that doesn't open this notebook (`internal` if the database
   * can't be read).
   */
  openWithRecoveryKey(recoveryKey: string): Promise<Phase>
  /** Upgrades a locked server-backed install with its old passphrase. */
  upgradeWithPassphrase(passphrase: string): Promise<Phase>
  /**
   * Gives up on the notebook that can't be opened (in `needs_old_passphrase`
   * or `needs_recovery_key`): its database is set aside under another name
   * (not deleted) and the welcome screen shows.
   */
  startOver(): Promise<Phase>

  // ---- vault ----
  tagTree(): Promise<TagNode[]>
  listNotes(selection: Selection, query: string): Promise<ListEntry[]>
  /** Creates a note for the selection (starting with its tag) and returns its id. */
  createNote(selection: Selection): Promise<string>
  setPinned(id: string, pinned: boolean): Promise<void>
  setArchived(id: string, archived: boolean): Promise<void>
  setTrashed(id: string, trashed: boolean): Promise<void>
  /** Deletes a trashed note from this device. */
  deletePermanently(id: string): Promise<void>
  /** Renames or moves a tag across every note; returns how many changed. */
  renameTag(old: string, renamed: string): Promise<number>
  /**
   * Exports every note as markdown (see `docs/export-format.md`): into a
   * folder the user picks on desktop, as a zip download in the browser.
   * Resolves `null` when the user cancels. Fails with `export_failed`,
   * whose message says why.
   */
  exportNotes(args: { includeTrashed: boolean }): Promise<ExportResult | null>

  // ---- backup (see `docs/backup-format.md`) ----
  backupStatus(): Promise<BackupStatus>
  /**
   * Opens the system folder picker and saves the choice. Resolves `null`
   * when the user cancels. The first folder chosen gets a backup at once;
   * if that fails the folder is still saved and the status says why.
   */
  chooseBackupFolder(): Promise<BackupStatus | null>
  setBackupOptions(options: BackupOptions): Promise<BackupStatus>
  /**
   * Writes a backup now. Fails with `backup_no_folder`, or `backup_failed`
   * whose message says why.
   */
  backupNow(): Promise<BackupStatus>
  /**
   * Opens the system file picker for a backup to restore, on the welcome
   * screen or with the notebook open, and remembers it for
   * {@link restoreBackup}. Resolves `null` when the user cancels. Fails
   * with `backup_not_a_backup`, `backup_too_new`, or `backup_damaged`.
   */
  pickBackup(): Promise<PickedBackup | null>
  /** Forgets the picked backup, when the user cancels the restore. */
  cancelRestore(): Promise<void>
  /**
   * Replaces this device's notes with the picked backup and opens it.
   * `recoveryKey` is needed unless the backup is of the open notebook
   * (`recovery_key_required`, `recovery_key_malformed`,
   * `recovery_key_wrong`). With the notebook open and a backup folder set,
   * the current notes are backed up first unless `skipSafetyBackup`; if
   * that fails it rejects with `safety_backup_failed` and changes nothing.
   */
  restoreBackup(args: {
    recoveryKey: string | null
    skipSafetyBackup: boolean
  }): Promise<Phase>

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
