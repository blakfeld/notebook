import type { CoreApi, Phase } from '../../platform'

/**
 * Which screen the app shows: signed out, locked, or unlocked. Follows the
 * core's `app-phase-changed` events, and command results that return a
 * phase are applied with {@link PhaseState.set} so the UI doesn't wait for
 * the event.
 */
export class PhaseState {
  /** `null` until the first `appState` answer. */
  current = $state<Phase | null>(null)
  /** Why the phase couldn't be read, if it couldn't. */
  error = $state<unknown>(null)
  readonly #core: CoreApi
  #off: (() => void) | null = null

  constructor(core: CoreApi) {
    this.#core = core
  }

  async start(): Promise<void> {
    this.#off ??= this.#core.on('app-phase-changed', (phase) => this.set(phase))
    try {
      const phase = await this.#core.appState()
      // An event that arrived meanwhile is at least as new.
      this.current ??= phase
    } catch (error) {
      this.error = error
    }
  }

  set(phase: Phase): void {
    this.current = phase
    this.error = null
  }

  stop(): void {
    this.#off?.()
    this.#off = null
  }
}
