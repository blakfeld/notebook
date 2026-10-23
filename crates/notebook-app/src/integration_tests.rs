//! End to end over IPC, as the frontend drives it: two installs of the app
//! against one in-process `notebook-server`.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tauri::WebviewWindow;
use tauri::test::MockRuntime;
use yrs::updates::decoder::Decode;
use yrs::{Text, Transact, Update};

use crate::state::tests::{Install, PASSPHRASE, TestServer, eventually};
use crate::tests::{call, mock_app};

type Webview = WebviewWindow<MockRuntime>;

fn ok(webview: &Webview, cmd: &str, args: Value) -> Value {
    call(webview, cmd, args).unwrap_or_else(|err| panic!("{cmd}: {err}"))
}

fn list(webview: &Webview, selection: Value) -> Vec<Value> {
    ok(
        webview,
        "list_notes",
        json!({"selection": selection, "query": ""}),
    )
    .as_array()
    .unwrap()
    .clone()
}

fn titles(webview: &Webview, selection: Value) -> Vec<String> {
    list(webview, selection)
        .iter()
        .map(|e| e["title"].as_str().unwrap().to_owned())
        .collect()
}

fn tree_names(webview: &Webview) -> Vec<String> {
    ok(webview, "tag_tree", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["path"].as_str().unwrap().to_owned())
        .collect()
}

/// Types `text` at the end of the note through an editor session, as the
/// frontend's Yjs binding would, and closes it.
fn type_into(webview: &Webview, note: &str, text: &str) {
    let opened = ok(webview, "editor_open", json!({"note": note}));
    let doc = yrs::Doc::with_options(yrs::Options {
        offset_kind: yrs::OffsetKind::Utf16,
        ..yrs::Options::with_client_id(yrs::ClientID::new(7))
    });
    let body = doc.get_or_insert_text("body");
    let state = STANDARD.decode(opened["state"].as_str().unwrap()).unwrap();
    doc.transact_mut()
        .apply_update(Update::decode_v1(&state).unwrap())
        .unwrap();
    let update = {
        let mut txn = doc.transact_mut();
        let end = body.len(&txn);
        body.insert(&mut txn, end, text);
        STANDARD.encode(txn.encode_update_v1())
    };
    let session = opened["session"].clone();
    ok(
        webview,
        "editor_push",
        json!({"session": session, "update": update}),
    );
    ok(webview, "editor_close", json!({"session": session}));
}

async fn until(what: &str, webview: &Webview, check: impl Fn(&Webview) -> bool) {
    eventually(what, || check(webview)).await;
}

#[tokio::test]
async fn two_devices_end_to_end() {
    let server = TestServer::start().await;
    let invite = server.invite().await;

    // Device A registers.
    let a_install = Install::new();
    let (_a_app, a) = mock_app(a_install.launch());
    assert_eq!(ok(&a, "app_state", json!({}))["phase"], "signed_out");
    let key = ok(&a, "generate_recovery_key", json!({}));
    assert!(key.as_str().is_some_and(|k| !k.is_empty()));
    let phase = ok(
        &a,
        "register",
        json!({
            "server": server.url,
            "username": "alice",
            "passphrase": PASSPHRASE,
            "confirm": PASSPHRASE,
            "inviteCode": invite,
        }),
    );
    assert_eq!(phase["phase"], "unlocked");

    // A creates and edits notes.
    let work = ok(
        &a,
        "create_note",
        json!({"selection": {"tag": "work/acme"}}),
    );
    let work = work.as_str().unwrap();
    type_into(&a, work, "Acme plan");
    let home = ok(&a, "create_note", json!({"selection": {"view": "notes"}}));
    let home = home.as_str().unwrap();
    type_into(&a, home, "Groceries\n#home");
    assert_eq!(tree_names(&a), ["home", "work"]);

    // Device B signs in and sees them.
    let b_install = Install::new();
    let (_b_app, b) = mock_app(b_install.launch());
    let phase = ok(
        &b,
        "sign_in",
        json!({"server": server.url, "username": "alice", "passphrase": PASSPHRASE}),
    );
    assert_eq!(phase["phase"], "unlocked");
    until("B to download both notes", &b, |b| {
        list(b, json!({"view": "notes"})).len() == 2
    })
    .await;
    assert_eq!(titles(&b, json!({"tag": "work"})), ["#work/acme"]);
    assert_eq!(titles(&b, json!({"tag": "home"})), ["Groceries"]);

    // A renames a tag; B follows.
    assert_eq!(
        ok(&a, "rename_tag", json!({"old": "work", "new": "job"})),
        1
    );
    assert_eq!(tree_names(&a), ["home", "job"]);
    until("B to see the rename", &b, |b| {
        tree_names(b) == ["home", "job"]
    })
    .await;
    assert_eq!(titles(&b, json!({"tag": "job/acme"})), ["#job/acme"]);

    // A trashes a note; B sees it in Trash.
    ok(&a, "set_trashed", json!({"id": home, "trashed": true}));
    until("B to see it trashed", &b, |b| {
        list(b, json!({"view": "trash"})).len() == 1 && list(b, json!({"view": "notes"})).len() == 1
    })
    .await;

    // A purges it once it has synced; it disappears everywhere.
    until("A to sync", &a, |a| ok(a, "pending_count", json!({})) == 0).await;
    ok(&a, "delete_permanently", json!({"id": home}));
    assert!(list(&a, json!({"view": "trash"})).is_empty());
    until("B to drop it", &b, |b| {
        list(b, json!({"view": "trash"})).is_empty()
    })
    .await;
    assert_eq!(tree_names(&b), ["job"]);

    // And a fresh device after all that sees the same vault.
    let c_install = Install::new();
    let (_c_app, c) = mock_app(c_install.launch());
    ok(
        &c,
        "sign_in",
        json!({"server": server.url, "username": "alice", "passphrase": PASSPHRASE}),
    );
    until("C to download", &c, |c| {
        list(c, json!({"view": "notes"})).len() == 1
    })
    .await;
    assert!(list(&c, json!({"view": "trash"})).is_empty());
    assert_eq!(titles(&c, json!({"tag": "job"})), ["#job/acme"]);
}
