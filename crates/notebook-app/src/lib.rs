//! Tauri host for the Notebook frontend.

use tauri::{Builder, Context, Runtime};

/// Returns the version of `notebook-core`.
#[tauri::command]
fn core_version() -> &'static str {
    notebook_core::version()
}

/// Registers every command on `builder`, generic over the runtime so tests can
/// drive it on the mock runtime.
fn register<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.invoke_handler(tauri::generate_handler![core_version])
}

/// The app's generated context. `generate_context!` may only expand once per
/// crate, so `run` and the tests share this.
fn context<R: Runtime>() -> Context<R> {
    tauri::generate_context!()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    register(Builder::default())
        .run(context())
        .expect("error while running Notebook");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{
        INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
    };
    use tauri::webview::{InvokeRequest, WebviewWindowBuilder};

    fn invoke(cmd: &str, body: InvokeBody) -> Result<String, serde_json::Value> {
        invoke_with(mock_context(noop_assets()), "tauri://localhost", cmd, body)
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
        assert_eq!(core_version(), notebook_core::version());
        assert!(!core_version().is_empty());
    }

    #[test]
    fn core_version_is_invocable_over_ipc() {
        assert_eq!(
            invoke("core_version", InvokeBody::default()),
            Ok(notebook_core::version().to_owned())
        );
    }

    #[test]
    fn core_version_ignores_unexpected_arguments() {
        let body = InvokeBody::Json(serde_json::json!({ "extra": 1 }));
        assert_eq!(
            invoke("core_version", body),
            Ok(notebook_core::version().to_owned())
        );
    }

    #[test]
    fn core_version_accepts_raw_body() {
        let body = InvokeBody::Raw(vec![1, 2, 3]);
        assert_eq!(
            invoke("core_version", body),
            Ok(notebook_core::version().to_owned())
        );
    }

    #[test]
    fn unregistered_command_is_rejected() {
        assert!(invoke("not_a_command", InvokeBody::default()).is_err());
    }

    #[test]
    fn command_names_are_case_sensitive() {
        assert!(invoke("Core_Version", InvokeBody::default()).is_err());
    }

    #[test]
    fn core_version_is_invocable_with_the_app_context() {
        assert_eq!(
            invoke_with(
                context(),
                "tauri://localhost",
                "core_version",
                InvokeBody::default()
            ),
            Ok(notebook_core::version().to_owned())
        );
    }

    #[test]
    fn remote_origin_is_rejected() {
        assert!(
            invoke_with(
                mock_context(noop_assets()),
                "https://example.com",
                "core_version",
                InvokeBody::default()
            )
            .is_err()
        );
    }
}
