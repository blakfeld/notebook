//! Vault commands, events, the editor host, settings, and what reaches the
//! disk, on a local notebook with no server.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use yrs::updates::decoder::Decode;
use yrs::{GetString, Text, Transact, Update};

use notebook_core::note::Note;

use crate::backup::tests::{StubPickers, backups_in};
use crate::credentials::{self, CredentialStore, Entry};
use crate::dto::{PhaseName, SelectionDto, ViewDto};
use crate::error::Code;
use crate::events;
use crate::state::tests::{Install, created, notes};
use crate::state::{AppState, now};
use crate::vault;

fn view(view: ViewDto) -> SelectionDto {
    SelectionDto::View(view)
}

fn tag(path: &str) -> SelectionDto {
    SelectionDto::Tag(path.into())
}

fn titles(app: &AppState, selection: SelectionDto, query: &str) -> Vec<String> {
    app.list_notes(selection, query)
        .unwrap()
        .into_iter()
        .map(|e| e.title)
        .collect()
}

fn named(install: &Install, name: &str) -> Vec<Value> {
    install
        .events
        .take()
        .into_iter()
        .filter(|(event, _)| *event == name)
        .map(|(_, payload)| payload)
        .collect()
}

/// Polls `check` until it holds, failing after ten seconds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// The editor's side of a session, as the frontend's Yjs replica.
struct Editor {
    doc: yrs::Doc,
    session: u64,
}

impl Editor {
    fn open(app: &AppState, note: &str) -> Self {
        let opened = app.editor_open(note).unwrap();
        let doc = yrs::Doc::with_options(yrs::Options {
            offset_kind: yrs::OffsetKind::Utf16,
            ..yrs::Options::with_client_id(yrs::ClientID::new(4242))
        });
        doc.get_or_insert_text("body");
        let editor = Self {
            doc,
            session: opened.session,
        };
        editor.apply(&opened.state);
        editor
    }

    fn apply(&self, update: &str) {
        let bytes = STANDARD.decode(update).unwrap();
        let update = Update::decode_v1(&bytes).unwrap();
        self.doc.transact_mut().apply_update(update).unwrap();
    }

    /// Types at the end and returns the update to push.
    fn type_text(&self, text: &str) -> String {
        let body = self.doc.get_or_insert_text("body");
        let mut txn = self.doc.transact_mut();
        let end = body.len(&txn);
        body.insert(&mut txn, end, text);
        STANDARD.encode(txn.encode_update_v1())
    }

    fn body(&self) -> String {
        let body = self.doc.get_or_insert_text("body");
        body.get_string(&self.doc.transact())
    }
}

// ---- vault commands ----

#[tokio::test]
async fn vault_commands_happy_paths() {
    let install = Install::new();
    let (app, _) = created(&install).await;

    let id = app.create_note(tag("work/acme")).unwrap();
    assert_eq!(titles(&app, tag("work"), ""), ["#work/acme"]);
    let tree = app.tag_tree().unwrap();
    assert_eq!(tree[0].name, "work");
    assert_eq!(tree[0].children[0].path, "work/acme");
    assert_eq!(tree[0].count, 1);

    app.set_pinned(&id, true).unwrap();
    assert_eq!(app.list_notes(view(ViewDto::Pinned), "").unwrap()[0].id, id);
    app.set_archived(&id, true).unwrap();
    assert_eq!(
        app.list_notes(view(ViewDto::Archive), "").unwrap()[0].id,
        id
    );
    assert!(app.tag_tree().unwrap().is_empty());
    app.set_archived(&id, false).unwrap();

    assert_eq!(app.rename_tag("work", "job").unwrap(), 1);
    assert_eq!(titles(&app, tag("job/acme"), ""), ["#job/acme"]);
    assert_eq!(
        app.rename_tag("job", "42").unwrap_err().code,
        Code::InvalidTagName
    );
}

