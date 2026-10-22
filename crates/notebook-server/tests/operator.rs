//! The `invite` and `user quota` commands run as processes, with
//! registration and login over the router against the same database.

use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use notebook_core::auth::api::{LoginStartResponse, RegisterStartResponse};
use notebook_core::auth::{Device, Login, Registration};
use notebook_server::app::{self, AppState};
use notebook_server::config::Config;
use notebook_server::{auth, db};
use tower::ServiceExt;

const PASSPHRASE: &str = "a long enough passphrase";

fn cli(dir: &Path, args: &[&str], env: &[(&str, &str)], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_notebook-server"))
        .args(args)
        .env_clear()
        .env("NOTEBOOK_PUBLIC_URL", "http://localhost")
        .env("NOTEBOOK_DATABASE_PATH", dir.join("notebook.db"))
        .env("NOTEBOOK_OPAQUE_KEY_PATH", dir.join("opaque.key"))
        .envs(env.iter().copied())
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

fn run(dir: &Path, args: &[&str]) -> Output {
    cli(dir, args, &[], "")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Both streams: errors are logged, and the log goes to stdout.
fn messages(output: &Output) -> String {
    stdout(output) + &stderr(output)
}

fn ok(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    stdout(&output)
}

/// `invite list` lines.
fn listing(dir: &Path, all: bool) -> Vec<String> {
    let args: &[&str] = if all {
        &["invite", "list", "--all"]
    } else {
        &["invite", "list"]
    };
    ok(run(dir, args)).lines().map(str::to_owned).collect()
}

/// The router over the CLI's database and key file.
async fn router(dir: &Path) -> Router {
    let config = Config::from_sources(None, |k| {
        (k == "NOTEBOOK_PUBLIC_URL").then(|| "http://localhost".to_owned())
    })
    .unwrap();
    let pool = db::open(&dir.join("notebook.db")).await.unwrap();
    let setup = auth::keyfile::load_or_create(&pool, &dir.join("opaque.key"))
        .await
        .unwrap();
    app::router(AppState::new(pool, config, setup))
}

async fn post(router: &Router, path: &str, body: &impl serde::Serialize) -> (StatusCode, Vec<u8>) {
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

/// Registers over HTTP; the finish status and body.
async fn register(router: &Router, username: &str, code: &str) -> (StatusCode, Vec<u8>) {
    let (reg, start) = Registration::start(username, PASSPHRASE, code).unwrap();
    let (status, body) = post(router, "/auth/register/start", &start).await;
    if status != StatusCode::OK {
        return (status, body);
    }
    let response: RegisterStartResponse = serde_json::from_slice(&body).unwrap();
    let (finish, _) = reg.finish(&response, Device::default(), 0).unwrap();
    post(router, "/auth/register/finish", &finish).await
}

async fn login(router: &Router, username: &str) -> StatusCode {
    let (login, start) = Login::start(username, PASSPHRASE).unwrap();
    let (status, body) = post(router, "/auth/login/start", &start).await;
    assert_eq!(status, StatusCode::OK);
    let response: LoginStartResponse = serde_json::from_slice(&body).unwrap();
    let (finish, _) = login.finish(&response, Device::default()).unwrap();
    post(router, "/auth/login/finish", &finish).await.0
}

/// True if any file in `dir` contains `needle`.
fn any_file_contains(dir: &Path, needle: &[u8]) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        bytes.windows(needle.len()).any(|w| w == needle)
    })
}

