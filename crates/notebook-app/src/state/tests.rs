//! State machine tests against a real `notebook-server` in this process.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notebook_core::crypto::{AuthSecret, KeyBundle, RecoveryKey};
use notebook_core::sync::SyncConfig;
use notebook_server::app;
use notebook_server::auth::{keyfile, unix_now};
use notebook_server::config::Config;
use notebook_server::sync::{LiveSettings, SyncState};
use notebook_server::{db, invites};
use serde_json::Value;

use super::*;
use crate::credentials::MemoryStore;
use crate::dto::ViewDto;
use crate::events::Recorder;

pub(crate) const PASSPHRASE: &str = "correct horse battery staple";
pub(crate) const NEW_PASSPHRASE: &str = "a brand new passphrase";

/// `notebook-server` on its own runtime, so stopping it drops every
/// connection at once (the server's graceful shutdown would keep them).
pub(crate) struct TestServer {
    pub url: String,
    addr: SocketAddr,
    dir: tempfile::TempDir,
    runtime: Option<tokio::runtime::Runtime>,
    pool: Option<sqlx::SqlitePool>,
}

impl TestServer {
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut server = Self {
            url: String::new(),
            addr: "127.0.0.1:0".parse().unwrap(),
            dir,
            runtime: None,
            pool: None,
        };
        server.serve().await;
        server
    }

    async fn serve(&mut self) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let db_path = self.dir.path().join("server.db");
        let key_path = self.dir.path().join("opaque.key");
        let addr = self.addr;
        let (pool, addr) = runtime
            .spawn(async move {
                let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
                let addr = listener.local_addr().unwrap();
                let url = format!("http://{addr}");
                let config = Config::from_sources(None, |key| match key {
                    "NOTEBOOK_PUBLIC_URL" => Some(url.clone()),
                    "NOTEBOOK_RATE_LIMIT_PER_SECOND" => Some("1000".into()),
                    "NOTEBOOK_RATE_LIMIT_BURST" => Some("1000".into()),
                    "NOTEBOOK_REGISTRATION_RATE_LIMIT_BURST" => Some("100".into()),
                    _ => None,
                })
                .unwrap();
                let pool = db::connect(&db_path).await.unwrap();
                db::migrate(&pool).await.unwrap();
                let setup = keyfile::load_or_create(&pool, &key_path).await.unwrap();
                let mut state = app::AppState::new(pool.clone(), config, setup);
                // Notice ended sessions on open live channels quickly.
                state.sync = Arc::new(SyncState::new(LiveSettings {
                    ping_interval: Duration::from_millis(200),
                    ..LiveSettings::default()
                }));
                let router = app::router(state);
                tokio::spawn(app::run(listener, router, std::future::pending()));
                (pool, addr)
            })
            .await
            .unwrap();
        self.addr = addr;
        self.url = format!("http://{addr}");
        self.pool = Some(pool);
        self.runtime = Some(runtime);
    }

    /// Stops answering: open connections drop, new ones are refused, and
    /// the database is closed (so its files can be copied).
    pub fn stop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
        if let Some(pool) = self.pool.take() {
            // On a fresh runtime: the server's is gone, and the caller may be
            // inside another.
            std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(pool.close());
            })
            .join()
            .unwrap();
        }
    }

    /// Serves again on the same address with the same data.
    pub async fn restart(&mut self) {
        self.stop();
        self.serve().await;
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.path().join("server.db")
    }

    pub async fn invite(&self) -> String {
        let pool = self.pool.clone().unwrap();
        self.runtime
            .as_ref()
            .unwrap()
            .spawn(async move {
                invites::create(&pool, unix_now(), Duration::from_secs(86_400), None)
                    .await
                    .unwrap()
                    .1
            })
            .await
            .unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One install: its data directory and credential store, which survive a
/// relaunch.
pub(crate) struct Install {
    pub dir: tempfile::TempDir,
    pub creds: Arc<MemoryStore>,
    pub events: Arc<Recorder>,
}

pub(crate) fn sync_config() -> SyncConfig {
    SyncConfig {
        jitter: false,
        debounce: Duration::from_millis(20),
        max_debounce: Duration::from_millis(100),
        poll_interval: Duration::from_millis(200),
        backoff_base: Duration::from_millis(50),
        backoff_cap: Duration::from_millis(300),
        ..SyncConfig::default()
    }
}

impl Install {
    pub fn new() -> Self {
        Self::with_store(MemoryStore::new())
    }

    pub fn with_store(store: MemoryStore) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            creds: Arc::new(store),
            events: Arc::new(Recorder::default()),
        }
    }

    /// Starts the app, as at launch.
    pub fn launch(&self) -> AppState {
        AppState::launch(
            AppConfig {
                data_dir: self.dir.path().to_owned(),
                sync: sync_config(),
                device: Device::default(),
            },
            self.creds.clone(),
            self.events.clone(),
        )
    }

    pub fn db(&self) -> PathBuf {
        vault::database_path(self.dir.path())
    }

    pub fn has(&self, entry: Entry) -> bool {
        self.creds.get(entry).unwrap().is_some()
    }
}

