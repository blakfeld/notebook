//! Tauri host for the Notebook frontend.
//!
//! The webview is a view: keys, decrypted notes, storage, sync, and
//! networking all live here, in [`state::AppState`].

mod commands;
mod credentials;
mod dto;
mod editor;
mod error;
mod events;
mod export;
#[cfg(test)]
mod integration_tests;
mod notes;
mod state;
mod vault;
#[cfg(test)]
mod vault_tests;

use std::path::Path;
use std::sync::Arc;

use notebook_core::auth::Device;
use notebook_core::auth::api::SessionKind;
use tauri::{Builder, Context, Manager, Runtime};

use crate::credentials::KeyringStore;
use crate::events::TauriEmitter;
use crate::state::{AppConfig, AppState};

/// Registers every command on `builder`, generic over the runtime so tests can
/// drive it on the mock runtime.
fn register<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        commands::core_version,
        commands::app_state,
        commands::sign_in,
        commands::generate_recovery_key,
        commands::register,
        commands::recover,
        commands::unlock,
        commands::lock,
        commands::reauthenticate,
        commands::change_passphrase,
        commands::sign_out,
        commands::pending_count,
        commands::sync_status,
        commands::tag_tree,
        commands::list_notes,
        commands::create_note,
        commands::set_pinned,
        commands::set_archived,
        commands::set_trashed,
        commands::delete_permanently,
        commands::rename_tag,
        commands::export_notes,
        commands::editor_open,
        commands::editor_push,
        commands::editor_close,
        commands::get_settings,
        commands::set_vim_mode,
        commands::set_preference,
        commands::set_tag_expanded,
    ])
}

/// The app's generated context. `generate_context!` may only expand once per
/// crate, so `run` and the tests share this.
fn context<R: Runtime>() -> Context<R> {
    tauri::generate_context!()
}

/// This install's id, which scopes its keychain entries. Not secret; kept
/// next to the database and created on first launch.
fn install_id(data_dir: &Path) -> std::io::Result<String> {
    let path = data_dir.join("install-id");
    match std::fs::read_to_string(&path) {
        Ok(id) if !id.trim().is_empty() => Ok(id.trim().to_owned()),
        Ok(_) => write_install_id(&path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => write_install_id(&path),
        Err(err) => Err(err),
    }
}

fn write_install_id(path: &Path) -> std::io::Result<String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(std::io::Error::other)?;
    let id = ulid::Ulid::from_bytes(random).to_string();
    std::fs::write(path, &id)?;
    Ok(id)
}

