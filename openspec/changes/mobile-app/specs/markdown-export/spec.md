# Spec Delta

## MODIFIED Requirements

### Requirement: Folder output
On desktop clients, export SHALL write the export folder inside a parent folder the user chooses. It SHALL NOT overwrite existing files: if a folder with the export name exists, the name SHALL get the smallest `-N` suffix (N ≥ 2) that is free. Mobile clients SHALL use the zip output instead. Each file's modification time SHALL be set to the note's updated time where the filesystem supports it.

#### Scenario: Export into a chosen folder
- **WHEN** the user picks `~/Backups` and exports at 2026-10-21 14:03:05 UTC
- **THEN** `~/Backups/notebook-export-20261021-140305/` contains one `.md` file per selected note

#### Scenario: Existing folder is not overwritten
- **WHEN** `notebook-export-20261021-140305` already exists in the chosen folder
- **THEN** the export is written to `notebook-export-20261021-140305-2` and the existing folder is unchanged

#### Scenario: File times match notes
- **WHEN** a note last updated at 2024-10-22T14:03:05Z is exported
- **THEN** its file's modification time is 2024-10-22T14:03:05Z
