import type { BackupStatus, CoreApi } from '../../platform'

/**
 * The backup status, for the Backup settings and the sidebar notice. It
 * follows the core's `backup-status-changed` events; command results are
 * applied with {@link BackupState.set} so the UI doesn't wait for them.
 */
export class BackupState {
  /** `null` until the first answer, or if it couldn't be read. */
  status = $state<BackupStatus | null>(null)
  /** Backups are failing or overdue: the sidebar says so. */
  readonly needsAttention = $derived(
    !!this.status?.folder && (this.status.failing || this.status.overdue),
  )

  readonly #core: CoreApi
  #off: (() => void) | null = null

  constructor(core: CoreApi) {
    this.#core = core
  }

  /** Subscribes to changes and loads the status. */
  start(): Promise<void> {
    this.#off ??= this.#core.on('backup-status-changed', (status) =>
      this.set(status),
    )
    return this.refresh()
  }

  stop(): void {
    this.#off?.()
    this.#off = null
  }

  async refresh(): Promise<void> {
    try {
      this.set(await this.#core.backupStatus())
    } catch {
      // Keep what is shown; the next event or command brings it up to date.
    }
  }

  set(status: BackupStatus): void {
    this.status = status
  }
}
