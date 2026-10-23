//! Backup settings, commands over IPC, and the scheduler with a manual
//! clock. Restore is tested in `vault_tests.rs`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value, json};

use super::*;
use crate::state::tests::{Install, created, notes};
use crate::tests::{call, mock_app_with_pickers};

const HOUR: i64 = 3_600_000;

/// Pickers that answer with whatever the test set, `None` meaning
/// cancelled.
#[derive(Default)]
pub(crate) struct StubPickers {
    pub folder: Mutex<Option<PathBuf>>,
    pub file: Mutex<Option<PathBuf>>,
}

impl StubPickers {
    pub fn folder(path: &Path) -> Arc<Self> {
        let pickers = Self::default();
        *pickers.folder.lock().unwrap() = Some(path.to_owned());
        Arc::new(pickers)
    }

    pub fn file(path: &Path) -> Arc<Self> {
        let pickers = Self::default();
        *pickers.file.lock().unwrap() = Some(path.to_owned());
        Arc::new(pickers)
    }
}

impl BackupPicker for StubPickers {
    fn pick_folder(&self) -> BoxFuture<'static, Option<PathBuf>> {
        Box::pin(std::future::ready(self.folder.lock().unwrap().clone()))
    }

    fn pick_file(&self) -> BoxFuture<'static, Option<PathBuf>> {
        Box::pin(std::future::ready(self.file.lock().unwrap().clone()))
    }
}

/// The backup files in `folder`, sorted.
pub(crate) fn backups_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".nbbackup"))
        .collect();
    names.sort();
    names
}

/// Polls `check` until it holds, failing after ten seconds.
pub(crate) async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Creates a note and waits until the backup state has seen the change.
pub(crate) async fn edit(app: &AppState, body: &str) -> String {
    let id = app.create_note(notes()).unwrap();
    let note = crate::dto::parse_note_id(&id).unwrap();
    app.with_vault(|open| Ok(open.workspace.set_body(note, body, now())?))
        .unwrap();
    eventually("the change to be seen", || {
        app.backup_status().unwrap().changed
    })
    .await;
    id
}

