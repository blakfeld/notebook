//! Vault commands, events, the editor host, settings, and what reaches the
//! disk, against a real server.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use yrs::updates::decoder::Decode;
use yrs::{GetString, Text, Transact, Update};

use crate::credentials::{self, Entry};
use crate::dto::{SelectionDto, SyncStateDto, ViewDto};
use crate::error::Code;
use crate::events;
use crate::state::AppState;
use crate::state::tests::{Install, PASSPHRASE, TestServer, eventually, notes, registered};

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

// ---- 5.1 vault commands ----

#[tokio::test]
async fn vault_commands_happy_paths() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;

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

    assert_eq!(
        app.delete_permanently(&id).await.unwrap_err().code,
        Code::NotTrashed
    );
    app.set_trashed(&id, true).unwrap();
    assert_eq!(app.list_notes(view(ViewDto::Trash), "").unwrap().len(), 1);
    eventually("sync", || app.pending_count().unwrap() == 0).await;
    app.delete_permanently(&id).await.unwrap();
    assert!(app.list_notes(view(ViewDto::Trash), "").unwrap().is_empty());
    assert_eq!(app.set_pinned(&id, true).unwrap_err().code, Code::NotFound);
}

#[tokio::test]
async fn vault_commands_need_an_unlocked_vault() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    let id = app.create_note(notes()).unwrap();
    app.lock().await.unwrap();

    let locked = |code: Code| assert_eq!(code, Code::Locked);
    locked(app.tag_tree().unwrap_err().code);
    locked(app.list_notes(notes(), "").unwrap_err().code);
    locked(app.create_note(notes()).unwrap_err().code);
    locked(app.set_pinned(&id, true).unwrap_err().code);
    locked(app.set_archived(&id, true).unwrap_err().code);
    locked(app.set_trashed(&id, true).unwrap_err().code);
    locked(app.delete_permanently(&id).await.unwrap_err().code);
    locked(app.rename_tag("a", "b").unwrap_err().code);
    locked(app.editor_open(&id).unwrap_err().code);
    locked(app.sync_status().unwrap_err().code);
}

#[tokio::test]
async fn delete_permanently_offline_keeps_the_note() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    let id = app.create_note(notes()).unwrap();
    app.set_trashed(&id, true).unwrap();
    eventually("sync", || app.pending_count().unwrap() == 0).await;
    server.stop();
    assert_eq!(
        app.delete_permanently(&id).await.unwrap_err().code,
        Code::Offline
    );
    assert_eq!(app.list_notes(view(ViewDto::Trash), "").unwrap().len(), 1);
}

// ---- 5.2 events ----

#[tokio::test]
async fn local_and_remote_changes_emit_vault_changed() {
    let server = TestServer::start().await;
    let a_install = Install::new();
    let a = registered(&server, &a_install, "alice").await;
    let b_install = Install::new();
    let b = b_install.launch();
    b.sign_in(&server.url, "alice", PASSPHRASE).await.unwrap();
    eventually("both synced", || {
        a.sync_status().unwrap().state == SyncStateDto::Synced
            && b.sync_status().unwrap().state == SyncStateDto::Synced
    })
    .await;
    a_install.events.take();
    b_install.events.take();

    let id = a.create_note(tag("ideas")).unwrap();
    eventually("the local event", || {
        named(&a_install, events::VAULT_CHANGED)
            .iter()
            .any(|p| p["changed"][0] == id.as_str() && p["tree_changed"] == true)
    })
    .await;
    let mut seen = Vec::new();
    eventually("the remote event", || {
        seen.extend(named(&b_install, events::VAULT_CHANGED));
        seen.iter().any(|p| p["changed"][0] == id.as_str())
    })
    .await;
    assert_eq!(titles(&b, tag("ideas"), ""), ["#ideas"]);
}

#[tokio::test]
async fn offline_status_counts_pending_changes() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    eventually("synced", || {
        app.sync_status().unwrap().state == SyncStateDto::Synced
    })
    .await;
    server.stop();
    app.create_note(notes()).unwrap();
    app.create_note(notes()).unwrap();
    eventually("offline with 2 pending", || {
        let status = app.sync_status().unwrap();
        status.state == SyncStateDto::Offline && status.pending == 2
    })
    .await;
    let statuses = named(&install, events::SYNC_STATUS);
    assert!(
        statuses
            .iter()
            .any(|s| s["state"] == "offline" && s["pending"] == 2),
        "{statuses:?}"
    );

    server.restart().await;
    eventually("back to synced", || {
        let status = app.sync_status().unwrap();
        status.state == SyncStateDto::Synced && status.pending == 0
    })
    .await;
}

// ---- 5.3 editor host ----

