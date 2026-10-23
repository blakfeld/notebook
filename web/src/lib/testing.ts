// Helpers for component tests (jsdom). Not part of the app.
import { flushSync, mount, unmount, type Component } from 'svelte'

/** Mounts `component` into a fresh element in the document. */
export function render<Props extends Record<string, unknown>>(
  component: Component<Props>,
  props: Props,
) {
  const target = document.createElement('div')
  document.body.append(target)
  const instance = mount(component, { target, props })
  flushSync()
  return {
    target,
    destroy() {
      unmount(instance)
      target.remove()
    },
  }
}

/** The form control labelled `label` (by `<label>` text or `aria-label`). */
export function field(root: ParentNode, label: string): HTMLInputElement {
  for (const el of root.querySelectorAll<HTMLInputElement>('input')) {
    if (el.getAttribute('aria-label') === label) return el
    const labelEl =
      el.closest('label') ??
      (el.id ? document.querySelector(`label[for="${el.id}"]`) : null)
    if (labelEl && ownText(labelEl) === label) return el
  }
  throw new Error(`no field labelled ${label}`)
}

/** An element's own text, without its children's (hints, errors). */
function ownText(el: Element): string {
  return [...el.childNodes]
    .filter((node) => node.nodeType === Node.TEXT_NODE)
    .map((node) => node.textContent)
    .join(' ')
    .replace(/\s+/g, ' ')
    .trim()
}

/** Types into an input as a user would, firing `input`. */
export function fill(input: HTMLInputElement, value: string): void {
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
  flushSync()
}

export function check(input: HTMLInputElement, checked = true): void {
  input.checked = checked
  input.dispatchEvent(new Event('change', { bubbles: true }))
  flushSync()
}

/** The button whose text or `aria-label` is `name`. */
export function button(root: ParentNode, name: string): HTMLButtonElement {
  const found = buttons(root).find((b) => nameOf(b) === name)
  if (!found) {
    const names = buttons(root).map(nameOf)
    throw new Error(`no button "${name}" (have: ${names.join(', ')})`)
  }
  return found
}

export function hasButton(root: ParentNode, name: string): boolean {
  return buttons(root).some((b) => nameOf(b) === name)
}

function buttons(root: ParentNode): HTMLButtonElement[] {
  return [...root.querySelectorAll<HTMLButtonElement>('button')]
}

function nameOf(el: HTMLElement): string {
  return (
    el.getAttribute('aria-label') ??
    (el.textContent ?? '').replace(/\s+/g, ' ').trim()
  )
}

export function click(el: HTMLElement): void {
  el.click()
  flushSync()
}

/** Text of every `role="alert"` element. */
export function alerts(root: ParentNode): string[] {
  return [...root.querySelectorAll('[role="alert"]')].map((el) =>
    (el.textContent ?? '').replace(/\s+/g, ' ').trim(),
  )
}

/** Lets pending promises settle, then flushes rendering. */
export async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
  await new Promise((resolve) => setTimeout(resolve, 0))
  flushSync()
}