fn device() -> Device {
    Device {
        name: format!("Notebook on {}", std::env::consts::OS),
        kind: SessionKind::Native,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    register(Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let creds = Arc::new(KeyringStore::new(&install_id(&data_dir)?));
            let config = AppConfig {
                data_dir,
                sync: Default::default(),
                device: device(),
            };
            let emitter = Arc::new(TauriEmitter(app.handle().clone()));
            app.manage(AppState::launch(config, creds, emitter));
            // Mobile exports through the share sheet (`mobile-app`).
            #[cfg(desktop)]
            app.manage(export::Picker(Arc::new(export::DialogPicker(
                app.handle().clone(),
            ))));
            Ok(())
        })
        .run(context())
        .expect("error while running Notebook");
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{
        INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
    };
    use tauri::webview::{InvokeRequest, WebviewWindowBuilder};

    /// The origin Tauri serves local app content from on this platform.
    const LOCAL_ORIGIN: &str = if cfg!(any(windows, target_os = "android")) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };

    fn invoke(cmd: &str, body: InvokeBody) -> Result<String, serde_json::Value> {
        invoke_with(mock_context(noop_assets()), LOCAL_ORIGIN, cmd, body)
    }

    fn invoke_with(
        context: Context<MockRuntime>,
        url: &str,
        cmd: &str,
        body: InvokeBody,
    ) -> Result<String, serde_json::Value> {
        let app = register(mock_builder())
            .build(context)
            .expect("mock app builds");
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("mock webview builds");
        get_ipc_response(
            &webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().unwrap(),
                body,
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|response| response.deserialize::<String>().expect("string response"))
    }

    #[test]
    fn core_version_returns_notebook_core_version() {
        assert_eq!(commands::core_version(), notebook_core::version());
        assert!(!commands::core_version().is_empty());
    }

    #[test]
    fn core_version_is_invocable_over_ipc() {
        assert_eq!(
            invoke("core_version", InvokeBody::default()),
            Ok(notebook_core::version().to_owned())
        );
    }

    #[test]
    fn unregistered_command_is_rejected() {
        assert!(invoke("not_a_command", InvokeBody::default()).is_err());
    }

    #[test]
    fn core_version_is_invocable_with_the_app_context() {
        assert_eq!(
            invoke_with(
                context(),
                LOCAL_ORIGIN,
                "core_version",
                InvokeBody::default()
            ),
            Ok(notebook_core::version().to_owned())
        );
    }

    #[test]
    fn remote_origin_is_rejected() {
        // The same command and context succeed locally, so the rejection below
        // can only come from the origin.
        assert!(
            invoke_with(
                context(),
                LOCAL_ORIGIN,
                "core_version",
                InvokeBody::default()
            )
            .is_ok()
        );

        let err = invoke_with(
            context(),
            "https://example.com",
            "core_version",
            InvokeBody::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("not allowed"),
            "expected an ACL rejection, got: {err}"
        );
    }

    // ---- commands over IPC, with a managed AppState ----

    use serde_json::{Value, json};

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::future::BoxFuture;

    use crate::export::{FolderPicker, Picker};
    use crate::state::tests::{Install, PASSPHRASE, TestServer, notes, registered};

    /// A folder picker that answers `folder` and counts how often it opened.
    struct StubPicker {
        folder: Option<PathBuf>,
        opened: AtomicUsize,
    }

    impl StubPicker {
        fn new(folder: Option<PathBuf>) -> Arc<Self> {
            Arc::new(Self {
                folder,
                opened: AtomicUsize::new(0),
            })
        }

        fn opened(&self) -> usize {
            self.opened.load(Ordering::SeqCst)
        }
    }

    impl FolderPicker for StubPicker {
        fn pick(&self) -> BoxFuture<'static, Option<PathBuf>> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(self.folder.clone()))
        }
    }

    /// A mock app managing `state`, and its main webview. Its folder picker
    /// is always cancelled.
    pub(crate) fn mock_app(
        state: AppState,
    ) -> (tauri::App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
        mock_app_with_picker(state, StubPicker::new(None))
    }

    fn mock_app_with_picker(
        state: AppState,
        picker: Arc<StubPicker>,
    ) -> (tauri::App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
        let app = register(mock_builder())
            .manage(state)
            .manage(Picker(picker))
            .build(mock_context(noop_assets()))
            .expect("mock app builds");
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("mock webview builds");
        (app, webview)
    }

    /// Invokes `cmd` with JSON arguments (camelCase, as the frontend sends).
    pub(crate) fn call(
        webview: &tauri::WebviewWindow<MockRuntime>,
        cmd: &str,
        args: Value,
    ) -> Result<Value, Value> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: LOCAL_ORIGIN.parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|response| response.deserialize::<Value>().expect("json response"))
    }

    #[tokio::test]
    async fn app_state_for_each_launch_case() {
        // No entry: signed out.
        let install = Install::new();
        let (_app, webview) = mock_app(install.launch());
        assert_eq!(
            call(&webview, "app_state", json!({})).unwrap()["phase"],
            "signed_out"
        );

        // Session and auto-unlock entries: unlocked without a passphrase.
        let server = TestServer::start().await;
        let install = Install::new();
        registered(&server, &install, "alice").await.quit().await;
        let (_app, webview) = mock_app(install.launch());
        let phase = call(&webview, "app_state", json!({})).unwrap();
        assert_eq!(phase["phase"], "unlocked");
        assert_eq!(phase["account"]["username"], "alice");
        assert_eq!(phase["credentials_remembered"], true);

        // Session without auto-unlock: locked.
        call(&webview, "lock", json!({})).unwrap();
        let (_app, webview) = mock_app(install.launch());
        assert_eq!(
            call(&webview, "app_state", json!({})).unwrap()["phase"],
            "locked"
        );
        let phase = call(&webview, "unlock", json!({"passphrase": PASSPHRASE})).unwrap();
        assert_eq!(phase["phase"], "unlocked");
    }

    #[tokio::test]
    async fn errors_cross_ipc_with_their_code() {
        let install = Install::new();
        let (_app, webview) = mock_app(install.launch());
        let err = call(
            &webview,
            "sign_in",
            json!({"server": "http://notes.example.com", "username": "a", "passphrase": "b"}),
        )
        .unwrap_err();
        assert_eq!(err["code"], "invalid_server_url");
        let err = call(
            &webview,
            "list_notes",
            json!({"selection": {"view": "notes"}, "query": ""}),
        )
        .unwrap_err();
        assert_eq!(err["code"], "locked");
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn export_is_refused_while_locked() {
        let install = Install::new();
        let parent = tempfile::tempdir().unwrap();
        let picker = StubPicker::new(Some(parent.path().to_owned()));
        let (_app, webview) = mock_app_with_picker(install.launch(), picker.clone());
        let err = call(&webview, "export_notes", json!({"includeTrashed": false})).unwrap_err();
        assert_eq!(err["code"], "locked");
        assert_eq!(picker.opened(), 0);
        assert!(file_names(parent.path()).is_empty());
    }

    #[tokio::test]
    async fn export_writes_into_the_picked_folder() {
        let server = TestServer::start().await;
        let install = Install::new();
        let state = registered(&server, &install, "alice").await;
        let mut ids = Vec::new();
        for body in ["Ideas", "Ideas", "Old #work/acme", "Gone"] {
            let id = state.create_note(notes()).unwrap();
            let note = dto::parse_note_id(&id).unwrap();
            state
                .with_vault(|u| Ok(u.workspace.set_body(note, body, state::now())?))
                .unwrap();
            ids.push(id);
        }
        state.set_archived(&ids[2], true).unwrap();
        state.set_trashed(&ids[3], true).unwrap();

        let parent = tempfile::tempdir().unwrap();
        let picker = StubPicker::new(Some(parent.path().to_owned()));
        let (_app, webview) = mock_app_with_picker(state, picker.clone());

        let result = call(&webview, "export_notes", json!({"includeTrashed": false})).unwrap();
        assert_eq!(result["count"], 3);
        let folder = PathBuf::from(result["path"].as_str().unwrap());
        assert_eq!(folder.parent(), Some(parent.path()));
        let name = folder.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("notebook-export-"), "{name}");
        assert_eq!(
            file_names(&folder),
            ["Ideas 2.md", "Ideas.md", "Old work acme.md"]
        );
        let archived = std::fs::read_to_string(folder.join("Old work acme.md")).unwrap();
        assert!(archived.contains("\narchived: true\n"));
        assert!(archived.ends_with("---\nOld #work/acme"));

        let result = call(&webview, "export_notes", json!({"includeTrashed": true})).unwrap();
        assert_eq!(result["count"], 4);
        let folder = PathBuf::from(result["path"].as_str().unwrap());
        assert!(folder.join("Gone.md").exists());
        assert_eq!(file_names(parent.path()).len(), 2);
        assert_eq!(picker.opened(), 2);
    }

    #[tokio::test]
    async fn cancelled_export_writes_nothing() {
        let server = TestServer::start().await;
        let install = Install::new();
        let state = registered(&server, &install, "alice").await;
        let (_app, webview) = mock_app(state);
        let result = call(&webview, "export_notes", json!({"includeTrashed": false})).unwrap();
        assert_eq!(result, Value::Null);
    }
}
