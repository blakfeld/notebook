# Tasks

## 1. Mobile projects and build

- [ ] 1.1 Run `cargo tauri ios init` and `cargo tauri android init` for `crates/notebook-app`, commit the output under `gen/apple` and `gen/android`, set Android `minSdk` 30, and switch any HTTP client feature in the app/core dependency tree to `rustls`; verify `cargo tauri ios build --debug --target aarch64-sim` and `cargo tauri android build --debug --apk` succeed locally (or report exactly which toolchain is missing)
- [ ] 1.2 Exclude app data from backups: `isExcludedFromBackup` on the app data directory at iOS startup, and `allowBackup="false"`, `fullBackupContent="false"`, and `dataExtractionRules` excluding all domains on Android; verify with a unit-tested helper for the iOS path flag and a manifest check, and on device that `adb shell bmgr backupnow` reports the package as not eligible
- [ ] 1.3 Add `ios` (macos-latest, simulator build) and `android` (ubuntu-latest, Java 17, SDK/NDK, debug APK) jobs to `.github/workflows/ci.yml`, both reusing the `web-dist` artifact; verify both jobs pass on a pushed branch
- [ ] 1.4 Write `docs/mobile.md` (prerequisites, `cargo tauri ios dev` / `android dev` on simulator and emulator, where Info.plist and manifest edits live, how release signing would be configured) and link it from the README; verify the documented dev commands launch the app on a simulator and an emulator

## 2. Mobile plugin and permissions

- [ ] 2.1 Create the workspace crate `crates/tauri-plugin-notebook-mobile` with `tauri plugin new` (Swift package and Kotlin library, no JS API package), register it in `notebook-app` under `#[cfg(mobile)]` only, and add `tauri-plugin-deep-link`; verify desktop `cargo test -p notebook-app` is unchanged and both mobile builds still succeed
- [ ] 2.2 Split capabilities: add `"platforms"` for desktop to `capabilities/default.json` and create `capabilities/mobile.json` granting only `core:default`, the plugin's `share` and `privacy` permissions, and `deep-link:default`; verify an IPC test on the mock runtime that a desktop build rejects the plugin commands, and that the CSP is unchanged
- [ ] 2.3 Add platform declarations: iOS `NSFaceIDUsageDescription`, `CFBundleURLTypes` for `notebook`, `UIApplicationShortcutItems` for New Note; Android `USE_BIOMETRIC`, the `notebook://new` intent filter, and `shortcuts.xml`; verify the built Info.plist and merged manifest contain exactly these additions and no other new permissions

## 3. Responsive layout and stacked navigation

- [ ] 3.1 Implement the navigation store (`view/tag`, `openNoteId`) and a width signal at 900 CSS px that picks three-pane or stacked rendering, keeping one mounted editor instance across switches; verify Vitest tests for pane selection at 390, 899, 900, and 1180 px and for state kept across a switch (spec: Responsive layout, Layout changes keep context)
- [ ] 3.2 Implement History-API stacking (push on each step down, `popstate` moves up, entries rebuilt with `replaceState` when the layout switches, no entries in three-pane mode), the visible back control, and the Notes-view start screen; verify Vitest tests that simulate push/pop sequences and a layout switch mid-stack (spec: Stacked navigation, Stacked navigation follows browser history)
- [ ] 3.3 Make every app-shell action reachable in the stacked layout: search and sync indicator in the note-list header, note actions in the editor menu and on long press in the list, tag rename/move on long press in the sidebar, settings from the sidebar; verify component tests for each long-press menu and a browser check at 390 px that each action works
- [ ] 3.4 Enable `allowsBackForwardNavigationGestures` from the plugin on iOS and confirm Android back follows WebView history and backgrounds the app at the sidebar; verify on an iOS simulator and Android emulator that edge-swipe/back moves up one level with scroll position kept, and if the iOS gesture is unreliable implement the JS edge-swipe fallback and verify it instead (spec: System back navigation)

## 4. Secure credential storage

