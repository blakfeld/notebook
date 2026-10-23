# Spec Delta

## ADDED Requirements

### Requirement: Export all notes from settings
Settings SHALL offer an "Export all notes" action with an include-trashed option that is off by default. It SHALL write a markdown export, as defined by the `markdown-export` capability, into a folder the user picks in the system folder picker.

#### Scenario: Desktop export
- **WHEN** the user chooses "Export all notes" and picks a folder in the system folder picker
- **THEN** the export folder is written inside the picked folder

#### Scenario: Cancelled picker
- **WHEN** the user dismisses the folder picker without choosing a folder
- **THEN** nothing is written and no error is shown

#### Scenario: Include trashed option
- **WHEN** the user turns on the include-trashed option before exporting
- **THEN** the export also contains the trashed notes

### Requirement: Export warns that output is unencrypted
Before exporting, the action SHALL tell the user that exported files are plain, unencrypted text readable by anyone with access to them, unlike backups, and SHALL advise against exporting into a cloud-synced folder.

#### Scenario: Warning shown
- **WHEN** the user opens the export action
- **THEN** the unencrypted-output warning is visible before the export starts

### Requirement: Export result is reported
After an export, the app SHALL report success with the number of notes exported and where they went, or report failure with the error message. A failed export SHALL NOT be reported as successful.

#### Scenario: Success message
- **WHEN** 42 notes are exported to a folder
- **THEN** the app reports that 42 notes were exported and shows the export folder's location

#### Scenario: Failure message
- **WHEN** the export fails because the chosen folder is not writable
- **THEN** the app shows an error explaining the failure