#[tokio::test]
async fn vault_commands_need_an_open_notebook() {
    let install = Install::new();
    let (app, key) = created(&install).await;
    let id = app.create_note(notes()).unwrap();
    app.quit();
    install.creds.delete(Entry::VaultKey).unwrap();
    let app = install.launch();

    let not_open = |code: Code| assert_eq!(code, Code::NotOpen);
    not_open(app.tag_tree().unwrap_err().code);
    not_open(app.list_notes(notes(), "").unwrap_err().code);
    not_open(app.create_note(notes()).unwrap_err().code);
    not_open(app.set_pinned(&id, true).unwrap_err().code);
    not_open(app.set_archived(&id, true).unwrap_err().code);
    not_open(app.set_trashed(&id, true).unwrap_err().code);
    not_open(app.delete_permanently(&id).unwrap_err().code);
    not_open(app.rename_tag("a", "b").unwrap_err().code);
    not_open(app.editor_open(&id).unwrap_err().code);
    not_open(app.set_vim_mode(true).unwrap_err().code);
    app.open_with_recovery_key(&key).await.unwrap();
    assert_eq!(app.list_notes(notes(), "").unwrap().len(), 1);
}

#[tokio::test]
async fn permanent_delete_is_local_and_survives_a_relaunch() {
    // No server exists anywhere in this test.
    let install = Install::new();
    let (app, _) = created(&install).await;
    let id = app.create_note(notes()).unwrap();
    let kept = app.create_note(notes()).unwrap();
    assert_eq!(
        app.delete_permanently(&id).unwrap_err().code,
        Code::NotTrashed
    );

    app.set_trashed(&id, true).unwrap();
    assert_eq!(app.list_notes(view(ViewDto::Trash), "").unwrap().len(), 1);
    app.delete_permanently(&id).unwrap();
    assert!(app.list_notes(view(ViewDto::Trash), "").unwrap().is_empty());
    assert_eq!(app.set_pinned(&id, true).unwrap_err().code, Code::NotFound);
    assert_eq!(
        app.delete_permanently(&id).unwrap_err().code,
        Code::NotFound
    );
    app.quit();

    let app = install.launch();
    assert!(app.list_notes(view(ViewDto::Trash), "").unwrap().is_empty());
    let left: Vec<String> = app
        .list_notes(notes(), "")
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(left, [kept]);
}

// ---- events ----

#[tokio::test]
async fn changes_emit_vault_changed() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    install.events.take();

    let id = app.create_note(tag("ideas")).unwrap();
    eventually("the create event", || {
        named(&install, events::VAULT_CHANGED)
            .iter()
            .any(|p| p["changed"][0] == id.as_str() && p["tree_changed"] == true)
    })
    .await;

    app.set_trashed(&id, true).unwrap();
    app.delete_permanently(&id).unwrap();
    let mut seen = Vec::new();
    eventually("the delete event", || {
        seen.extend(named(&install, events::VAULT_CHANGED));
        seen.len() >= 2 && seen.iter().all(|p| p["changed"][0] == id.as_str())
    })
    .await;
    assert!(app.tag_tree().unwrap().is_empty());
}

// ---- editor host ----

#[tokio::test]
async fn editor_edits_are_stored_and_renames_reach_the_editor() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let id = app.create_note(notes()).unwrap();
    let editor = Editor::open(&app, &id);

    app.editor_push(editor.session, &editor.type_text("Hello #work"))
        .unwrap();
    assert_eq!(titles(&app, notes(), ""), ["Hello #work"]);

    // A rename rewrites the note under the open editor, which is sent what
    // it lacks.
    install.events.take();
    app.rename_tag("work", "job").unwrap();
    let mut updates = Vec::new();
    eventually("the editor update", || {
        updates.extend(named(&install, events::EDITOR_REMOTE_UPDATE));
        for update in updates.drain(..) {
            assert_eq!(update["session"], editor.session);
            editor.apply(update["update"].as_str().unwrap());
        }
        editor.body() == "Hello #job"
    })
    .await;

    // Stored: a relaunch reads it back.
    app.quit();
    let app = install.launch();
    assert_eq!(titles(&app, notes(), ""), ["Hello #job"]);

    // Trashed notes are read-only, even for an editor opened earlier.
    let editor = Editor::open(&app, &id);
    app.set_trashed(&id, true).unwrap();
    assert_eq!(
        app.editor_push(editor.session, &editor.type_text("!"))
            .unwrap_err()
            .code,
        Code::Trashed
    );
    app.editor_close(editor.session).unwrap();
    assert_eq!(
        app.editor_push(editor.session, &editor.type_text("!"))
            .unwrap_err()
            .code,
        Code::NotFound
    );
}