#[tokio::test]
async fn created_code_redeems_once_and_is_never_stored() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(dir.path(), &["invite", "create", "--note", "for alice"]);
    let code = ok(output.clone()).trim_end().to_owned();
    assert_eq!(
        stdout(&output),
        format!("{code}\n"),
        "only the code on stdout"
    );
    assert_eq!(code.len(), 23, "{code}");
    assert!(stderr(&output).contains("expires"), "{output:?}");
    let created = listing(dir.path(), false);
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(created[0].contains(r#"note "for alice""#), "{created:?}");
    let id = created[0].split_whitespace().next().unwrap().to_owned();

    let router = router(dir.path()).await;
    let (status, body) = register(&router, "alice", &code).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let (status, body) = register(&router, "bob", &code).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, br#"{"error":"invalid_invite"}"#);

    assert!(listing(dir.path(), false).is_empty());
    let all = listing(dir.path(), true);
    assert_eq!(all.len(), 1);
    assert!(all[0].contains(" used by alice at "), "{all:?}");

    // Revoking it now fails and leaves alice's account alone.
    let output = run(dir.path(), &["invite", "revoke", &id]);
    assert!(!output.status.success());
    assert!(
        messages(&output).contains("already used by alice"),
        "{output:?}"
    );
    assert_eq!(login(&router, "alice").await, StatusCode::OK);

    for form in [
        code.clone(),
        code.replace('-', ""),
        code.replace('-', "").to_lowercase(),
    ] {
        assert!(!any_file_contains(dir.path(), form.as_bytes()), "{form}");
    }
}

#[tokio::test]
async fn list_and_revoke() {
    let dir = tempfile::tempdir().unwrap();
    let created = run(dir.path(), &["invite", "create"]);
    // "Invite <id> expires <time>." on stderr.
    let id = stderr(&created)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    assert_eq!(id.len(), 8);
    let first = ok(created);
    ok(run(dir.path(), &["invite", "create", "--expires", "30m"]));
    let active = listing(dir.path(), false);
    assert_eq!(active.len(), 2, "{active:?}");
    assert!(active[0].starts_with(&id), "creation order: {active:?}");
    for line in &active {
        assert!(line.contains("  active"), "{line}");
        assert!(!line.contains(first.trim()), "listing shows a code: {line}");
    }

    let output = run(dir.path(), &["invite", "revoke", &id.to_lowercase()]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(listing(dir.path(), false), active[1..]);
    let all = listing(dir.path(), true);
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .any(|l| l.starts_with(&id) && l.contains("  revoked"))
    );

    // The revoked code no longer registers.
    let router = router(dir.path()).await;
    assert_eq!(
        register(&router, "carol", first.trim()).await.0,
        StatusCode::FORBIDDEN
    );

    let output = run(dir.path(), &["invite", "revoke", &id]);
    assert!(!output.status.success());
    assert!(messages(&output).contains("already revoked"), "{output:?}");
    let output = run(dir.path(), &["invite", "revoke", "ZZZZZZZZ"]);
    assert!(!output.status.success());
    assert!(messages(&output).contains("no invite"), "{output:?}");
    assert_eq!(listing(dir.path(), true), all);
}

#[tokio::test]
async fn bad_expiry_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["soon", "0m", "366d"] {
        let output = run(dir.path(), &["invite", "create", "--expires", bad]);
        assert!(!output.status.success(), "{bad}");
        assert!(stderr(&output).contains("--expires"), "{output:?}");
        assert!(stdout(&output).is_empty());
    }
    assert!(listing(dir.path(), true).is_empty());
}

#[tokio::test]
async fn user_add_needs_no_invite_and_gets_the_default_quota() {
    let dir = tempfile::tempdir().unwrap();
    let fifty_mib = [("NOTEBOOK_DEFAULT_VAULT_QUOTA_BYTES", "52428800")];
    ok(cli(
        dir.path(),
        &["user", "add", "alice", "--passphrase-stdin"],
        &fifty_mib,
        &format!("{PASSPHRASE}\n"),
    ));
    assert!(listing(dir.path(), true).is_empty());
    assert_eq!(
        ok(run(dir.path(), &["user", "quota", "Alice"])),
        "alice: 0 of 52428800 bytes used\n"
    );
    let router = router(dir.path()).await;
    assert_eq!(login(&router, "alice").await, StatusCode::OK);
}

#[tokio::test]
async fn user_quota_shows_and_sets() {
    let dir = tempfile::tempdir().unwrap();
    ok(cli(
        dir.path(),
        &["user", "add", "alice", "--passphrase-stdin"],
        &[],
        &format!("{PASSPHRASE}\n"),
    ));
    assert_eq!(
        ok(run(dir.path(), &["user", "quota", "alice"])),
        "alice: 0 of 104857600 bytes used\n"
    );
    assert_eq!(
        ok(run(dir.path(), &["user", "quota", "alice", "1GiB"])),
        "alice: 0 of 1073741824 bytes used\n"
    );

    // Lowering below usage is allowed.
    let pool = db::open(&dir.path().join("notebook.db")).await.unwrap();
    sqlx::query("UPDATE vaults SET used_bytes = 500")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    assert_eq!(
        ok(run(dir.path(), &["user", "quota", "alice", "10"])),
        "alice: 500 of 10 bytes used\n"
    );

    for args in [
        &["user", "quota", "nobody"][..],
        &["user", "quota", "nobody", "1GiB"],
        &["user", "quota", "alice", "lots"],
        &["user", "quota", "alice", "0"],
    ] {
        let output = run(dir.path(), args);
        assert!(!output.status.success(), "{args:?}");
    }
    assert_eq!(
        ok(run(dir.path(), &["user", "quota", "alice"])),
        "alice: 500 of 10 bytes used\n"
    );
}
