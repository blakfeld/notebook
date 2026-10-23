# Spec Delta

## ADDED Requirements

### Requirement: Restore from backup on first launch
The welcome screen SHALL offer "Restore from backup" beside creating a new notebook. It SHALL let the user pick a backup file in the system file picker and enter the recovery key. After a successful restore the main layout SHALL show the restored notes. A malformed recovery key SHALL be reported as mistyped before the backup is opened.

#### Scenario: New computer
- **WHEN** a user on a fresh install picks a backup from their Dropbox folder and enters the recovery key
- **THEN** the main layout appears with every note from the backup

#### Scenario: Cancelled picker
- **WHEN** the user dismisses the file picker
- **THEN** the welcome screen stays as it was and no error is shown

### Requirement: Backup settings
Settings SHALL have a Backup section where the user can choose the backup folder in the system folder picker, turn automatic backups on or off, and choose the interval (hourly, daily, weekly) and the number of backups to keep. It SHALL show the folder, the last successful backup time, when the next is due, and the last error if the most recent attempt failed. It SHALL offer Back up now. It SHALL say that backups are encrypted and need the recovery key to open on another computer.

#### Scenario: No folder yet
- **WHEN** the user opens Backup settings before choosing a folder
- **THEN** the section offers to choose a folder and Back up now is unavailable

#### Scenario: Status shown
- **WHEN** a backup succeeded earlier today
- **THEN** the section shows that time and when the next automatic backup is due

### Requirement: Restore from settings
Settings SHALL offer "Restore from backup…", which replaces the device's notes after the user confirms that the current notes will be replaced. When a backup folder is set, the app SHALL first write a backup of the current notes, and SHALL NOT restore if that backup fails unless the user confirms again.

#### Scenario: Safety backup first
- **WHEN** the user restores an older backup from settings with a backup folder set
- **THEN** a backup of the current notes is written before they are replaced

#### Scenario: Cancelled confirmation
- **WHEN** the user cancels the replace confirmation
- **THEN** no backup is written and the notes are unchanged

### Requirement: Backup problem notice
When backups are failing or overdue, the sidebar SHALL show a short notice that opens Backup settings. When backups are healthy, or no folder has been chosen, no notice SHALL be shown.

#### Scenario: Dropbox folder gone
- **WHEN** automatic backups fail because the folder is unavailable
- **THEN** the sidebar shows that backups are failing, and selecting it opens Backup settings

#### Scenario: Healthy
- **WHEN** the last backup succeeded and none is overdue
- **THEN** the sidebar shows no backup notice