// ---- settings ----

#[tokio::test]
async fn settings_default_and_persist() {
    let install = Install::new();
    let app = install.launch();
    let defaults = app.settings().unwrap();
    assert!(!defaults.vim_mode);
    assert_eq!(defaults.theme, "system");
    assert!(!defaults.source_view && !defaults.focus_mode);
    assert!(defaults.collapsed_tags.is_empty());

    let (app, _) = created(&install).await;
    assert!(!app.settings().unwrap().vim_mode);
    app.set_vim_mode(true).unwrap();
    app.set_preference("appearance.theme", "dark").unwrap();
    app.set_preference("editor.focusMode", "true").unwrap();
    assert!(app.set_preference("appearance.theme", "blue").is_err());
    assert!(app.set_preference("editor.vimMode", "true").is_err());
    assert!(app.set_preference("other", "true").is_err());
    app.set_tag_expanded("Work", false).unwrap();
    app.set_tag_expanded("home", false).unwrap();
    app.set_tag_expanded("home", true).unwrap();
    app.quit();

    let app = install.launch();
    let settings = app.settings().unwrap();
    assert!(settings.vim_mode);
    assert_eq!(settings.theme, "dark");
    assert!(settings.focus_mode && !settings.source_view);
    assert_eq!(settings.collapsed_tags, ["work"]);
}

// ---- nothing secret on disk ----

fn files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn no_plaintext_or_keys_in_app_files() {
    const MARKER: &str = "xylophone-MARKER-8821";
    let install = Install::new();
    let (app, recovery) = created(&install).await;
    let id = app.create_note(notes()).unwrap();
    let editor = Editor::open(&app, &id);
    app.editor_push(
        editor.session,
        &editor.type_text(&format!("Groceries {MARKER}\n#{MARKER}")),
    )
    .unwrap();
    app.rename_tag(MARKER, &format!("{MARKER}2")).unwrap();
    let (_, key) = credentials::load_vault_key(&*install.creds)
        .unwrap()
        .unwrap();
    let key = *key.to_secret_bytes();
    let recovery_compact = recovery.replace('-', "");
    app.quit();

    let found = files(install.dir.path());
    assert!(!found.is_empty());
    for path in found {
        let bytes = std::fs::read(&path).unwrap();
        assert!(!contains(&bytes, MARKER.as_bytes()), "marker in {path:?}");
        assert!(!contains(&bytes, &key), "vault key in {path:?}");
        assert!(
            !contains(&bytes, recovery.as_bytes())
                && !contains(&bytes, recovery_compact.as_bytes()),
            "recovery key in {path:?}"
        );
    }
    // The vault key stayed in the credential store.
    assert!(install.has(Entry::VaultKey));
}

// ---- restore from backup ----

fn all_notes(app: &AppState) -> Vec<Note> {
    app.with_vault(|open| Ok(open.workspace.all_notes()))
        .unwrap()
}

fn new_note(app: &AppState, body: &str) -> String {
    let id = app.create_note(notes()).unwrap();
    let note = crate::dto::parse_note_id(&id).unwrap();
    app.with_vault(|open| Ok(open.workspace.set_body(note, body, now())?))
        .unwrap();
    id
}

fn sorted_ids(app: &AppState) -> Vec<String> {
    let mut ids: Vec<String> = all_notes(app).iter().map(|n| n.id().to_string()).collect();
    ids.sort();
    ids
}

