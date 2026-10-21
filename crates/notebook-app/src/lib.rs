//! Tauri host for the Notebook frontend.

/// Returns the version of `notebook-core`.
#[tauri::command]
fn core_version() -> &'static str {
    notebook_core::version()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_version])
        .run(tauri::generate_context!())
        .expect("error while running Notebook");
}