/// An open notebook backing up into a new folder; its first backup is
/// written. Returns the folder (kept alive by the guard) and that backup's
/// time.
async fn backing_up(app: &AppState) -> (tempfile::TempDir, i64) {
    let folder = tempfile::tempdir().unwrap();
    let status = app
        .choose_backup_folder(&*StubPickers::folder(folder.path()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(backups_in(folder.path()).len(), 1);
    (folder, status.last.unwrap())
}

fn statuses(install: &Install) -> Vec<Value> {
    install
        .events
        .take()
        .into_iter()
        .filter(|(event, _)| *event == events::BACKUP_STATUS_CHANGED)
        .map(|(_, payload)| payload)
        .collect()
}

// ---- commands ----

#[tokio::test]
async fn backup_commands_over_ipc() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let folder = tempfile::tempdir().unwrap();
    let pickers = Arc::new(StubPickers::default());
    let (_app, webview) =
        mock_app_with_pickers(app, Arc::new(crate::tests::NoFolder), pickers.clone());

    // No folder yet: nothing scheduled, nothing to back up into.
    let status = call(&webview, "backup_status", json!({})).unwrap();
    assert_eq!(
        status,
        json!({
            "folder": null, "auto": true, "interval": "daily", "keep": 14,
            "last": null, "next_due": null, "changed": false,
            "last_error": null, "failing": false, "overdue": false, "running": false,
        })
    );
    let err = call(&webview, "backup_now", json!({})).unwrap_err();
    assert_eq!(err["code"], "backup_no_folder");

    // A cancelled picker changes nothing.
    assert_eq!(
        call(&webview, "choose_backup_folder", json!({})).unwrap(),
        Value::Null
    );
    assert_eq!(
        call(&webview, "backup_status", json!({})).unwrap()["folder"],
        Value::Null
    );

    // Choosing a folder backs up into it right away.
    *pickers.folder.lock().unwrap() = Some(folder.path().to_owned());
    let status = call(&webview, "choose_backup_folder", json!({})).unwrap();
    assert_eq!(status["folder"], folder.path().display().to_string());
    let last = status["last"].as_i64().unwrap();
    assert_eq!(status["next_due"], last + 24 * HOUR);
    assert_eq!(backups_in(folder.path()).len(), 1);

    let status = call(
        &webview,
        "set_backup_options",
        json!({"auto": false, "interval": "weekly", "keep": 3}),
    )
    .unwrap();
    assert_eq!(
        (&status["auto"], &status["interval"], &status["keep"]),
        (&json!(false), &json!("weekly"), &json!(3))
    );
    assert_eq!(status["next_due"], Value::Null);
    let err = call(
        &webview,
        "set_backup_options",
        json!({"auto": true, "interval": "monthly", "keep": 3}),
    );
    assert!(err.is_err());
    let err = call(
        &webview,
        "set_backup_options",
        json!({"auto": true, "interval": "daily", "keep": 0}),
    );
    assert!(err.is_err());

    // Back up now writes another backup even though nothing changed.
    std::thread::sleep(Duration::from_millis(1100));
    let status = call(&webview, "backup_now", json!({})).unwrap();
    assert!(status["last"].as_i64().unwrap() > last);
    assert_eq!(backups_in(folder.path()).len(), 2);

    let events = statuses(&install);
    assert!(events.iter().any(|s| s["running"] == true));
    assert_eq!(events.last().unwrap()["running"], false);
}

#[tokio::test]
async fn backup_commands_need_an_open_notebook() {
    let install = Install::new();
    let folder = tempfile::tempdir().unwrap();
    let (_app, webview) = mock_app_with_pickers(
        install.launch(),
        Arc::new(crate::tests::NoFolder),
        StubPickers::folder(folder.path()),
    );
    for (cmd, args) in [
        ("backup_status", json!({})),
        ("choose_backup_folder", json!({})),
        ("backup_now", json!({})),
        (
            "set_backup_options",
            json!({"auto": true, "interval": "daily", "keep": 3}),
        ),
    ] {
        assert_eq!(call(&webview, cmd, args).unwrap_err()["code"], "not_open");
    }
    assert!(backups_in(folder.path()).is_empty());
}

#[tokio::test]
async fn backup_now_reports_failure() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let (folder, last) = backing_up(&app).await;
    let path = folder.path().to_owned();
    drop(folder);

    let err = app.backup_now().await.unwrap_err();
    assert_eq!(err.code, Code::BackupFailed);
    assert_eq!(err.message, "the backup folder is unavailable");
    let status = app.backup_status().unwrap();
    assert_eq!(status.last, Some(last));
    assert!(status.failing);
    assert_eq!(
        status.last_error.as_deref(),
        Some("the backup folder is unavailable")
    );
    // Never created.
    assert!(!path.exists());
}

#[tokio::test]
async fn a_failed_first_backup_keeps_the_folder() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let gone = tempfile::tempdir().unwrap().path().to_owned();
    let status = app
        .choose_backup_folder(&*StubPickers::folder(&gone))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.folder, Some(gone.display().to_string()));
    assert!(status.failing);
    assert_eq!(status.last, None);
}

// ---- scheduler, with a manual clock ----

#[tokio::test]
async fn due_backup_runs() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let (folder, last) = backing_up(&app).await;
    edit(&app, "changed").await;

    // Not due within the interval; the change is remembered.
    app.backup_tick(Timestamp(last + 23 * HOUR)).await.unwrap();
    assert_eq!(backups_in(folder.path()).len(), 1);
    let replica = app.with_vault(|open| {
        Ok(open
            .workspace
            .replica()
            .lock()
            .storage()?
            .setting(DIRTY_KEY)?)
    });
    assert_eq!(replica.unwrap().as_deref(), Some("true"));

    let at = last + 25 * HOUR;
    app.backup_tick(Timestamp(at)).await.unwrap();
    assert_eq!(backups_in(folder.path()).len(), 2);
    let status = app.backup_status_at(Timestamp(at)).unwrap();
    assert_eq!(status.last, Some(at));
    assert!(!status.changed);
    assert_eq!(status.next_due, Some(at + 24 * HOUR));
}

