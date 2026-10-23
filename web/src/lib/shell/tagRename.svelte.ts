import { errorMessage } from '../errors'
import type { Vault } from '../state'

/**
 * The sidebar's inline tag rename. The user edits the tag's full path, so
 * a rename can also move it under another parent. One tag at a time.
 */
export class TagRenamer {
  /** The tag being renamed, or `null`. */
  path = $state<string | null>(null)
  value = $state('')
  error = $state('')
  busy = $state(false)
  readonly #vault: Vault

  constructor(vault: Vault) {
    this.#vault = vault
  }

  start(path: string): void {
    this.path = path
    this.value = path
    this.error = ''
  }

  cancel(): void {
    this.path = null
    this.error = ''
  }

  async commit(): Promise<void> {
    const old = this.path
    const next = this.value.trim()
    if (old === null || this.busy) return
    if (next === old) {
      this.cancel()
      return
    }
    const { core, notes, selection } = this.#vault
    this.busy = true
    this.error = ''
    try {
      await core.renameTag(old, next)
      // Keep showing the renamed tag (or its renamed descendant).
      const current = selection.current
      if ('tag' in current) {
        const tag = current.tag.toLowerCase()
        const prefix = old.toLowerCase()
        if (tag === prefix || tag.startsWith(`${prefix}/`)) {
          selection.select({ tag: next + current.tag.slice(old.length) })
        }
      }
      this.cancel()
      await notes.refresh()
    } catch (error) {
      this.error = errorMessage(error)
    } finally {
      this.busy = false
    }
  }
}