/// A notebook with active, archived, pinned, and trashed notes, backed up
/// into a new folder. Returns the app, its recovery key, the folder, and
/// the backup file.
async fn backed_up(install: &Install) -> (AppState, String, tempfile::TempDir, PathBuf) {
    let (app, recovery) = created(install).await;
    new_note(&app, "Active");
    let archived = new_note(&app, "Archived");
    app.set_archived(&archived, true).unwrap();
    let pinned = new_note(&app, "Pinned");
    app.set_pinned(&pinned, true).unwrap();
    let trashed = new_note(&app, "Trashed");
    app.set_trashed(&trashed, true).unwrap();

    let folder = tempfile::tempdir().unwrap();
    app.choose_backup_folder(&*StubPickers::folder(folder.path()))
        .await
        .unwrap()
        .unwrap();
    let file = folder.path().join(&backups_in(folder.path())[0]);
    (app, recovery, folder, file)
}

/// A copy of `file` in `dir` with one byte of its payload flipped.
fn damaged_copy(file: &Path, dir: &Path) -> PathBuf {
    let mut bytes = std::fs::read(file).unwrap();
    let at = bytes.len() - 20;
    bytes[at] ^= 1;
    let copy = dir.join("damaged.nbbackup");
    std::fs::write(&copy, bytes).unwrap();
    copy
}

#[tokio::test]
async fn full_round_trip_on_an_empty_device() {
    let (app, recovery, _folder, file) = backed_up(&Install::new()).await;
    let expected = all_notes(&app);
    assert_eq!(expected.len(), 4);

    let device = Install::new();
    let fresh = device.launch();
    let picked = fresh
        .pick_backup(&*StubPickers::file(&file))
        .await
        .unwrap()
        .unwrap();
    assert!(!picked.same_vault);
    assert_eq!(
        picked.file_name,
        file.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(
        fresh.restore_backup(None, false).await.unwrap_err().code,
        Code::RecoveryKeyRequired
    );
    let phase = fresh.restore_backup(Some(&recovery), false).await.unwrap();
    assert_eq!(phase.phase, PhaseName::Open);
    assert_eq!(all_notes(&fresh), expected);
    assert!(device.has(Entry::VaultKey));
    fresh.quit();
    drop(fresh);

    // Later launches open it with no prompt.
    let relaunched = device.launch();
    assert_eq!(relaunched.phase().phase, PhaseName::Open);
    assert_eq!(all_notes(&relaunched), expected);
}

#[tokio::test]
async fn cancelled_picker_changes_nothing() {
    let device = Install::new();
    let fresh = device.launch();
    assert_eq!(
        fresh.pick_backup(&StubPickers::default()).await.unwrap(),
        None
    );
    assert_eq!(
        fresh.restore_backup(None, false).await.unwrap_err().code,
        Code::NotOpen
    );
    assert_eq!(fresh.phase().phase, PhaseName::Setup);
    assert!(device.files().is_empty());
}

#[tokio::test]
async fn a_cancelled_restore_forgets_the_pick() {
    let install = Install::new();
    let (app, _, _folder, file) = backed_up(&install).await;
    let before = all_notes(&app);
    app.pick_backup(&*StubPickers::file(&file))
        .await
        .unwrap()
        .unwrap();
    new_note(&app, "Since the backup");
    app.cancel_restore();
    assert_eq!(
        app.restore_backup(None, true).await.unwrap_err().code,
        Code::NotOpen
    );
    assert_eq!(all_notes(&app).len(), before.len() + 1);
}

/// A restore from the welcome screen never deletes an earlier
/// `before-restore`, which may be the only copy of a notebook whose restore
/// was cut short.
#[tokio::test]
async fn a_welcome_restore_keeps_an_earlier_before_restore() {
    let (_app, recovery, _folder, file) = backed_up(&Install::new()).await;
    let device = Install::new();
    let db = vault::database_path(device.dir.path());
    let before = vault::with_suffix(&db, ".before-restore");
    std::fs::write(&before, b"the only copy").unwrap();

    let fresh = device.launch();
    fresh
        .pick_backup(&*StubPickers::file(&file))
        .await
        .unwrap()
        .unwrap();
    fresh.restore_backup(Some(&recovery), false).await.unwrap();
    let kept: Vec<_> = device
        .files()
        .into_iter()
        .filter(|name| name.starts_with("vault.sqlite3.before-restore.legacy-"))
        .collect();
    assert_eq!(kept.len(), 1, "{:?}", device.files());
    assert_eq!(
        std::fs::read(device.dir.path().join(&kept[0])).unwrap(),
        b"the only copy"
    );
}

#[tokio::test]
async fn restore_over_existing_notes() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let b = new_note(&app, "B prime");
    let c = new_note(&app, "C");
    let folder = tempfile::tempdir().unwrap();
    app.choose_backup_folder(&*StubPickers::folder(folder.path()))
        .await
        .unwrap()
        .unwrap();
    let file = folder.path().join(&backups_in(folder.path())[0]);

    let b_id = crate::dto::parse_note_id(&b).unwrap();
    app.with_vault(|open| Ok(open.workspace.set_body(b_id, "B", now())?))
        .unwrap();
    app.set_trashed(&c, true).unwrap();
    app.delete_permanently(&c).unwrap();
    new_note(&app, "A");

    let picked = app
        .pick_backup(&*StubPickers::file(&file))
        .await
        .unwrap()
        .unwrap();
    // Same-vault restore needs no key.
    assert!(picked.same_vault);
    assert_eq!(
        app.restore_backup(None, false).await.unwrap().phase,
        PhaseName::Open
    );
    let mut expected = vec![b, c];
    expected.sort();
    assert_eq!(sorted_ids(&app), expected);
    let mut bodies: Vec<String> = all_notes(&app)
        .iter()
        .map(|n| n.body().to_owned())
        .collect();
    bodies.sort();
    assert_eq!(bodies, ["B prime", "C"]);

    // A safety backup first, and the old database kept aside.
    assert_eq!(backups_in(folder.path()).len(), 2);
    let db = vault::database_path(install.dir.path());
    assert!(vault::with_suffix(&db, ".before-restore").exists());
    assert!(!vault::with_suffix(&db, ".restoring").exists());
    // Backup settings carry over.
    assert_eq!(
        app.backup_status().unwrap().folder,
        Some(folder.path().display().to_string())
    );
    app.quit();
    drop(app);

    let relaunched = install.launch();
    assert_eq!(relaunched.phase().phase, PhaseName::Open);
    assert_eq!(sorted_ids(&relaunched), expected);
}