#[tokio::test]
async fn nothing_changed() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let (folder, last) = backing_up(&app).await;
    app.backup_tick(Timestamp(last + 25 * HOUR)).await.unwrap();
    app.backup_tick(Timestamp(last + 30 * 24 * HOUR))
        .await
        .unwrap();
    assert_eq!(backups_in(folder.path()).len(), 1);
    let status = app.backup_status_at(Timestamp(last + 30 * 24 * HOUR));
    assert!(!status.unwrap().overdue);
}

#[tokio::test]
async fn first_backup() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    // No folder: ticks never back up.
    edit(&app, "changed").await;
    app.backup_tick(now()).await.unwrap();
    let (folder, _) = backing_up(&app).await;
    assert_eq!(backups_in(folder.path()).len(), 1);
}

#[tokio::test]
async fn auto_off_never_backs_up() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let (folder, last) = backing_up(&app).await;
    app.set_backup_options(false, Interval::Hourly, 14).unwrap();
    edit(&app, "changed").await;
    app.backup_tick(Timestamp(last + 25 * HOUR)).await.unwrap();
    assert_eq!(backups_in(folder.path()).len(), 1);
}

#[tokio::test]
async fn failing_and_overdue_status() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let (folder, last) = backing_up(&app).await;
    let path = folder.path().to_owned();
    drop(folder);
    edit(&app, "changed").await;

    let at = Timestamp(last + 25 * HOUR);
    app.backup_tick(at).await.unwrap();
    let status = app.backup_status_at(at).unwrap();
    assert!(status.failing);
    assert!(!status.overdue);
    assert_eq!(status.last, Some(last));
    assert_eq!(
        status.last_error.as_deref(),
        Some("the backup folder is unavailable")
    );
    // The failed backup's changes still count.
    assert!(status.changed);
    assert!(!path.exists());

    let status = app.backup_status_at(Timestamp(last + 49 * HOUR)).unwrap();
    assert!(status.overdue);

    // The folder comes back: the next tick succeeds and clears both.
    std::fs::create_dir(&path).unwrap();
    let at = Timestamp(last + 50 * HOUR);
    app.backup_tick(at).await.unwrap();
    let status = app.backup_status_at(at).unwrap();
    assert!(!status.failing && !status.overdue);
    assert_eq!(backups_in(&path).len(), 1);
}

#[tokio::test]
async fn changes_survive_a_relaunch() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let (folder, last) = backing_up(&app).await;
    // Quit before any tick saw the change.
    edit(&app, "changed").await;
    app.quit();
    drop(app);

    let app = install.launch();
    let at = Timestamp(last + 25 * HOUR);
    assert!(app.backup_status_at(at).unwrap().changed);
    app.backup_tick(at).await.unwrap();
    assert_eq!(backups_in(folder.path()).len(), 2);
}

#[tokio::test]
async fn retention_keeps_the_newest() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let (folder, last) = backing_up(&app).await;
    app.set_backup_options(true, Interval::Hourly, 3).unwrap();
    for hour in 1..=4 {
        edit(&app, "changed").await;
        app.backup_tick(Timestamp(last + hour * HOUR))
            .await
            .unwrap();
    }
    let names = backups_in(folder.path());
    assert_eq!(names.len(), 3);
    assert_eq!(
        names.last().unwrap(),
        &backup::file_name(Timestamp(last + 4 * HOUR), 1)
    );
}

#[test]
fn intervals_round_trip() {
    for interval in [Interval::Hourly, Interval::Daily, Interval::Weekly] {
        assert_eq!(Interval::parse(interval.as_str()), Some(interval));
        assert_eq!(
            serde_json::to_value(interval).unwrap(),
            json!(interval.as_str())
        );
    }
    assert_eq!(Interval::parse("monthly"), None);
}
