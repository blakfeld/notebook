import { isCoreError, type ErrorCode } from '../platform'

/**
 * What the UI says for each error code. The host's messages are for logs;
 * these are for people. A mistyped recovery key and one that doesn't open
 * the notebook get different messages, since the fix differs.
 */
const MESSAGES: Record<ErrorCode, string> = {
  recovery_key_malformed:
    'That recovery key looks mistyped. Check each group and try again.',
  recovery_key_wrong: 'That recovery key doesn’t open this notebook.',
  wrong_passphrase: 'That passphrase is incorrect.',
  invalid_tag_name:
    'That isn’t a valid tag name. A tag can’t be only digits, contain # or a line break, or end in punctuation.',
  trashed: 'This note is in Trash. Restore it to edit it.',
  not_trashed: 'Only notes in Trash can be deleted permanently.',
  not_open: 'The notebook isn’t open.',
  not_found: 'That note no longer exists.',
  export_failed: 'The export failed. Nothing was written.',
  internal: 'Something went wrong. Try again.',
}

/** The user-facing message for a failed command. */
export function errorMessage(error: unknown): string {
  if (!isCoreError(error)) return MESSAGES.internal
  // An export failure says why (e.g. the folder isn't writable).
  if (error.code === 'export_failed' && error.message) {
    return `${MESSAGES.export_failed} ${sentence(error.message)}`
  }
  return MESSAGES[error.code] ?? MESSAGES.internal
}

function sentence(text: string): string {
  const trimmed = text.trim()
  const capital = trimmed.charAt(0).toUpperCase() + trimmed.slice(1)
  return /[.!?]$/.test(capital) ? capital : `${capital}.`
}