#[tokio::test]
async fn wrong_recovery_key() {
    let (_app, _, _folder, file) = backed_up(&Install::new()).await;
    let (_other, other_key) = created(&Install::new()).await;

    let device = Install::new();
    let fresh = device.launch();
    fresh
        .pick_backup(&*StubPickers::file(&file))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fresh
            .restore_backup(Some("not a recovery key"), false)
            .await
            .unwrap_err()
            .code,
        Code::RecoveryKeyMalformed
    );
    assert_eq!(
        fresh
            .restore_backup(Some(&other_key), false)
            .await
            .unwrap_err()
            .code,
        Code::RecoveryKeyWrong
    );
    assert_eq!(fresh.phase().phase, PhaseName::Setup);
    assert!(device.files().is_empty());
    assert!(!device.has(Entry::VaultKey));
}

#[tokio::test]
async fn damaged_backup() {
    let install = Install::new();
    let (app, recovery, folder, file) = backed_up(&install).await;
    let before = all_notes(&app);
    let damaged = damaged_copy(&file, install.dir.path());

    let picked = app
        .pick_backup(&*StubPickers::file(&damaged))
        .await
        .unwrap()
        .unwrap();
    assert!(picked.same_vault);
    assert_eq!(
        app.restore_backup(None, false).await.unwrap_err().code,
        Code::BackupDamaged
    );
    assert_eq!(all_notes(&app), before);
    // Checked before the safety backup.
    assert_eq!(backups_in(folder.path()).len(), 1);
    let db = vault::database_path(install.dir.path());
    assert!(!vault::with_suffix(&db, ".before-restore").exists());

    // On a new device too.
    let device = Install::new();
    let fresh = device.launch();
    fresh
        .pick_backup(&*StubPickers::file(&damaged))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fresh
            .restore_backup(Some(&recovery), false)
            .await
            .unwrap_err()
            .code,
        Code::BackupDamaged
    );
    assert!(device.files().is_empty());
}

