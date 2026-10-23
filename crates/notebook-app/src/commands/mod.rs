//! Tauri commands: thin wrappers that deserialize, call [`AppState`], and
//! return DTOs or an [`AppError`]. Passphrases and recovery keys are
//! zeroized once the call is done.

use tauri::State;
use zeroize::Zeroizing;

use crate::dto::{
    ExportResultDto, ListEntryDto, PhaseDto, SelectionDto, SettingsDto, SyncStatusDto, TagNodeDto,
};
use crate::editor::OpenedDto;
use crate::error::AppError;
use crate::export::Picker;
use crate::state::AppState;

type Result<T> = std::result::Result<T, AppError>;

/// Returns the version of `notebook-core`.
#[tauri::command]
pub fn core_version() -> &'static str {
    notebook_core::version()
}

// ---- app and account ----

#[tauri::command]
pub async fn app_state(state: State<'_, AppState>) -> Result<PhaseDto> {
    Ok(state.phase())
}

#[tauri::command]
pub async fn sign_in(
    state: State<'_, AppState>,
    server: String,
    username: String,
    passphrase: String,
) -> Result<PhaseDto> {
    let passphrase = Zeroizing::new(passphrase);
    state.sign_in(&server, &username, &passphrase).await
}

#[tauri::command]
pub async fn generate_recovery_key(state: State<'_, AppState>) -> Result<String> {
    Ok(state.generate_recovery_key()?.to_string())
}

#[tauri::command]
pub async fn register(
    state: State<'_, AppState>,
    server: String,
    username: String,
    passphrase: String,
    confirm: String,
    invite_code: String,
) -> Result<PhaseDto> {
    let (passphrase, confirm) = (Zeroizing::new(passphrase), Zeroizing::new(confirm));
    state
        .register(&server, &username, &passphrase, &confirm, &invite_code)
        .await
}

#[tauri::command]
pub async fn recover(
    state: State<'_, AppState>,
    server: String,
    username: String,
    recovery_key: String,
    passphrase: String,
    confirm: String,
) -> Result<PhaseDto> {
    let recovery_key = Zeroizing::new(recovery_key);
    let (passphrase, confirm) = (Zeroizing::new(passphrase), Zeroizing::new(confirm));
    state
        .recover(&server, &username, &recovery_key, &passphrase, &confirm)
        .await
}

#[tauri::command]
pub async fn unlock(state: State<'_, AppState>, passphrase: String) -> Result<PhaseDto> {
    let passphrase = Zeroizing::new(passphrase);
    state.unlock(&passphrase).await
}

#[tauri::command]
pub async fn lock(state: State<'_, AppState>) -> Result<PhaseDto> {
    state.lock().await
}

#[tauri::command]
pub async fn reauthenticate(state: State<'_, AppState>, passphrase: String) -> Result<()> {
    let passphrase = Zeroizing::new(passphrase);
    state.reauthenticate(&passphrase).await
}

#[tauri::command]
pub async fn change_passphrase(
    state: State<'_, AppState>,
    current: String,
    new: String,
    confirm: String,
) -> Result<()> {
    let (current, new, confirm) = (
        Zeroizing::new(current),
        Zeroizing::new(new),
        Zeroizing::new(confirm),
    );
    state.change_passphrase(&current, &new, &confirm).await
}

#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>, confirm: bool) -> Result<PhaseDto> {
    state.sign_out(confirm).await
}

#[tauri::command]
pub async fn pending_count(state: State<'_, AppState>) -> Result<usize> {
    state.pending_count()
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> Result<SyncStatusDto> {
    state.sync_status()
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
    state.delete_permanently(&id).await
}

#[tauri::command]
pub async fn rename_tag(state: State<'_, AppState>, old: String, new: String) -> Result<usize> {
    state.rename_tag(&old, &new)
}

/// Opens the system folder picker and exports into the chosen folder.
/// `null` when the user cancels.
#[tauri::command]
pub async fn export_notes(
    state: State<'_, AppState>,
    picker: State<'_, Picker>,
    include_trashed: bool,
) -> Result<Option<ExportResultDto>> {
    state.export_notes(&*picker.0, include_trashed).await
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
pub async fn set_tag_expanded(
    state: State<'_, AppState>,
    path: String,
    expanded: bool,
) -> Result<()> {
    state.set_tag_expanded(&path, expanded)
}
