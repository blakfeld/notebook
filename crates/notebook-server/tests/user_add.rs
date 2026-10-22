//! `notebook-server user add` run as a process, then a login over the router
//! with the account it created.

use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use notebook_core::auth::api::{LoginFinishResponse, LoginStartResponse};
use notebook_core::auth::{Device, Login};
use notebook_core::crypto::{KeyBundle, RecoveryKey};
use notebook_server::app::{self, AppState};
use notebook_server::config::Config;
use notebook_server::{auth, db};
use tower::ServiceExt;

const PASSPHRASE: &str = "a long enough passphrase";

fn user_add(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_notebook-server"))
        .args(["user", "add"])
        .args(args)
        .env_clear()
        .env("NOTEBOOK_PUBLIC_URL", "http://localhost")
        .env("NOTEBOOK_DATABASE_PATH", dir.join("notebook.db"))
        .env("NOTEBOOK_OPAQUE_KEY_PATH", dir.join("opaque.key"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// The recovery key line from `user add`'s output.
fn recovery_key(output: &Output) -> RecoveryKey {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|l| l.len() == 39 && l.matches('-').count() == 7)
        .unwrap_or_else(|| panic!("no recovery key in {stdout}"));
    RecoveryKey::parse(line).unwrap()
}

async fn post(
    router: &axum::Router,
    path: &str,
    body: &impl serde::Serialize,
) -> (StatusCode, Vec<u8>) {
    let request = Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap();
    let response = router
        .clone()
        .layer(MockConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1))))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

async fn login(dir: &Path, username: &str, passphrase: &str) -> (LoginFinishResponse, KeyBundle) {
    let config = Config::from_sources(None, |k| {
        (k == "NOTEBOOK_PUBLIC_URL").then(|| "http://localhost".to_owned())
    })
    .unwrap();
    let pool = db::connect(&dir.join("notebook.db")).await.unwrap();
    let setup = auth::keyfile::load_or_create(&pool, &dir.join("opaque.key"))
        .await
        .unwrap();
    let router = app::router(AppState::new(pool, config, setup));

    let (login, start) = Login::start(username, passphrase).unwrap();
    let (status, body) = post(&router, "/auth/login/start", &start).await;
    assert_eq!(status, StatusCode::OK);
    let response: LoginStartResponse = serde_json::from_slice(&body).unwrap();
    let (finish, keys) = login.finish(&response, Device::default()).unwrap();
    let (status, body) = post(&router, "/auth/login/finish", &finish).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let response: LoginFinishResponse = serde_json::from_slice(&body).unwrap();
    keys.unlock(&response.key_bundle).unwrap();
    let bundle = KeyBundle::from_bytes(&response.key_bundle).unwrap();
    (response, bundle)
}

async fn user_count(dir: &Path) -> i64 {
    let pool = db::connect(&dir.join("notebook.db")).await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn added_user_logs_in_with_the_existing_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let output = user_add(
        dir.path(),
        &["Alice", "--passphrase-stdin"],
        &format!("{PASSPHRASE}\n"),
    );
    assert!(output.status.success(), "{output:?}");
    let recovery = recovery_key(&output);
    assert!(dir.path().join("opaque.key").exists());

    let (response, bundle) = login(dir.path(), "alice", PASSPHRASE).await;
    assert!(!response.session.is_admin);
    // The bundle the CLI created, which the printed recovery key also opens.
    assert!(bundle.unlock_with_recovery(&recovery).is_ok());
    assert_eq!(user_count(dir.path()).await, 1);
}

#[tokio::test]
async fn admin_flag_is_reported_at_login() {
    let dir = tempfile::tempdir().unwrap();
    let output = user_add(
        dir.path(),
        &["root", "--admin", "--passphrase-stdin"],
        &format!("{PASSPHRASE}\n"),
    );
    assert!(output.status.success(), "{output:?}");
    let (response, _) = login(dir.path(), "root", PASSPHRASE).await;
    assert!(response.session.is_admin);
}

#[tokio::test]
async fn bad_input_exits_non_zero_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ok = format!("{PASSPHRASE}\n");
    assert!(
        user_add(dir.path(), &["bob", "--passphrase-stdin"], &ok)
            .status
            .success()
    );

    for (args, stdin) in [
        (&["al ice", "--passphrase-stdin"][..], ok.as_str()),
        (&["BOB", "--passphrase-stdin"][..], ok.as_str()),
        (&["carol", "--passphrase-stdin"][..], "elevenchars\n"),
        // No terminal to prompt on, and no --passphrase-stdin.
        (&["dave"][..], ok.as_str()),
    ] {
        let output = user_add(dir.path(), args, stdin);
        assert!(!output.status.success(), "{args:?} succeeded");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("Recovery key"),
            "{args:?} printed {stdout}"
        );
    }
    assert_eq!(user_count(dir.path()).await, 1);
    login(dir.path(), "bob", PASSPHRASE).await;
}