#[tokio::test]
async fn files_that_are_not_backups_are_refused_when_picked() {
    let install = Install::new();
    let fresh = install.launch();
    let dir = tempfile::tempdir().unwrap();
    let markdown = dir.path().join("notes.md");
    std::fs::write(&markdown, "# Notes\n").unwrap();
    assert_eq!(
        fresh
            .pick_backup(&*StubPickers::file(&markdown))
            .await
            .unwrap_err()
            .code,
        Code::BackupNotABackup
    );

    let (_app, _, _folder, file) = backed_up(&Install::new()).await;
    let mut bytes = std::fs::read(&file).unwrap();
    bytes[8] = notebook_core::backup::FORMAT_VERSION + 1;
    let newer = dir.path().join("newer.nbbackup");
    std::fs::write(&newer, bytes).unwrap();
    assert_eq!(
        fresh
            .pick_backup(&*StubPickers::file(&newer))
            .await
            .unwrap_err()
            .code,
        Code::BackupTooNew
    );
}

#[tokio::test]
async fn safety_backup_failure_needs_confirmation() {
    let install = Install::new();
    let (app, _, folder, file) = backed_up(&install).await;
    let before = all_notes(&app);
    let copy = install.dir.path().join("copy.nbbackup");
    std::fs::copy(&file, &copy).unwrap();
    let folder_path = folder.path().to_owned();
    drop(folder);

    app.pick_backup(&*StubPickers::file(&copy))
        .await
        .unwrap()
        .unwrap();
    new_note(&app, "Since the backup");
    let err = app.restore_backup(None, false).await.unwrap_err();
    assert_eq!(err.code, Code::SafetyBackupFailed);
    assert_eq!(err.message, "the backup folder is unavailable");
    assert_eq!(all_notes(&app).len(), before.len() + 1);
    assert!(!folder_path.exists());

    // Confirmed again: restores without it.
    app.restore_backup(None, true).await.unwrap();
    assert_eq!(all_notes(&app), before);
}

#[tokio::test]
async fn keychain_failure_rolls_the_restore_back() {
    let (_source, recovery, _folder, file) = backed_up(&Install::new()).await;
    let install = Install::new();
    let (app, _) = created(&install).await;
    new_note(&app, "Mine");
    let before = all_notes(&app);
    let files_before = install.files();

    app.pick_backup(&*StubPickers::file(&file))
        .await
        .unwrap()
        .unwrap();
    install.creds.broken.store(true, Ordering::SeqCst);
    let err = app
        .restore_backup(Some(&recovery), false)
        .await
        .unwrap_err();
    assert_eq!(err.code, Code::Internal);
    assert_eq!(app.phase().phase, PhaseName::Open);
    assert_eq!(all_notes(&app), before);
    assert_eq!(install.files(), files_before);
    install.creds.broken.store(false, Ordering::SeqCst);
    app.quit();
    drop(app);

    let relaunched = install.launch();
    assert_eq!(relaunched.phase().phase, PhaseName::Open);
    assert_eq!(all_notes(&relaunched), before);
}

/// Collects formatted log output.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every log line of this test process. Installed once as the global
/// subscriber: a per-thread one races with call sites that other tests'
/// threads cache as uninteresting.
fn captured_logs() -> &'static Captured {
    static CAPTURED: std::sync::OnceLock<Captured> = std::sync::OnceLock::new();
    CAPTURED.get_or_init(|| {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        captured
    })
}

#[tokio::test]
async fn restore_logs_carry_no_recovery_key() {
    let captured = captured_logs();
    let (_app, _, _folder, file) = backed_up(&Install::new()).await;
    let (_other, other_key) = created(&Install::new()).await;

    let device = Install::new();
    let fresh = device.launch();
    fresh
        .pick_backup(&*StubPickers::file(&file))
        .await
        .unwrap()
        .unwrap();
    for typed in [other_key.clone(), other_key.replace('-', "")] {
        assert_eq!(
            fresh
                .restore_backup(Some(&typed), false)
                .await
                .unwrap_err()
                .code,
            Code::RecoveryKeyWrong
        );
    }

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("restore failed"), "{logs}");
    assert!(!logs.contains(&other_key));
    assert!(!logs.contains(&other_key.replace('-', "")));
}
