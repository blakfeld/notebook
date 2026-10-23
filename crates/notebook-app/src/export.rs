//! "Export all notes": the vault as markdown files in a folder the user
//! picks. The picker runs here, not in the webview, so the webview never
//! names a path that plaintext is written to.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use notebook_core::export::{self, ExportOptions};

use crate::dto::ExportResultDto;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, now};

/// Asks the user for the folder to export into.
pub trait FolderPicker: Send + Sync {
    /// The chosen folder, or `None` when the user cancels.
    fn pick(&self) -> BoxFuture<'static, Option<PathBuf>>;
}

/// The picker the `export_notes` command uses, as managed state.
#[derive(Clone)]
pub struct Picker(pub Arc<dyn FolderPicker>);

/// The system folder picker, through `tauri-plugin-dialog`. Desktop only:
/// the plugin has no folder picker on mobile, which shares a zip instead.
#[cfg(desktop)]
pub struct DialogPicker<R: tauri::Runtime>(pub tauri::AppHandle<R>);

#[cfg(desktop)]
impl<R: tauri::Runtime> FolderPicker for DialogPicker<R> {
    fn pick(&self) -> BoxFuture<'static, Option<PathBuf>> {
        use futures::FutureExt;
        use tauri_plugin_dialog::DialogExt;

        let (tx, rx) = futures::channel::oneshot::channel();
        self.0
            .dialog()
            .file()
            .set_title("Export all notes")
            .pick_folder(move |folder| {
                let _ = tx.send(folder.and_then(|f| f.into_path().ok()));
            });
        rx.map(|picked| picked.ok().flatten()).boxed()
    }
}

impl AppState {
    /// Exports every note (trashed ones too with `include_trashed`) into a
    /// new folder inside the one the user picks. `None` when they cancel.
    pub async fn export_notes(
        &self,
        picker: &dyn FolderPicker,
        include_trashed: bool,
    ) -> AppResult<Option<ExportResultDto>> {
        self.with_vault(|_| Ok(()))?;
        let Some(parent) = picker.pick().await else {
            return Ok(None);
        };
        // Read after the picker closes, so edits made meanwhile are in, and a
        // vault locked meanwhile fails with `locked`.
        let notes = self.with_vault(|u| Ok(u.workspace.all_notes()))?;
        let plan = export::plan(&notes, ExportOptions { include_trashed }, now());
        drop(notes);
        let count = plan.files.len();
        let path = tauri::async_runtime::spawn_blocking(move || export::write_dir(&plan, &parent))
            .await
            .map_err(AppError::internal)??;
        Ok(Some(ExportResultDto {
            count,
            path: path.display().to_string(),
        }))
    }
}
