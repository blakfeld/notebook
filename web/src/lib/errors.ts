import { isCoreError, type ErrorCode } from '../platform'

/** The minimum passphrase length, in characters (as in `notebook-core`). */
export const MIN_PASSPHRASE_CHARS = 12

/**
 * What the UI says for each error code. The host's messages are for logs;
 * these are for people. `bad_credentials` covers both an unknown username
 * and a wrong passphrase, so neither reveals which accounts exist.
 */
const MESSAGES: Record<ErrorCode, string> = {
  invalid_server_url: 'That server address isn’t valid.',
  invalid_username: 'That username isn’t valid.',
  passphrase_too_short: `The passphrase must be at least ${MIN_PASSPHRASE_CHARS} characters.`,
  passphrase_mismatch: 'The passphrases don’t match.',
  bad_credentials: 'The username or passphrase is incorrect.',
  sign_in_required:
    'Your session has ended. Sign in again with your passphrase, then try again.',
  unreachable:
    'The server could not be reached. Check the address and your connection.',
  rate_limited: 'Too many attempts. Wait a minute and try again.',
  invalid_invite:
    'That invite code wasn’t accepted. It may be mistyped, already used, or expired.',
  username_taken: 'That username is taken. Choose a different one.',
  recovery_key_required: 'The recovery key is missing. Start again.',
  recovery_key_malformed:
    'That recovery key looks mistyped. Check each group and try again.',
  recovery_key_wrong: 'That recovery key doesn’t unlock this account.',
  wrong_passphrase: 'That passphrase is incorrect.',
  offline: 'This needs a connection to the server. Try again when online.',
  pending_changes: 'Some changes haven’t synced yet.',
  invalid_tag_name:
    'That isn’t a valid tag name. A tag can’t be only digits, contain # or a line break, or end in punctuation.',
  trashed: 'This note is in Trash. Restore it to edit it.',
  not_trashed: 'Only notes in Trash can be deleted permanently.',
  locked: 'The vault is locked.',
  not_found: 'That note no longer exists.',
  export_failed: 'The export failed. Nothing was written.',
  internal: 'Something went wrong. Try again.',
}

/** The user-facing message for a failed command. */
export function errorMessage(error: unknown): string {
  if (!isCoreError(error)) return MESSAGES.internal
  // The server address check names the problem itself (e.g. that HTTPS is
  // required), and its message is written for people.
  if (error.code === 'invalid_server_url' && error.message) {
    return sentence(error.message)
  }
  // An export failure says why (e.g. the folder isn't writable).
  if (error.code === 'export_failed' && error.message) {
    return `${MESSAGES.export_failed} ${sentence(error.message)}`
  }
  return MESSAGES[error.code] ?? MESSAGES.internal
}

/**
 * Checks a new passphrase and its confirmation before anything is sent.
 * Returns the message to show, or `null` when they're fine.
 */
export function checkNewPassphrase(
  passphrase: string,
  confirm: string,
): string | null {
  // Characters, not UTF-16 units, as the core counts them.
  if ([...passphrase].length < MIN_PASSPHRASE_CHARS) {
    return MESSAGES.passphrase_too_short
  }
  if (passphrase !== confirm) return MESSAGES.passphrase_mismatch
  return null
}

function sentence(text: string): string {
  const trimmed = text.trim()
  const capital = trimmed.charAt(0).toUpperCase() + trimmed.slice(1)
  return /[.!?]$/.test(capital) ? capital : `${capital}.`
}