#[tokio::test]
async fn editor_updates_flow_both_ways() {
    let server = TestServer::start().await;
    let a_install = Install::new();
    let a = registered(&server, &a_install, "alice").await;
    let b_install = Install::new();
    let b = b_install.launch();
    b.sign_in(&server.url, "alice", PASSPHRASE).await.unwrap();

    let id = a.create_note(notes()).unwrap();
    let editor = Editor::open(&a, &id);
    eventually("a synced", || a.pending_count().unwrap() == 0).await;

    // A local edit is stored and queued.
    a.editor_push(editor.session, &editor.type_text("Hello"))
        .unwrap();
    assert!(a.pending_count().unwrap() > 0);
    assert_eq!(titles(&a, notes(), ""), ["Hello"]);
    eventually("b to see it", || titles(&b, notes(), "") == ["Hello"]).await;

    // A remote edit reaches the open editor.
    a_install.events.take();
    let other = Editor::open(&b, &id);
    b.editor_push(other.session, &other.type_text(" world"))
        .unwrap();
    let mut updates = Vec::new();
    eventually("the remote update", || {
        updates.extend(named(&a_install, events::EDITOR_REMOTE_UPDATE));
        for update in updates.drain(..) {
            assert_eq!(update["session"], editor.session);
            editor.apply(update["update"].as_str().unwrap());
        }
        editor.body() == "Hello world"
    })
    .await;

    // Trashed notes are read-only, even for an editor opened earlier.
    a.set_trashed(&id, true).unwrap();
    assert_eq!(
        a.editor_push(editor.session, &editor.type_text("!"))
            .unwrap_err()
            .code,
        Code::Trashed
    );
    a.editor_close(editor.session).unwrap();
    assert_eq!(
        a.editor_push(editor.session, &editor.type_text("!"))
            .unwrap_err()
            .code,
        Code::NotFound
    );
}

// ---- 5.4 settings ----

#[tokio::test]
async fn settings_default_and_persist() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = install.launch();
    let defaults = app.settings().unwrap();
    assert!(!defaults.vim_mode);
    assert_eq!(defaults.theme, "system");
    assert!(!defaults.source_view && !defaults.focus_mode);
    assert!(defaults.collapsed_tags.is_empty());

    let app = registered(&server, &install, "alice").await;
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
    app.quit().await;

    let app = install.launch();
    let settings = app.settings().unwrap();
    assert!(settings.vim_mode);
    assert_eq!(settings.theme, "dark");
    assert!(settings.focus_mode && !settings.source_view);
    assert_eq!(settings.collapsed_tags, ["work"]);

    // Readable and writable while locked, too.
    app.lock().await.unwrap();
    assert!(app.settings().unwrap().vim_mode);
    app.set_vim_mode(false).unwrap();
    app.unlock(PASSPHRASE).await.unwrap();
    assert!(!app.settings().unwrap().vim_mode);
}

// ---- 5.5 nothing secret on disk ----

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
async fn no_plaintext_or_tokens_in_app_files() {
    const MARKER: &str = "xylophone-MARKER-8821";
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    let id = app.create_note(notes()).unwrap();
    let editor = Editor::open(&app, &id);
    app.editor_push(
        editor.session,
        &editor.type_text(&format!("Groceries {MARKER}\n#{MARKER}")),
    )
    .unwrap();
    app.rename_tag(MARKER, &format!("{MARKER}2")).unwrap();
    eventually("sync", || app.pending_count().unwrap() == 0).await;
    let token = credentials::load_session(&*install.creds)
        .unwrap()
        .unwrap()
        .refresh_token
        .0;
    app.lock().await.unwrap();

    let found = files(install.dir.path());
    assert!(!found.is_empty());
    for path in found {
        let bytes = std::fs::read(&path).unwrap();
        assert!(!contains(&bytes, MARKER.as_bytes()), "marker in {path:?}");
        assert!(
            !contains(&bytes, token.as_bytes()),
            "refresh token in {path:?}"
        );
    }
    // The secrets stayed in the credential store.
    assert!(install.has(Entry::Session));
}

// ---- 5.6 server restore ----

fn copy_db(from: &Path, to: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut src = from.as_os_str().to_owned();
        src.push(suffix);
        let mut dst = to.as_os_str().to_owned();
        dst.push(suffix);
        match std::fs::copy(&src, &dst) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(&dst);
            }
            Err(err) => panic!("{err}"),
        }
    }
}

#[tokio::test]
async fn server_restore_rebuilds_storage_and_keeps_local_edits() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    let first = app.create_note(notes()).unwrap();
    let editor = Editor::open(&app, &first);
    app.editor_push(editor.session, &editor.type_text("First"))
        .unwrap();
    eventually("synced", || {
        app.sync_status().unwrap().state == SyncStateDto::Synced
            && app.pending_count().unwrap() == 0
    })
    .await;

    // Back up the server, then keep working.
    let backup = tempfile::tempdir().unwrap();
    let saved = backup.path().join("server.db");
    server.stop();
    copy_db(&server.db_path(), &saved);
    server.restart().await;
    app.editor_push(
        editor.session,
        &editor.type_text(" edited after the backup"),
    )
    .unwrap();
    let second = app.create_note(tag("later")).unwrap();
    eventually("synced again", || {
        app.sync_status().unwrap().state == SyncStateDto::Synced
            && app.pending_count().unwrap() == 0
    })
    .await;

    // The server loses everything after the backup.
    install.events.take();
    server.stop();
    copy_db(&saved, &server.db_path());
    server.restart().await;

    let fresh_install = Install::new();
    let fresh = fresh_install.launch();
    fresh
        .sign_in(&server.url, "alice", PASSPHRASE)
        .await
        .unwrap();
    eventually("the resync to re-upload local notes", || {
        let titles = titles(&fresh, notes(), "");
        titles.contains(&"First edited after the backup".to_owned())
            && titles.contains(&"#later".to_owned())
    })
    .await;
    // The store was reopened at the same path and the workspace reloaded.
    assert!(
        named(&install, events::VAULT_CHANGED)
            .iter()
            .any(|p| p["tree_changed"] == true)
    );
    assert!(install.db().exists());
    assert_eq!(titles(&app, tag("later"), ""), ["#later"]);
    assert!(
        app.list_notes(notes(), "")
            .unwrap()
            .iter()
            .any(|e| e.id == second)
    );
}
