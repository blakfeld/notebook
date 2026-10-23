# Spec Delta

## ADDED Requirements

### Requirement: Import a note from a snapshot
A store SHALL import a note as its wrapped key and an encrypted base snapshot with no log entries, in one atomic step. An imported note SHALL load with that key and snapshot and SHALL be listed like any other note. Importing a note whose ID already exists SHALL fail and leave the existing note unchanged.

#### Scenario: Imported note loads
- **WHEN** note N is imported with wrapped key K and snapshot S
- **THEN** N is listed, and loading N returns K, base snapshot S, and no entries

#### Scenario: Duplicate import
- **WHEN** a note is imported with an ID the store already holds
- **THEN** import fails with a duplicate-note error and the existing note is unchanged

#### Scenario: Edits after import
- **WHEN** a local update is appended to an imported note
- **THEN** loading the note returns snapshot S followed by that update