impl AppState {
    /// Quits: stops the engine without changing any entry.
    pub(crate) async fn quit(&self) {
        let previous = std::mem::replace(&mut *self.lock_phase(), Phase::SignedOut);
        if let Phase::Unlocked(unlocked) = previous {
            self.0.stop(*unlocked).await;
        }
    }
}

/// Polls `check` until it holds, failing after ten seconds.
pub(crate) async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub(crate) async fn registered(server: &TestServer, install: &Install, username: &str) -> AppState {
    let app = install.launch();
    app.generate_recovery_key().unwrap();
    let invite = server.invite().await;
    app.register(&server.url, username, PASSPHRASE, PASSPHRASE, &invite)
        .await
        .unwrap();
    app
}

pub(crate) fn notes() -> SelectionDto {
    SelectionDto::View(ViewDto::Notes)
}

use crate::dto::SelectionDto;

fn code<T: std::fmt::Debug>(result: AppResult<T>) -> Code {
    result.unwrap_err().code
}

fn phases(events: &Recorder) -> Vec<Value> {
    events
        .take()
        .into_iter()
        .filter(|(name, _)| *name == events::APP_PHASE_CHANGED)
        .map(|(_, payload)| payload["phase"].clone())
        .collect()
}

/// An address nothing listens on: any request to it fails.
fn dead_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---- 4.2 launch phases ----

#[tokio::test]
async fn launch_without_entries_is_signed_out() {
    let install = Install::new();
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::SignedOut);
    assert_eq!(app.phase().account, None);
    assert!(app.phase().credentials_remembered);
}

#[tokio::test]
async fn launch_with_auto_unlock_opens_the_vault() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.create_note(notes()).unwrap();
    app.quit().await;

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::Unlocked);
    assert_eq!(app.list_notes(notes(), "").unwrap().len(), 1);
}

#[tokio::test]
async fn launch_without_auto_unlock_is_locked() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.lock().await.unwrap();
    assert!(!install.has(Entry::AutoUnlock));
    app.quit().await;

    let app = install.launch();
    let phase = app.phase();
    assert_eq!(phase.phase, PhaseName::Locked);
    assert_eq!(phase.account.unwrap().username, "alice");
    assert_eq!(code(app.list_notes(notes(), "")), Code::Locked);
}

// ---- 4.3 sign in and register ----

#[tokio::test]
async fn invalid_input_sends_no_request() {
    let install = Install::new();
    let app = install.launch();
    // A remote http address is refused before any connection.
    assert_eq!(
        code(
            app.sign_in("http://notes.example.com", "alice", PASSPHRASE)
                .await
        ),
        Code::InvalidServerUrl
    );
    // Against an address nothing answers, a request would be `unreachable`.
    let dead = dead_server();
    app.generate_recovery_key().unwrap();
    assert_eq!(
        code(
            app.register(&dead, "alice", PASSPHRASE, "different one!!", "x")
                .await
        ),
        Code::PassphraseMismatch
    );
    assert_eq!(
        code(
            app.register(&dead, "alice", "too short", "too short", "x")
                .await
        ),
        Code::PassphraseTooShort
    );
    assert_eq!(
        code(
            app.recover(&dead, "alice", "not-a-key", PASSPHRASE, PASSPHRASE)
                .await
        ),
        Code::RecoveryKeyMalformed
    );
    assert_eq!(
        code(app.sign_in(&dead, "alice", PASSPHRASE).await),
        Code::Unreachable
    );
}

