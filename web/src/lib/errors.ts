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
  recovery_key_required: 'Enter the recovery key for this backup.',
  wrong_passphrase: 'That passphrase is incorrect.',
  invalid_tag_name:
    'That isn’t a valid tag name. A tag can’t be only digits, contain # or a line break, or end in punctuation.',
  trashed: 'This note is in Trash. Restore it to edit it.',
  not_trashed: 'Only notes in Trash can be deleted permanently.',
  not_open: 'The notebook isn’t open.',
  not_found: 'That note no longer exists.',
  export_failed: 'The export failed. Nothing was written.',
  backup_failed: 'The backup failed.',
  backup_no_folder: 'Choose a backup folder first.',
  backup_not_a_backup: 'That file isn’t a Notebook backup.',
  backup_too_new:
    'That backup was made by a newer version of Notebook. Update Notebook to restore it.',
  backup_damaged: 'That backup is damaged and can’t be restored.',
  safety_backup_failed:
    'Your current notes couldn’t be backed up first, so nothing was replaced.',
  internal: 'Something went wrong. Try again.',
}

/** Failures whose host message says why, shown after the code's own. */
const SAYS_WHY = new Set<ErrorCode>([
  'export_failed',
  'backup_failed',
  'safety_backup_failed',
])

/** The user-facing message for a failed command. */
export function errorMessage(error: unknown): string {
  if (!isCoreError(error)) return MESSAGES.internal
  // Export and backup failures say why (e.g. the folder isn't writable).
  if (SAYS_WHY.has(error.code) && error.message) {
    return `${MESSAGES[error.code]} ${sentence(error.message)}`
  }
  return MESSAGES[error.code] ?? MESSAGES.internal
}

/**
 * The message for a failed restore: as {@link errorMessage}, except that a
 * wrong key is one that doesn't open the backup.
 */
export function restoreErrorMessage(error: unknown): string {
  return isCoreError(error) && error.code === 'recovery_key_wrong'
    ? 'That recovery key doesn’t open this backup.'
    : errorMessage(error)
}

/** A host message as a sentence: capitalized, with a full stop. */
export function sentence(text: string): string {
  const trimmed = text.trim()
  const capital = trimmed.charAt(0).toUpperCase() + trimmed.slice(1)
  return /[.!?]$/.test(capital) ? capital : `${capital}.`
}
