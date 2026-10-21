# Proposal

## Why

Notes must be available and editable on phones, offline, with edits syncing to desktop and the browser. Tauri 2 targets iOS and Android, so the same Rust core, SQLite storage, sync engine, and frontend can ship to mobile. However, a three-pane desktop layout and a vim-first editor don't work on a touch screen, so mobile needs its own UX.

## What Changes

- Tauri 2 iOS and Android builds of `notebook-app`, using the native SQLite storage and sync engine.
- **Mobile navigation**: a stacked flow (tag tree, then note list, then editor) with back gestures, replacing the three-pane layout on narrow screens.
- **Editor on touch**:
  - A formatting toolbar above the keyboard (headings, bold, italic, list, task, tag, link, undo/redo).
  - Vim mode defaults to off on mobile. It stays available as a per-device setting and is useful with a hardware keyboard (e.g. iPad).
  - Live preview behaves correctly with touch selection and the virtual keyboard.
- **Secure credentials**: refresh token and key material in iOS Keychain / Android Keystore. Optional biometric unlock.
- **Lifecycle**: sync on foreground, flush the outbox on background, and lock after a configurable inactivity period.
- Quick-capture entry point: open straight into a new note.
- Export uses the platform share sheet.

## Capabilities

### New Capabilities
- `mobile-experience`: Mobile-specific behavior: stacked navigation, touch formatting toolbar, mobile vim defaults, biometric unlock, app lifecycle sync/lock rules, quick capture, and share-sheet export.

### Modified Capabilities
- `app-shell` (introduced by `desktop-app-shell`): the layout requirement becomes responsive, using the three-pane layout on wide screens and stacked navigation on narrow ones.

## Impact

- `notebook-app`: Tauri mobile targets, platform plugins (secure storage, biometrics, share sheet), and iOS/Android project files.
- Frontend: responsive layout, touch toolbar, and safe-area handling.
- Build infrastructure: Xcode and Android SDK in CI, plus signing for distribution.
