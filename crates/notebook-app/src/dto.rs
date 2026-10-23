//! Serializable command inputs and results. Note ids cross as ULID strings,
//! times as Unix milliseconds.

use notebook_core::note::NoteId;
use notebook_core::sync::{Attention, SyncState, SyncStatus};
use notebook_core::tags::{TagNode, TagPath, View};
use notebook_core::workspace::{Changed, ListEntry, Selection};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult, Code};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseName {
    SignedOut,
    Locked,
    Unlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountDto {
    pub server: String,
    pub username: String,
}

/// The `app_state` result and the `app-phase-changed` payload. Never holds
/// secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseDto {
    pub phase: PhaseName,
    pub account: Option<AccountDto>,
    /// False when there is no OS secure store, so the user will have to sign
    /// in again next launch.
    pub credentials_remembered: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStateDto {
    Synced,
    Syncing,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionDto {
    SignInRequired,
    UpgradeRequired,
    QuotaFull,
    UpdateTooLarge,
    PushBlocked,
    IntegrityWarning,
}

/// The `sync-status` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncStatusDto {
    pub state: SyncStateDto,
    pub pending: usize,
    pub attention: Vec<AttentionDto>,
}

impl From<&SyncStatus> for SyncStatusDto {
    fn from(status: &SyncStatus) -> Self {
        let state = match status.state {
            SyncState::Synced => SyncStateDto::Synced,
            SyncState::Syncing => SyncStateDto::Syncing,
            SyncState::Offline => SyncStateDto::Offline,
        };
        let mut attention: Vec<AttentionDto> = status
            .attention
            .iter()
            .map(|a| match a {
                Attention::SignInRequired => AttentionDto::SignInRequired,
                Attention::UpgradeRequired => AttentionDto::UpgradeRequired,
                Attention::QuotaFull => AttentionDto::QuotaFull,
                Attention::UpdateTooLarge(_) => AttentionDto::UpdateTooLarge,
                Attention::PushBlocked => AttentionDto::PushBlocked,
            })
            .collect();
        if status.integrity_warnings > 0 {
            attention.push(AttentionDto::IntegrityWarning);
        }
        Self {
            state,
            pending: status.pending,
            attention,
        }
    }
}

impl SyncStatusDto {
    /// Before the engine has said anything.
    pub fn starting(pending: usize) -> Self {
        Self {
            state: SyncStateDto::Syncing,
            pending,
            attention: Vec::new(),
        }
    }
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
    use notebook_core::sync::SyncState;
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

    #[test]
    fn sync_status_payloads() {
        let status = |state, pending, attention, integrity_warnings| SyncStatus {
            state,
            pending,
            attention,
            integrity_warnings,
        };
        assert_eq!(
            serde_json::to_value(SyncStatusDto::from(&status(SyncState::Offline, 2, None, 0)))
                .unwrap(),
            json!({"state": "offline", "pending": 2, "attention": []})
        );
        assert_eq!(
            serde_json::to_value(SyncStatusDto::from(&status(
                SyncState::Synced,
                0,
                Some(Attention::UpgradeRequired),
                0
            )))
            .unwrap(),
            json!({"state": "synced", "pending": 0, "attention": ["upgrade_required"]})
        );
        assert_eq!(
            serde_json::to_value(SyncStatusDto::from(&status(
                SyncState::Syncing,
                3,
                Some(Attention::QuotaFull),
                1
            )))
            .unwrap(),
            json!({"state": "syncing", "pending": 3, "attention": ["quota_full", "integrity_warning"]})
        );
        let large = status(
            SyncState::Syncing,
            1,
            Some(Attention::UpdateTooLarge(NoteId::from_parts(1, 1))),
            0,
        );
        assert_eq!(
            SyncStatusDto::from(&large).attention,
            [AttentionDto::UpdateTooLarge]
        );
        let signed_out = status(SyncState::Offline, 1, Some(Attention::SignInRequired), 0);
        assert_eq!(
            SyncStatusDto::from(&signed_out).attention,
            [AttentionDto::SignInRequired]
        );
    }
}