- [ ] 4.1 Add a per-entry protection level (`Standard` / `DeviceAuth`) to desktop-app-shell's `CredentialStore` (desktop `keyring` and in-memory implementations ignore it) and a mobile implementation over the plugin; verify existing desktop credential tests pass unchanged and in-memory-store tests cover the level being recorded per entry
- [ ] 4.2 Implement the iOS secure store in the plugin (`WhenUnlockedThisDeviceOnly` for Standard, `WhenPasscodeSetThisDeviceOnly` + `.userPresence` for DeviceAuth) and the device-auth availability check; verify on a simulator that set/get/delete round-trips, a DeviceAuth read prompts, and cancelling returns a distinct "cancelled" error
- [ ] 4.3 Implement the Android secure store (AES-256-GCM AndroidKeyStore key, `setUnlockedDeviceRequired` for Standard, per-use `BIOMETRIC_STRONG | DEVICE_CREDENTIAL` auth with `BiometricPrompt` + `CryptoObject` for DeviceAuth, ciphertext in `noBackupFilesDir`) and the availability check; verify the same round-trip, prompt, and cancel cases on an emulator with a PIN set
- [ ] 4.4 Map "item missing" / `KeyPermanentlyInvalidatedException` on `auto-unlock` to an "entry lost" outcome that deletes the entry, turns app lock off, and shows passphrase unlock while keeping the outbox; verify an in-memory-store test and, on device, that removing the passcode then relaunching shows passphrase unlock, which works offline via `local-unlock`, and pending edits push afterwards (spec: Fallback to passphrase unlock)
- [ ] 4.5 Confirm no secret reaches files, preferences, or logs: verify a test that captures app logs through sign-in, lock, and unlock with marker token and key values and asserts the markers are absent, plus a grep of the app sandbox (simulator) and `adb run-as` data dir for the markers (spec: Secure credential storage on mobile)

## 5. App lock and lifecycle

- [ ] 5.1 Add the `auto_lock()` phase transition (drop the `Workspace`, stop sync, keep `auto-unlock`) and `unlock_with_device()` command alongside desktop-app-shell's explicit lock (which still deletes `auto-unlock`), and a lock screen offering device authentication only when a DeviceAuth `auto-unlock` entry exists, plus the passphrase; verify mock-runtime tests that key-dependent commands fail while auto-locked, device unlock rebuilds the workspace, explicit lock removes the device option, and launch with app lock on lands on the lock screen (spec: Stored keys gated by device authentication, Explicit lock on mobile)
- [ ] 5.2 Add app lock settings (enabled, delay with default 5 minutes) in device-local settings, gating "enable" on device-auth availability and a successful prompt, and rewriting only `auto-unlock` at the new protection level on toggle; verify in-memory-store tests for the toggle flow, and on device that the setting is disabled with the passcode message when no passcode is set (spec: App lock)
- [ ] 5.3 Implement `app_backgrounded` / `app_foregrounded` driven by `visibilitychange`: `NoteEditorHandle.flush()`, outbox persist and push attempt inside an iOS background task, `backgrounded_at` from the wall clock, auto-lock on "immediately", and on foreground auto-lock when the delay is exceeded or the clock went backwards, otherwise pull and resume live mode; verify unit tests for the delay decision (within, beyond, negative elapsed) and an integration test with the sync engine's test transport that a foreground triggers a pull (spec: Auto-lock after time in background, Sync on foreground, Flush on background)
- [ ] 5.4 Implement app-switcher privacy in the plugin (iOS overlay on `willResignActive`, Android `FLAG_SECURE`), enabled only while app lock is on; verify on simulator and emulator that the switcher shows the placeholder with lock on and the note with lock off (spec: App switcher privacy)
- [ ] 5.5 Verify lifecycle on devices: type then switch apps immediately with the network off, force-stop/terminate the app, relaunch, and confirm the edit is present and pushed when the network returns; edit on desktop while the phone is backgrounded and confirm it appears on foreground; cold launch with app lock on shows nothing before authentication

## 6. Touch editing

