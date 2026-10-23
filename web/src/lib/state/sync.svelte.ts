import { errorCode, type CoreApi, type SyncStatus } from '../../platform'

/** The sync engine's status, from `sync_status` and `sync-status` events. */
export class SyncStatusState {
  /** `null` until the first answer. */
  status = $state<SyncStatus | null>(null)
  /**
   * Set when a command failed with `sign_in_required`, so the sign-in
   * prompt shows even before the engine reports it.
   */
  signInRequested = $state(false)
  readonly #core: CoreApi
  #off: (() => void) | null = null

  constructor(core: CoreApi) {
    this.#core = core
  }

  async start(): Promise<void> {
    this.#off ??= this.#core.on('sync-status', (status) => {
      this.status = status
    })
    try {
      const status = await this.#core.syncStatus()
      this.status ??= status
    } catch {
      // Locked meanwhile; the next unlock starts again.
    }
  }

  /** Asks for the sign-in prompt if `error` says the session ended. */
  checkError(error: unknown): void {
    if (errorCode(error) === 'sign_in_required') this.signInRequested = true
  }

  stop(): void {
    this.#off?.()
    this.#off = null
  }
}