#[tokio::test]
async fn registration_opens_an_empty_vault() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    let phase = app.phase();
    assert_eq!(phase.phase, PhaseName::Unlocked);
    assert_eq!(phase.account.unwrap().server, server.url);
    assert!(app.list_notes(notes(), "").unwrap().is_empty());
    assert!(install.has(Entry::Session));
    assert!(install.has(Entry::LocalUnlock));
    assert!(install.has(Entry::AutoUnlock));
    assert!(install.db().exists());
    assert_eq!(phases(&install.events), [Value::from("unlocked")]);
}

#[tokio::test]
async fn register_needs_a_pending_recovery_key() {
    let server = TestServer::start().await;
    let app_install = Install::new();
    let app = app_install.launch();
    let invite = server.invite().await;
    assert_eq!(
        code(
            app.register(&server.url, "alice", PASSPHRASE, PASSPHRASE, &invite)
                .await
        ),
        Code::RecoveryKeyRequired
    );
}

#[tokio::test]
async fn register_refusals_keep_the_recovery_key() {
    let server = TestServer::start().await;
    let _taken_install = Install::new();
    let _taken = registered(&server, &_taken_install, "taken").await;

    let app_install = Install::new();
    let app = app_install.launch();
    app.generate_recovery_key().unwrap();
    assert_eq!(
        code(
            app.register(&server.url, "bob", PASSPHRASE, PASSPHRASE, "bogus-code")
                .await
        ),
        Code::InvalidInvite
    );
    let invite = server.invite().await;
    assert_eq!(
        code(
            app.register(&server.url, "taken", PASSPHRASE, PASSPHRASE, &invite)
                .await
        ),
        Code::UsernameTaken
    );
    // The same key is still pending, so the user can fix the username.
    app.register(&server.url, "bob", PASSPHRASE, PASSPHRASE, &invite)
        .await
        .unwrap();
    assert_eq!(app.phase().phase, PhaseName::Unlocked);
}

#[tokio::test]
async fn unknown_user_and_wrong_passphrase_are_both_bad_credentials() {
    let server = TestServer::start().await;
    let _alice_install = Install::new();
    let _alice = registered(&server, &_alice_install, "alice").await;
    let app_install = Install::new();
    let app = app_install.launch();
    assert_eq!(
        code(app.sign_in(&server.url, "nobody", PASSPHRASE).await),
        Code::BadCredentials
    );
    assert_eq!(
        code(app.sign_in(&server.url, "alice", "wrong wrong wrong").await),
        Code::BadCredentials
    );
    assert_eq!(app.phase().phase, PhaseName::SignedOut);
}

#[tokio::test]
async fn sign_in_on_a_fresh_install_downloads_the_vault() {
    let server = TestServer::start().await;
    let first_install = Install::new();
    let first = registered(&server, &first_install, "alice").await;
    for _ in 0..3 {
        first.create_note(notes()).unwrap();
    }
    eventually("the first device to sync", || {
        first.pending_count().unwrap() == 0
    })
    .await;

    let install = Install::new();
    let second = install.launch();
    second
        .sign_in(&server.url, "alice", PASSPHRASE)
        .await
        .unwrap();
    eventually("the second device to download", || {
        second.list_notes(notes(), "").unwrap().len() == 3
    })
    .await;
}

