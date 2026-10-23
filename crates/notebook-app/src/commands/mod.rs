//! Tauri commands: thin wrappers that deserialize, call [`AppState`], and
//! return DTOs or an [`AppError`]. Passphrases and recovery keys are
//! zeroized once the call is done.

use tauri::State;
use zeroize::Zeroizing;

use crate::dto::{ListEntryDto, PhaseDto, SelectionDto, SettingsDto, TagNodeDto};
use crate::editor::OpenedDto;
use crate::error::AppError;
use crate::state::AppState;

type Result<T> = std::result::Result<T, AppError>;

/// Returns the version of `notebook-core`.
#[tauri::command]
pub fn core_version() -> &'static str {
    notebook_core::version()
}

// ---- app and notebook ----

#[tauri::command]
pub async fn app_state(state: State<'_, AppState>) -> Result<PhaseDto> {
    Ok(state.phase())
}

/// Returns the new recovery key's display form.
#[tauri::command]
pub async fn create_notebook(state: State<'_, AppState>) -> Result<String> {
    Ok(state.create_notebook().await?.to_string())
}

/// The recovery key waiting to be acknowledged, e.g. after an upgrade
/// generated it at launch.
#[tauri::command]
pub async fn pending_recovery_key(state: State<'_, AppState>) -> Result<String> {
    Ok(state.pending_recovery_key()?.to_string())
}

#[tauri::command]
pub async fn acknowledge_recovery_key(state: State<'_, AppState>) -> Result<PhaseDto> {
    state.acknowledge_recovery_key().await
}

#[tauri::command]
pub async fn open_with_recovery_key(
    state: State<'_, AppState>,
    recovery_key: String,
) -> Result<PhaseDto> {
    let recovery_key = Zeroizing::new(recovery_key);
    state.open_with_recovery_key(&recovery_key).await
}

#[tauri::command]
pub async fn upgrade_with_passphrase(
    state: State<'_, AppState>,
    passphrase: String,
) -> Result<PhaseDto> {
    let passphrase = Zeroizing::new(passphrase);
    state.upgrade_with_passphrase(&passphrase).await
}

#[tauri::command]
pub async fn start_over(state: State<'_, AppState>) -> Result<PhaseDto> {
    state.start_over().await
}

// ---- vault ----

#[tauri::command]
pub async fn tag_tree(state: State<'_, AppState>) -> Result<Vec<TagNodeDto>> {
    state.tag_tree()
}

#[tauri::command]
pub async fn list_notes(
    state: State<'_, AppState>,
    selection: SelectionDto,
    query: String,
) -> Result<Vec<ListEntryDto>> {
    state.list_notes(selection, &query)
}

#[tauri::command]
pub async fn create_note(state: State<'_, AppState>, selection: SelectionDto) -> Result<String> {
    state.create_note(selection)
}

#[tauri::command]
pub async fn set_pinned(state: State<'_, AppState>, id: String, pinned: bool) -> Result<()> {
    state.set_pinned(&id, pinned)
}

#[tauri::command]
pub async fn set_archived(state: State<'_, AppState>, id: String, archived: bool) -> Result<()> {
    state.set_archived(&id, archived)
}

#[tauri::command]
pub async fn set_trashed(state: State<'_, AppState>, id: String, trashed: bool) -> Result<()> {
    state.set_trashed(&id, trashed)
}

#[tauri::command]
pub async fn delete_permanently(state: State<'_, AppState>, id: String) -> Result<()> {
    state.delete_permanently(&id)
}

#[tauri::command]
pub async fn rename_tag(state: State<'_, AppState>, old: String, new: String) -> Result<usize> {
    state.rename_tag(&old, &new)
}

// ---- editor ----

#[tauri::command]
pub async fn editor_open(state: State<'_, AppState>, note: String) -> Result<OpenedDto> {
    state.editor_open(&note)
}

#[tauri::command]
pub async fn editor_push(state: State<'_, AppState>, session: u64, update: String) -> Result<()> {
    state.editor_push(session, &update)
}

#[tauri::command]
pub async fn editor_close(state: State<'_, AppState>, session: u64) -> Result<()> {
    state.editor_close(session)
}

// ---- settings ----

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<SettingsDto> {
    state.settings()
}

#[tauri::command]
pub async fn set_vim_mode(state: State<'_, AppState>, on: bool) -> Result<()> {
    state.set_vim_mode(on)
}

#[tauri::command]
pub async fn set_preference(state: State<'_, AppState>, key: String, value: String) -> Result<()> {
    state.set_preference(&key, &value)
}

#[tauri::command]
pub async fn set_tag_expanded(
    state: State<'_, AppState>,
    path: String,
    expanded: bool,
) -> Result<()> {
    state.set_tag_expanded(&path, expanded)
}
