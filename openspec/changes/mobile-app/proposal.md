# Proposal

## Why

Notes must be available and editable on phones, offline, with edits syncing to desktop and the browser. Tauri 2 targets iOS and Android, so the same Rust core, SQLite storage, sync engine, and frontend can ship to mobile. However, a three-pane desktop layout and a vim-first editor don't work on a touch screen, and a phone is lost or handed to someone else far more often than a laptop, so mobile needs its own UX and a stricter local lock.

## What Changes

- Tauri 2 iOS and Android builds of `notebook-app`, using the native SQLite storage and sync engine. The local database is excluded from device backups (the server holds the encrypted copy).
- **Responsive layout** (all clients): below a width breakpoint the three-pane layout becomes a **stacked flow** (sidebar, then note list, then editor) with back navigation. Wide screens (iPad landscape, desktop windows) keep three panes. This also makes the browser client usable on a phone.
- **Native back navigation**: iOS edge-swipe back and the Android system back gesture/button move up the stack.
- **Editor on touch**:
  - A formatting toolbar above the virtual keyboard (heading, bold, italic, list, task, tag, link, undo/redo, dismiss keyboard).
  - The caret and toolbar stay visible as the keyboard opens and resizes, and content respects safe areas.
  - Vim mode keeps the `note-editor` default (off) and stays available as a per-device setting, useful with a hardware keyboard (e.g. iPad).
  - Live preview works with touch: tapping places the caret in the underlying text, and task checkboxes toggle without raising the keyboard.
- **Secure credentials**: refresh token and key material live only in iOS Keychain / Android Keystore-protected storage, device-only and excluded from backups.
- **App lock**: an optional lock that requires device authentication (biometrics, with the device passcode as fallback) to open the app. When enabled, the app locks after a configurable time in the background, drops in-memory keys and plaintext, and hides note content in the app switcher.
- **Lifecycle**: sync on foreground, commit pending edits and flush the outbox when the app goes to the background.
- **Quick capture**: a "New Note" home-screen quick action / app shortcut and a `notebook://new` link open straight into a new note with the keyboard up.
- **Export** produces the `markdown-export` zip and hands it to the platform share sheet, then deletes the temporary file.
- CI builds unsigned iOS (simulator) and Android debug artifacts. Store submission is out of scope.

## Capabilities

### New Capabilities
- `mobile-experience`: Behavior specific to the native iOS/Android app: system back navigation, touch formatting toolbar, keyboard and safe-area handling, touch behavior of live preview, secure credential storage, app lock with device authentication, app-switcher privacy, lifecycle sync and flush rules, quick capture, share-sheet export, and backup exclusion.

### Modified Capabilities
- `app-shell` (introduced by `desktop-app-shell`): "Three-pane layout" applies only at 900 CSS px and wider; new requirements add the stacked layout below that, context-preserving layout switches, stacked navigation tied to history, and reachability of every action in the stacked layout. These apply to every client, not only mobile.
- `markdown-export` (introduced by `markdown-export`): folder output becomes desktop-only; mobile clients use the zip output.

## Impact

- `notebook-app`: iOS and Android targets (`cargo tauri ios init` / `android init` output under `crates/notebook-app/gen/`), mobile capability file, lifecycle hooks, and mobile-only commands.
- New in-repo Tauri plugin crate `tauri-plugin-notebook-mobile` (Swift + Kotlin): secure storage with access control, device authentication, app-switcher privacy, background task assertion, home-screen quick action, and share sheet. Plus `tauri-plugin-deep-link` for `notebook://new`.
- Frontend (`web/`): responsive layout and stack router, touch toolbar, visual-viewport and safe-area handling, mobile settings (app lock, auto-lock delay).
- Editor package: touch-friendly formatting commands (shared with desktop keyboard shortcuts).
- Build infrastructure: Xcode and Android SDK/NDK CI jobs; a short mobile build and signing guide.
- Depends on `desktop-app-shell` (archived first, since this modifies `app-shell`), `markdown-export` (zip output), `note-editor-component`, and `editor-live-preview` (touch behavior of live preview).
