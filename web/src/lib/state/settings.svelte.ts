import { coreSettings, type CoreApi, type DeviceSettings } from '../../platform'

/**
 * Device-local settings: vim mode and which sidebar tags are collapsed.
 * Changes apply at once and are written to the core in the background.
 */
export class SettingsState {
  vimMode = $state(false)
  /** Lowercased paths of collapsed tags. */
  collapsed = $state<string[]>([])
  /** The last failed write, for the UI to report. */
  error = $state<unknown>(null)
  /** For the editor, which reads and saves vim mode itself. */
  readonly device: DeviceSettings
  readonly #core: CoreApi

  constructor(core: CoreApi) {
    this.#core = core
    this.device = coreSettings(core)
  }

  async load(): Promise<void> {
    try {
      const settings = await this.#core.getSettings()
      this.vimMode = settings.vim_mode
      this.collapsed = settings.collapsed_tags
    } catch (error) {
      this.error = error
    }
  }

  isExpanded(path: string): boolean {
    return !this.collapsed.includes(path.toLowerCase())
  }

  async setExpanded(path: string, expanded: boolean): Promise<void> {
    const key = path.toLowerCase()
    const rest = this.collapsed.filter((p) => p !== key)
    this.collapsed = expanded ? rest : [...rest, key]
    try {
      await this.#core.setTagExpanded(path, expanded)
    } catch (error) {
      this.error = error
    }
  }

  /**
   * Turns vim mode on or off. The open editor follows the new value; the
   * write goes through the editor's settings adapter, so it happens once.
   */
  async setVimMode(on: boolean): Promise<void> {
    this.vimMode = on
    try {
      await this.device.set('editor.vimMode', String(on))
    } catch (error) {
      this.error = error
    }
  }
}
