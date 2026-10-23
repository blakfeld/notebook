//! Vault operations over the open workspace. All of them fail with
//! `not_open` unless the notebook is open.

use crate::dto::{ListEntryDto, SelectionDto, TagNodeDto, parse_note_id};
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

    /// Deletes a trashed note from this device at once. Backups taken
    /// earlier still contain it.
    pub fn delete_permanently(&self, id: &str) -> AppResult<()> {
        let id = parse_note_id(id)?;
        self.with_vault(|u| Ok(u.workspace.delete_permanently(id)?))
    }

    /// Returns how many notes were rewritten.
    pub fn rename_tag(&self, old: &str, new: &str) -> AppResult<usize> {
        self.with_vault(|u| Ok(u.workspace.rename_tag(old, new, now())?))
    }
}