#[tokio::test]
async fn without_a_secure_store_nothing_is_remembered() {
    let server = TestServer::start().await;
    let _alice_install = Install::new();
    let _alice = registered(&server, &_alice_install, "alice").await;

    let install = Install::with_store(MemoryStore::unavailable());
    let app = install.launch();
    let phase = app.sign_in(&server.url, "alice", PASSPHRASE).await.unwrap();
    assert_eq!(phase.phase, PhaseName::Unlocked);
    assert!(!phase.credentials_remembered);
    app.create_note(notes()).unwrap();
    app.quit().await;

    // The OS store still has nothing (and can't), so the next launch asks
    // to sign in.
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::SignedOut);
    // No secret went to a file instead.
    for name in file_names(install.dir.path()) {
        assert!(name.starts_with("vault.sqlite3"), "{name}");
    }
}

// ---- 4.4 lock and unlock ----

#[tokio::test]
async fn lock_hides_everything_and_persists() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.create_note(notes()).unwrap();
    install.events.take();

    let phase = app.lock().await.unwrap();
    assert_eq!(phase.phase, PhaseName::Locked);
    assert_eq!(phases(&install.events), [Value::from("locked")]);
    assert_eq!(code(app.list_notes(notes(), "")), Code::Locked);
    assert_eq!(code(app.tag_tree()), Code::Locked);
    assert!(!install.has(Entry::AutoUnlock));
    assert!(install.has(Entry::Session));

    app.quit().await;
    assert_eq!(install.launch().phase().phase, PhaseName::Locked);
}

#[tokio::test]
async fn unlock_online() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.create_note(notes()).unwrap();
    app.lock().await.unwrap();

    let phase = app.unlock(PASSPHRASE).await.unwrap();
    assert_eq!(phase.phase, PhaseName::Unlocked);
    assert_eq!(app.list_notes(notes(), "").unwrap().len(), 1);
    assert!(install.has(Entry::AutoUnlock));
    eventually("sync to resume", || app.pending_count().unwrap() == 0).await;
}

#[tokio::test]
async fn unlock_offline() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.create_note(notes()).unwrap();
    app.lock().await.unwrap();
    server.stop();

    assert_eq!(
        code(app.unlock("wrong wrong wrong").await),
        Code::WrongPassphrase
    );
    assert_eq!(app.phase().phase, PhaseName::Locked);
    app.unlock(PASSPHRASE).await.unwrap();
    assert_eq!(app.list_notes(notes(), "").unwrap().len(), 1);
}

#[tokio::test]
async fn wrong_passphrase_online_stays_locked() {
    let server = TestServer::start().await;
    let app_install = Install::new();
    let app = registered(&server, &app_install, "alice").await;
    app.lock().await.unwrap();
    assert_eq!(
        code(app.unlock("wrong wrong wrong").await),
        Code::WrongPassphrase
    );
    assert_eq!(app.phase().phase, PhaseName::Locked);
}

#[tokio::test]
async fn changed_fingerprint_forces_online_unlock() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;

    // A refresh that brings back a different bundle, as after a passphrase
    // change on another device.
    let vault = match &*app.lock_phase() {
        Phase::Unlocked(u) => u.account.vault,
        _ => unreachable!(),
    };
    let other = KeyBundle::create(
        vault,
        &VaultKey::generate().unwrap(),
        &AuthSecret::from_bytes(&[5; 32]).unwrap(),
        &RecoveryKey::generate().unwrap(),
    )
    .unwrap();
    let stored = credentials::load_session(&*install.creds).unwrap().unwrap();
    app.0.on_refresh(&Refreshed {
        refresh_token: stored.refresh_token.clone(),
        key_bundle: Some(other.to_bytes()),
    });
    assert!(!install.has(Entry::LocalUnlock));

    // The same bundle again changes nothing.
    app.lock().await.unwrap();
    server.stop();
    assert_eq!(code(app.unlock(PASSPHRASE).await), Code::Offline);
    server.restart().await;
    app.unlock(PASSPHRASE).await.unwrap();
    assert!(install.has(Entry::LocalUnlock));
}

