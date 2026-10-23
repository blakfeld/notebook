# Spec Delta

## Purpose

Defines the app's visual themes: light and dark color schemes that follow the operating system by default, a per-device manual override, and the contrast every theme must meet across the editor and the surrounding UI.

## ADDED Requirements

### Requirement: Light and dark themes
The client SHALL provide a light theme and a dark theme. A theme SHALL apply to the whole client at once, including the editor, sidebar, note list, dialogs, and settings, so no surface shows one theme while another shows the other.

#### Scenario: Whole UI switches together
- **WHEN** the active theme changes from light to dark
- **THEN** the editor, sidebar, note list, and settings all render with dark theme colors

### Requirement: Theme follows the system by default
The theme preference SHALL have three values: System, Light, and Dark, defaulting to System. Under System, the client SHALL use the dark theme when the operating system or browser prefers a dark color scheme and the light theme otherwise, and SHALL switch live when that preference changes, without a reload.

#### Scenario: Default follows OS
- **WHEN** the preference has never been set and the OS prefers dark
- **THEN** the client uses the dark theme

#### Scenario: OS switches while open
- **WHEN** the preference is System and the OS changes from light to dark while a note is open
- **THEN** the client switches to the dark theme without reloading and the open note keeps its cursor position and unsaved state

### Requirement: Manual theme override
The user SHALL be able to set the theme preference from Settings. Light or Dark SHALL apply that theme regardless of the OS preference, and the change SHALL take effect immediately.

#### Scenario: Force light on a dark OS
- **WHEN** the OS prefers dark and the user selects Light
- **THEN** the client immediately uses the light theme and stays light while the OS preference remains dark

#### Scenario: Return to System
- **WHEN** the preference is Dark, the OS prefers light, and the user selects System
- **THEN** the client switches to the light theme

### Requirement: Theme preference is per device
The theme preference SHALL be stored as a device-local, non-sensitive UI preference. It SHALL NOT be stored in the vault or included in backups, and it SHALL persist across restarts.

#### Scenario: Not synced
- **WHEN** the user selects Dark and a backup of the notebook is restored on another computer
- **THEN** that computer keeps its own theme preference

#### Scenario: Survives restart
- **WHEN** the user selects Light and restarts the desktop app
- **THEN** the app starts with the light theme

#### Scenario: Unavailable storage
- **WHEN** device-local storage cannot be read
- **THEN** the client uses System and still renders correctly

### Requirement: No flash of the wrong theme
The client SHALL apply the resolved theme before first paint of the app UI, so a client whose preference is Dark never briefly shows the light theme on startup, and vice versa.

#### Scenario: Dark startup
- **WHEN** the preference is Dark and the app starts
- **THEN** the first rendered frame of the app UI uses dark theme colors

### Requirement: Readable contrast
In both themes, body text, headings, links, tag pills, inline code, and code block text SHALL have a contrast ratio of at least 4.5:1 against their background, and UI controls, focus indicators, and checkbox outlines SHALL have at least 3:1. De-emphasized text, including revealed markdown syntax and focus-mode dimming, SHALL keep at least 3:1.

#### Scenario: Body text contrast
- **WHEN** body text color is measured against the editor background in either theme
- **THEN** the contrast ratio is at least 4.5:1

#### Scenario: Dimmed text stays legible
- **WHEN** focus mode dims a paragraph in either theme
- **THEN** the dimmed text has a contrast ratio of at least 3:1 against the background
