//! Vault commands, events, the editor host, settings, and what reaches the
//! disk, on a local notebook with no server.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use yrs::updates::decoder::Decode;
use yrs::{GetString, Text, Transact, Update};

use crate::credentials::{self, CredentialStore, Entry};
use crate::dto::{SelectionDto, ViewDto};
use crate::error::Code;
use crate::events;
use crate::state::AppState;
use crate::state::tests::{Install, created, notes};

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
