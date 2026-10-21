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
}