- [ ] 6.1 Implement formatting commands in `web/src/editor/formatting.ts` (`toggleWrap` for bold/italic, `cycleHeading`, `toggleBullet`, `insertTag` using the core tag rules for multi-word selections, `makeLink`) and extend `editor-live-preview`'s toggle-task command to add `- [ ] ` on lines with no list marker; verify Vitest tests on `EditorState` for every scenario in the inline, line, and tag formatting requirements, and that the existing toggle-task tests still pass
- [ ] 6.2 Build the keyboard toolbar component (heading, bold, italic, list, task, tag, link, undo, redo, dismiss) shown when the editor is focused, the pointer is coarse, and the keyboard is visible, positioned from `visualViewport`, with `pointerdown` preventing focus loss and undo/redo through `NoteEditorHandle` disabled when unavailable; verify component tests for visibility rules and focus retention, and on device that the toolbar sits on the keyboard and hides with a hardware keyboard (spec: Formatting toolbar, Toolbar undo and redo)
- [ ] 6.3 Add viewport and safe-area handling: `viewport-fit=cover` and `interactive-widget=resizes-content`, `max(env(...), var(--sai-...))` padding with insets published by the plugin on Android, bottom padding and `scrollIntoView` on viewport resize; verify on notched simulators/emulators in portrait and landscape that no control sits under cutouts and the caret stays above the toolbar at the end of a long note (spec: Caret stays visible, Safe areas)
- [ ] 6.4 Make live-preview checkboxes handle `pointerdown` themselves and ignore editor focus, and check tap-to-caret and selection handles across hidden syntax; verify a DOM test that a checkbox tap toggles `- [ ]` without focusing the editor, and on device the tap-on-bold and selection scenarios (spec: Live preview on touch)
- [ ] 6.5 Expose the vim toggle in mobile settings (per-device, default off) and confirm toolbar actions apply with vim on; verify a Vitest test of the default and a device check with a hardware keyboard on iPad simulator, plus a device check of autocorrect, dictation, and Gboard composition with live sync running (spec: Vim mode on mobile)

## 7. Quick capture

- [ ] 7.1 Handle `notebook://new` through the deep-link plugin, accepting only host `new` and ignoring path, query, and fragment, and translate the iOS quick action and Android shortcut into the same `quick-capture` event; verify unit tests of the URL filter (`notebook://new`, `notebook://new?text=x`, `notebook://other`) (spec: Quick capture is content-free)
- [ ] 7.2 Queue quick capture until unlocked, then select Notes, create an empty note with `Workspace::create`, navigate to the editor, focus it, and purge it with `Workspace::purge` if the user leaves while it is still empty; verify frontend tests for the queue-through-lock flow and the empty-note cleanup
- [ ] 7.3 Verify on device: New Note from the home screen and from Android's shortcut opens a new note with the keyboard up, including after a device-auth prompt with app lock on; if the iOS keyboard cannot be raised reliably, stop and report rather than narrowing the spec (spec: Quick capture)

## 8. Share-sheet export

- [ ] 8.1 On mobile, have "Export all notes" build the `markdown-export` zip, write it to the temp `exports/` folder (iOS complete file protection, Android `FileProvider` scoped to that folder), call the plugin share, delete the file on completion, and empty `exports/` at launch; verify a Rust test of the write/cleanup helpers with a temp dir, and on device that Save to Files yields the named zip and a cancelled share leaves `exports/` empty (spec: Export through the share sheet)

## 9. Integration checks

- [ ] 9.1 Run `scripts/check.sh` and the web checks (`npm run lint`, `format:check`, `check`, `test`, `build`) and verify they pass with desktop behavior unchanged
- [ ] 9.2 End-to-end on one iOS simulator and one Android emulator against a local server: sign in, offline edit, background/foreground, auto-lock and unlock, quick capture, export, sign out; and confirm the same note edited on desktop, browser, and phone converges
- [ ] 9.3 Once `desktop-app-shell` and `markdown-export` are archived, diff the MODIFIED blocks in `specs/app-shell/spec.md` (Three-pane layout) and `specs/markdown-export/spec.md` (Folder output) against the archived main specs and carry over any wording changes made since; verify `openspec validate mobile-app --strict` passes with no "archive would refuse" notices

## Workflow follow-up

- Review with the code-reviewer and security-analyst agents (secure storage, app lock, deep link, FileProvider scope, capabilities) before archiving.
- Archive with `/opsx:archive` only after `desktop-app-shell` and `markdown-export` are archived (this change MODIFIES their capabilities), then mark `mobile-app` `[x]` in `TODO.md`.
