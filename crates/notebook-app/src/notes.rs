//! Vault operations over the unlocked workspace. All of them fail with
//! `locked` unless the vault is open.

use crate::dto::{ListEntryDto, SelectionDto, SyncStatusDto, TagNodeDto, parse_note_id};
use crate::error::AppResult;
use crate::state::{AppState, now};

impl AppState {
    pub fn tag_tree(&self) -> AppResult<Vec<TagNodeDto>> {
        self.with_vault(|u| {
            Ok(u.workspace
                .tree()
                .roots()
                .iter()
                .map(TagNodeDto::from)
                .collect())
        })
    }

    pub fn list_notes(&self, selection: SelectionDto, query: &str) -> AppResult<Vec<ListEntryDto>> {
        let selection = selection.parse()?;
        self.with_vault(|u| {
            Ok(u.workspace
                .list(&selection, query)
                .into_iter()
                .map(ListEntryDto::from)
                .collect())
        })
    }

    /// Returns the new note's id.
    pub fn create_note(&self, selection: SelectionDto) -> AppResult<String> {
        let selection = selection.parse()?;
        self.with_vault(|u| Ok(u.workspace.create(&selection, now())?.to_string()))
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> AppResult<()> {
        let id = parse_note_id(id)?;
        self.with_vault(|u| Ok(u.workspace.set_pinned(id, pinned)?))
    }

    pub fn set_archived(&self, id: &str, archived: bool) -> AppResult<()> {
        let id = parse_note_id(id)?;
        self.with_vault(|u| Ok(u.workspace.set_archived(id, archived)?))
    }

    pub fn set_trashed(&self, id: &str, trashed: bool) -> AppResult<()> {
        let id = parse_note_id(id)?;
        self.with_vault(|u| Ok(u.workspace.set_trashed(id, trashed)?))
    }

    /// Deletes a trashed note on the server and then here. Needs a
    /// connection; offline it fails with `offline` and nothing changes.
    pub async fn delete_permanently(&self, id: &str) -> AppResult<()> {
        let id = parse_note_id(id)?;
        let purge = self.with_vault(|u| Ok(u.workspace.purge(id)?))?;
        purge.await?;
        // Locked meanwhile: the next unlock reads the store fresh.
        let _ = self.with_vault(|u| {
            u.workspace.refresh(&[id]);
            // Freed space: a push paused by a full quota can retry now.
            u.workspace.sync().wake();
            Ok(())
        });
        Ok(())
    }

    /// Returns how many notes were rewritten.
    pub fn rename_tag(&self, old: &str, new: &str) -> AppResult<usize> {
        self.with_vault(|u| Ok(u.workspace.rename_tag(old, new, now())?))
    }

    pub fn sync_status(&self) -> AppResult<SyncStatusDto> {
        self.with_vault(|u| Ok(u.status.clone()))
    }
}