#[tokio::test]
async fn a_refresh_with_the_same_bundle_keeps_the_local_wrap() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    let (tokens, account) = app
        .with_vault(|u| Ok((u.tokens.clone(), u.account.clone())))
        .unwrap();
    let client = auth_client(&account.server).unwrap();
    let bundle = tokens
        .authed(&client, |t| {
            let client = &client;
            async move { client.key_bundle(&t).await }
        })
        .await
        .unwrap();
    check_fingerprint(&*install.creds, account.vault, &bundle);
    assert!(install.has(Entry::LocalUnlock));
}

// ---- 4.5 recovery, passphrase change, sign-out, session expiry ----

#[tokio::test]
async fn recovery_typo_and_wrong_key_differ() {
    let server = TestServer::start().await;
    let _alice_install = Install::new();
    let _alice = registered(&server, &_alice_install, "alice").await;
    let app_install = Install::new();
    let app = app_install.launch();
    let key = RecoveryKey::generate().unwrap();
    let mut typo = key.to_display().to_string();
    let last = typo.pop().unwrap();
    typo.push(if last == 'A' { 'B' } else { 'A' });
    assert_eq!(
        code(
            app.recover(&server.url, "alice", &typo, NEW_PASSPHRASE, NEW_PASSPHRASE)
                .await
        ),
        Code::RecoveryKeyMalformed
    );
    assert_eq!(
        code(
            app.recover(
                &server.url,
                "alice",
                &key.to_display(),
                NEW_PASSPHRASE,
                NEW_PASSPHRASE
            )
            .await
        ),
        Code::RecoveryKeyWrong
    );
}

#[tokio::test]
async fn recovery_replaces_the_passphrase() {
    let server = TestServer::start().await;
    let first_install = Install::new();
    let first = first_install.launch();
    let display = first.generate_recovery_key().unwrap().to_string();
    let invite = server.invite().await;
    first
        .register(&server.url, "alice", PASSPHRASE, PASSPHRASE, &invite)
        .await
        .unwrap();
    first.create_note(notes()).unwrap();
    eventually("the note to sync", || first.pending_count().unwrap() == 0).await;

    let app_install = Install::new();
    let app = app_install.launch();
    app.recover(
        &server.url,
        "alice",
        &display,
        NEW_PASSPHRASE,
        NEW_PASSPHRASE,
    )
    .await
    .unwrap();
    eventually("notes to download", || {
        app.list_notes(notes(), "").unwrap().len() == 1
    })
    .await;

    let other_install = Install::new();
    let other = other_install.launch();
    assert_eq!(
        code(other.sign_in(&server.url, "alice", PASSPHRASE).await),
        Code::BadCredentials
    );
    other
        .sign_in(&server.url, "alice", NEW_PASSPHRASE)
        .await
        .unwrap();
}

#[tokio::test]
async fn passphrase_change() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;

    assert_eq!(
        code(
            app.change_passphrase("wrong wrong wrong", NEW_PASSPHRASE, NEW_PASSPHRASE)
                .await
        ),
        Code::WrongPassphrase
    );
    assert_eq!(
        code(
            app.change_passphrase(PASSPHRASE, NEW_PASSPHRASE, "something else!")
                .await
        ),
        Code::PassphraseMismatch
    );
    app.change_passphrase(PASSPHRASE, NEW_PASSPHRASE, NEW_PASSPHRASE)
        .await
        .unwrap();

    let other_install = Install::new();
    let other = other_install.launch();
    assert_eq!(
        code(other.sign_in(&server.url, "alice", PASSPHRASE).await),
        Code::BadCredentials
    );
    other
        .sign_in(&server.url, "alice", NEW_PASSPHRASE)
        .await
        .unwrap();

    // The old passphrase no longer unlocks this device, even offline.
    app.lock().await.unwrap();
    server.stop();
    assert_eq!(code(app.unlock(PASSPHRASE).await), Code::WrongPassphrase);
    app.unlock(NEW_PASSPHRASE).await.unwrap();
}

