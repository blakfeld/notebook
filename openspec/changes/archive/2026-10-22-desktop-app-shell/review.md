# Review notes

## Manual checks (2026-10-22)

The user confirmed these as complete on macOS:

- 4.1: `cargo test -p notebook-app -- --ignored keyring_round_trip` (the entry appears under service `com.blakfeld.notebook`).
- 8.5: design review, and each screen checked with `cargo tauri dev` in light and dark appearance.
- 9.2: against a local server: register, sign in, quit and relaunch without a prompt, edit offline, relaunch offline, edits sync once back online, lock and unlock offline, sign out.

Linux and Windows were not checked.

## Reviews

- Rust code review and security review. Fixed findings:
  - sync never resumed after a relaunch or an offline unlock;
  - the old passphrase still unlocked a device offline after a change elsewhere (now fails closed on `sign_in_required`);
  - tag names were stored in plaintext (now sealed with the vault key);
  - the rotated refresh token was saved only after a network call;
  - keychain entries were stranded on a mid-write fallback;
  - several smaller issues (see design.md).
- Frontend code review and security check. Fixed findings:
  - lock, sign-out and trash lost unflushed editor edits and leaked the editor session;
  - an open note kept stale flags when it changed on another device;
  - creating a note from Pinned left it unlisted;
  - a remote update could slip past a subscription race;
  - vim's module state kept note text after lock.
