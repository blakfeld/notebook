//! Serializable command inputs and results. Note ids cross as ULID strings,
//! times as Unix milliseconds.

use notebook_core::note::NoteId;
use notebook_core::tags::{TagNode, TagPath, View};
use notebook_core::workspace::{Changed, ListEntry, Selection};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult, Code};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseName {
    /// No notebook on this device: the welcome screen.
    Setup,
    /// A recovery key was generated and is waiting to be acknowledged.
    PendingRecoveryKey,
    /// A notebook exists but its key isn't in the secure store.
    NeedsRecoveryKey,
    /// Upgrading a locked server-backed install: the old passphrase opens it.
    NeedsOldPassphrase,
    Open,
}

/// The `app_state` result and the `app-phase-changed` payload. Never holds
/// secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseDto {
    pub phase: PhaseName,
    /// False when the OS has no usable secure store, so the vault key isn't
    /// remembered and the recovery key is asked for at each launch.
    pub secure_store: bool,
    /// True while converting a server-backed install, so the screens can
    /// say that the old recovery key no longer matters.
    pub upgrade: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewDto {
    Notes,
    Untagged,
    Pinned,
    Archive,
    Trash,
}

impl From<ViewDto> for View {
    fn from(view: ViewDto) -> Self {
        match view {
            ViewDto::Notes => View::Notes,
            ViewDto::Untagged => View::Untagged,
            ViewDto::Pinned => View::Pinned,
            ViewDto::Archive => View::Archive,
            ViewDto::Trash => View::Trash,
        }
    }
}

/// `{"view": "notes"}` or `{"tag": "work/acme"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionDto {
    View(ViewDto),
    Tag(String),
}

impl SelectionDto {
    pub fn parse(self) -> AppResult<Selection> {
        Ok(match self {
            Self::View(view) => Selection::View(view.into()),
            Self::Tag(path) => Selection::Tag(
                TagPath::parse(&path)
                    .ok_or_else(|| AppError::new(Code::InvalidTagName, "invalid tag"))?,
            ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagNodeDto {
    pub name: String,
    /// The full path in display spelling, as used in a selection.
    pub path: String,
    pub count: usize,
    pub children: Vec<TagNodeDto>,
}

impl From<&TagNode> for TagNodeDto {
    fn from(node: &TagNode) -> Self {
        Self {
            name: node.name.clone(),
            path: node.path.to_string(),
            count: node.notes.len(),
            children: node.children.iter().map(Self::from).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListEntryDto {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub updated: i64,
    pub pinned: bool,
    pub archived: bool,
    pub trashed: bool,
}

impl From<ListEntry> for ListEntryDto {
    fn from(entry: ListEntry) -> Self {
        Self {
            id: entry.id.to_string(),
            title: entry.title,
            preview: entry.preview,
            updated: entry.updated.as_millis(),
            pinned: entry.pinned,
            archived: entry.archived,
            trashed: entry.trashed,
        }
    }
}

pub fn parse_note_id(id: &str) -> AppResult<NoteId> {
    id.parse()
        .map_err(|_| AppError::new(Code::NotFound, "malformed note id"))
}

/// The `export_notes` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportResultDto {
    /// How many notes were exported.
    pub count: usize,
    /// The export folder that was written.
    pub path: String,
}

/// The `vault-changed` payload: ids only; the frontend re-queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultChangedDto {
    pub changed: Vec<String>,
    pub tree_changed: bool,
}

impl From<&Changed> for VaultChangedDto {
    fn from(changed: &Changed) -> Self {
        Self {
            changed: changed.notes.iter().map(NoteId::to_string).collect(),
            tree_changed: changed.tree_changed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsDto {
    pub vim_mode: bool,
    /// `system`, `light`, or `dark`.
    pub theme: String,
    pub source_view: bool,
    pub focus_mode: bool,
    /// Tag paths (lowercased) the user collapsed in the sidebar.
    pub collapsed_tags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selections_parse_from_json() {
        let view: SelectionDto = serde_json::from_value(json!({"view": "trash"})).unwrap();
        assert_eq!(view.parse().unwrap(), Selection::View(View::Trash));
        let tag: SelectionDto = serde_json::from_value(json!({"tag": "work/acme"})).unwrap();
        assert_eq!(
            tag.parse().unwrap(),
            Selection::Tag(TagPath::parse("work/acme").unwrap())
        );
        let bad: SelectionDto = serde_json::from_value(json!({"tag": "42"})).unwrap();
        assert_eq!(bad.parse().unwrap_err().code, Code::InvalidTagName);
    }
}