#[tokio::test]
async fn passphrase_change_offline_is_refused() {
    let mut server = TestServer::start().await;
    let app_install = Install::new();
    let app = registered(&server, &app_install, "alice").await;
    server.stop();
    assert_eq!(
        code(
            app.change_passphrase(PASSPHRASE, NEW_PASSPHRASE, NEW_PASSPHRASE)
                .await
        ),
        Code::Offline
    );
    server.restart().await;
    let other_install = Install::new();
    let other = other_install.launch();
    other
        .sign_in(&server.url, "alice", PASSPHRASE)
        .await
        .unwrap();
}

#[tokio::test]
async fn sign_out_removes_the_database_and_entries() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.create_note(notes()).unwrap();
    eventually("the note to sync", || app.pending_count().unwrap() == 0).await;
    install.events.take();

    let phase = app.sign_out(false).await.unwrap();
    assert_eq!(phase.phase, PhaseName::SignedOut);
    assert_eq!(phases(&install.events), [Value::from("signed_out")]);
    for entry in [Entry::Session, Entry::LocalUnlock, Entry::AutoUnlock] {
        assert!(!install.has(entry), "{entry:?}");
    }
    assert_eq!(file_names(install.dir.path()), Vec::<String>::new());
    assert_eq!(install.launch().phase().phase, PhaseName::SignedOut);
}

#[tokio::test]
async fn sign_out_with_pending_changes_needs_confirmation() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    eventually("the first sync", || {
        app.sync_status().unwrap().state == crate::dto::SyncStateDto::Synced
    })
    .await;
    server.stop();
    for _ in 0..3 {
        app.create_note(notes()).unwrap();
    }
    assert_eq!(app.pending_count().unwrap(), 3);
    let err = app.sign_out(false).await.unwrap_err();
    assert_eq!(err.code, Code::PendingChanges);
    assert!(err.message.contains('3'));
    assert_eq!(app.phase().phase, PhaseName::Unlocked);

    // Signing out with the server unreachable still finishes.
    app.sign_out(true).await.unwrap();
    assert!(!install.db().exists());
    assert!(!install.has(Entry::Session));
}

#[tokio::test]
async fn sign_out_while_locked() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.lock().await.unwrap();
    assert_eq!(app.pending_count().unwrap(), 0);
    app.sign_out(false).await.unwrap();
    assert!(!install.db().exists());
    assert!(!install.has(Entry::Session));
}

#[tokio::test]
async fn edits_continue_after_the_session_ends() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    eventually("the first sync", || {
        app.sync_status().unwrap().state == crate::dto::SyncStateDto::Synced
    })
    .await;

    // A passphrase change on another device ends this device's session.
    let other_install = Install::new();
    let other = other_install.launch();
    other
        .sign_in(&server.url, "alice", PASSPHRASE)
        .await
        .unwrap();
    other
        .change_passphrase(PASSPHRASE, NEW_PASSPHRASE, NEW_PASSPHRASE)
        .await
        .unwrap();

    let id = app.create_note(notes()).unwrap();
    eventually("sign-in required", || {
        app.sync_status()
            .unwrap()
            .attention
            .contains(&crate::dto::AttentionDto::SignInRequired)
    })
    .await;
    app.set_pinned(&id, true).unwrap();
    assert_eq!(app.phase().phase, PhaseName::Unlocked);
    assert!(app.pending_count().unwrap() > 0);

    assert_eq!(
        code(app.reauthenticate(PASSPHRASE).await),
        Code::WrongPassphrase
    );
    app.reauthenticate(NEW_PASSPHRASE).await.unwrap();
    eventually("pending edits to sync", || {
        app.pending_count().unwrap() == 0
    })
    .await;
    eventually("the other device to see them", || {
        other
            .list_notes(notes(), "")
            .unwrap()
            .iter()
            .any(|e| e.id == id && e.pinned)
    })
    .await;
}

// ---- review fixes ----

#[tokio::test]
async fn sync_resumes_after_a_relaunch() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.quit().await;

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::Unlocked);
    app.create_note(notes()).unwrap();
    eventually("the relaunched device to sync", || {
        let status = app.sync_status().unwrap();
        status.pending == 0 && status.state == crate::dto::SyncStateDto::Synced
    })
    .await;
    assert!(app.sync_status().unwrap().attention.is_empty());
}

