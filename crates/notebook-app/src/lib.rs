//! Tauri host for the Notebook frontend.
//!
//! The webview is a view: keys, decrypted notes, and storage all live here,
//! in [`state::AppState`]. Nothing here uses the network.

mod commands;
mod credentials;
mod dto;
mod editor;
mod error;
mod events;
mod notes;
mod state;
mod vault;
#[cfg(test)]
mod vault_tests;

use std::path::Path;
use std::sync::Arc;

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
        commands::create_notebook,
        commands::pending_recovery_key,
        commands::acknowledge_recovery_key,
        commands::open_with_recovery_key,
        commands::upgrade_with_passphrase,
        commands::start_over,
        commands::tag_tree,
        commands::list_notes,
        commands::create_note,
        commands::set_pinned,
        commands::set_archived,
        commands::set_trashed,
        commands::delete_permanently,
        commands::rename_tag,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    register(Builder::default())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let creds = Arc::new(KeyringStore::new(&install_id(&data_dir)?));
            let config = AppConfig { data_dir };
            let emitter = Arc::new(TauriEmitter(app.handle().clone()));
            app.manage(AppState::launch(config, creds, emitter));
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

    use crate::credentials::{self, CredentialStore};
    use crate::state::tests::Install;

    /// A mock app managing `state`, and its main webview.
    pub(crate) fn mock_app(
        state: AppState,
    ) -> (tauri::App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
        let app = register(mock_builder())
            .manage(state)
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
        // Nothing on the device: the welcome screen.
        let install = Install::new();
        let (_app, webview) = mock_app(install.launch());
        let phase = call(&webview, "app_state", json!({})).unwrap();
        assert_eq!(
            phase,
            json!({"phase": "setup", "secure_store": true, "upgrade": false})
        );

        // Create, read the key back while pending, acknowledge.
        let key = call(&webview, "create_notebook", json!({})).unwrap();
        assert_eq!(key.as_str().unwrap().len(), 39);
        assert_eq!(
            call(&webview, "pending_recovery_key", json!({})).unwrap(),
            key
        );
        let phase = call(&webview, "acknowledge_recovery_key", json!({})).unwrap();
        assert_eq!(phase["phase"], "open");
        assert_eq!(
            call(&webview, "pending_recovery_key", json!({})).unwrap_err()["code"],
            "not_open"
        );
        drop(webview);
        drop(_app);

        // Relaunch: open with no prompt.
        let (_app, webview) = mock_app(install.launch());
        assert_eq!(
            call(&webview, "app_state", json!({})).unwrap()["phase"],
            "open"
        );

        // Without the keychain entry: the recovery key opens it.
        install.creds.delete(credentials::Entry::VaultKey).unwrap();
        let (_app, webview) = mock_app(install.launch());
        assert_eq!(
            call(&webview, "app_state", json!({})).unwrap()["phase"],
            "needs_recovery_key"
        );
        let phase = call(
            &webview,
            "open_with_recovery_key",
            json!({"recoveryKey": key}),
        )
        .unwrap();
        assert_eq!(phase["phase"], "open");
    }

    #[tokio::test]
    async fn errors_cross_ipc_with_their_code() {
        let install = Install::new();
        let (_app, webview) = mock_app(install.launch());
        let err = call(
            &webview,
            "list_notes",
            json!({"selection": {"view": "notes"}, "query": ""}),
        )
        .unwrap_err();
        assert_eq!(err["code"], "not_open");
        let err = call(
            &webview,
            "open_with_recovery_key",
            json!({"recoveryKey": "not a key"}),
        )
        .unwrap_err();
        assert_eq!(err["code"], "recovery_key_malformed");
    }

    #[test]
    fn removed_account_and_sync_commands_are_gone() {
        for cmd in [
            "sign_in",
            "register",
            "recover",
            "unlock",
            "lock",
            "reauthenticate",
            "change_passphrase",
            "sign_out",
            "pending_count",
            "sync_status",
            "generate_recovery_key",
        ] {
            assert!(invoke(cmd, InvokeBody::default()).is_err(), "{cmd}");
        }
    }
}