#[tokio::test]
async fn sync_resumes_after_an_offline_unlock() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.lock().await.unwrap();
    server.stop();
    app.unlock(PASSPHRASE).await.unwrap();
    app.create_note(notes()).unwrap();
    server.restart().await;
    eventually("the edit to sync once online", || {
        app.pending_count().unwrap() == 0
    })
    .await;
    assert!(app.sync_status().unwrap().attention.is_empty());
}

#[tokio::test]
async fn old_passphrase_stops_unlocking_after_a_change_elsewhere() {
    let mut server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    eventually("the first sync", || {
        app.sync_status().unwrap().state == crate::dto::SyncStateDto::Synced
    })
    .await;

    let other_install = Install::new();
    let other = other_install.launch();
    other
        .sign_in(&server.url, "alice", PASSPHRASE)
        .await
        .unwrap();
    other
        .change_passphrase(PASSPHRASE, NEW_PASSPHRASE, NEW_PASSPHRASE)
        .await
        .unwrap();

    // This device's session ends. It can't fetch the new bundle, so it fails
    // closed.
    eventually("sign-in required", || {
        app.sync_status()
            .unwrap()
            .attention
            .contains(&crate::dto::AttentionDto::SignInRequired)
    })
    .await;
    assert!(!install.has(Entry::LocalUnlock));
    app.lock().await.unwrap();
    server.stop();
    assert_eq!(code(app.unlock(PASSPHRASE).await), Code::Offline);
    server.restart().await;
    assert_eq!(code(app.unlock(PASSPHRASE).await), Code::WrongPassphrase);
    app.unlock(NEW_PASSPHRASE).await.unwrap();
}

#[tokio::test]
async fn a_store_failing_partway_leaves_nothing_behind() {
    let server = TestServer::start().await;
    let alice_install = Install::new();
    registered(&server, &alice_install, "alice").await;

    // The session entry is written, then the store stops working.
    let install = Install::with_store(MemoryStore::fails_after(1));
    let app = install.launch();
    let phase = app.sign_in(&server.url, "alice", PASSPHRASE).await.unwrap();
    assert!(!phase.credentials_remembered);
    assert!(!install.has(Entry::Session), "stranded in the OS store");

    // Everything still works from memory for this run.
    app.lock().await.unwrap();
    app.unlock(PASSPHRASE).await.unwrap();
    app.sign_out(true).await.unwrap();
    assert_eq!(install.launch().phase().phase, PhaseName::SignedOut);
}

#[tokio::test]
async fn collapsed_tags_are_sealed_and_hidden_while_locked() {
    const TAG: &str = "secret-project-omega";
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.set_tag_expanded(TAG, false).unwrap();
    assert_eq!(app.settings().unwrap().collapsed_tags, [TAG]);
    app.lock().await.unwrap();

    assert!(app.settings().unwrap().collapsed_tags.is_empty());
    assert_eq!(code(app.set_tag_expanded(TAG, true)), Code::Locked);
    for name in file_names(install.dir.path()) {
        let bytes = std::fs::read(install.dir.path().join(&name)).unwrap();
        assert!(
            !bytes.windows(TAG.len()).any(|w| w == TAG.as_bytes()),
            "{name}"
        );
    }
    app.unlock(PASSPHRASE).await.unwrap();
    assert_eq!(app.settings().unwrap().collapsed_tags, [TAG]);
}

#[tokio::test]
async fn sign_out_works_when_the_local_store_is_broken() {
    let server = TestServer::start().await;
    let install = Install::new();
    let app = registered(&server, &install, "alice").await;
    app.lock().await.unwrap();
    std::fs::write(install.db(), b"not a database at all").unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(install.dir.path().join(format!("vault.sqlite3{suffix}")));
    }
    assert_eq!(code(app.sign_out(false).await), Code::PendingChanges);
    app.sign_out(true).await.unwrap();
    assert!(!install.db().exists());
    assert!(!install.has(Entry::Session));
}
