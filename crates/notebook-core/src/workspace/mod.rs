//! The unlocked vault as the UI sees it: views, note and tag operations,
//! search, and change notifications over the sync replica.
//!
//! [`Workspace`] owns no documents: the shared [`Replica`] holds them and
//! performs every edit with its own storage transaction and outbox entry.
//! The workspace adds what the UI needs on top: the tag tree, the search
//! index, list entries, and app rules such as "trashed notes are read-only".
//!
//! Every change, local or remote, comes out of one [`Changed`] stream, so
//! the UI re-queries the same way for both. Local edits produce no engine
//! event, so the workspace reports its own; the host passes engine events to
//! [`Workspace::apply_sync_event`] for remote ones.
//!
//! Like the replica, a workspace is only ever used inside synchronous calls.
//! [`Workspace::purge`] returns a future that doesn't borrow it, so the host
//! can await it without holding whatever lock guards the workspace.

mod server_url;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;

use futures::channel::mpsc;

pub use server_url::{ServerUrl, ServerUrlError};

use crate::document::EditorUpdate;
use crate::note::{Note, NoteId, Timestamp, after_title};
use crate::search::SearchIndex;
use crate::storage::Storage;
use crate::sync::{
    EditorSessionId, Replica, ReplicaError, Shared, SyncError, SyncEvent, SyncHandle,
};
use crate::tags::{self, RenameError, TagPath, TagTree, View};

/// What the note list shows: a special view or a tag (with its
/// descendants).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    View(View),
    Tag(TagPath),
}

/// One row of the note list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub id: NoteId,
    /// Empty when the note has no title; the UI shows a placeholder.
    pub title: String,
    /// Plain text after the title line, shortened.
    pub preview: String,
    pub updated: Timestamp,
    pub pinned: bool,
    pub archived: bool,
    pub trashed: bool,
}

/// Notes that changed (created, edited, or removed), and whether the tag
/// tree changed with them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changed {
    pub notes: Vec<NoteId>,
    pub tree_changed: bool,
}

/// Why a workspace operation was refused or failed. On error nothing
/// changed, except as noted for [`Workspace::rename_tag`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    NotFound(NoteId),
    /// The note is in the trash, so its body is read-only.
    Trashed(NoteId),
    /// Only trashed notes can be deleted permanently.
    NotTrashed(NoteId),
    InvalidTagName(String),
    /// A tag in this note can't be renamed without changing the text around
    /// it.
    Unrepresentable(NoteId),
    Replica(ReplicaError),
    /// Permanent deletion failed, for example because the server can't be
    /// reached ([`SyncError::Offline`]).
    Sync(SyncError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "note {id} not found"),
            Self::Trashed(id) => write!(f, "note {id} is in the trash"),
            Self::NotTrashed(id) => write!(f, "note {id} is not in the trash"),
            Self::InvalidTagName(name) => write!(f, "invalid tag name: {name:?}"),
            Self::Unrepresentable(id) => write!(f, "tag can't be rewritten in note {id}"),
            Self::Replica(err) => err.fmt(f),
            Self::Sync(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<ReplicaError> for WorkspaceError {
    fn from(err: ReplicaError) -> Self {
        match err {
            ReplicaError::NoteNotFound(id) => Self::NotFound(id),
            err => Self::Replica(err),
        }
    }
}

impl From<RenameError> for WorkspaceError {
    fn from(err: RenameError) -> Self {
        match err {
            RenameError::InvalidName(name) => Self::InvalidTagName(name),
            RenameError::Unrepresentable(id) => Self::Unrepresentable(id),
        }
    }
}

type Result<T> = std::result::Result<T, WorkspaceError>;

/// Longest list preview, in characters.
const PREVIEW_CHARS: usize = 200;

/// The unlocked vault: the shared replica and its engine handle, plus the
/// tag tree and search index derived from it.
pub struct Workspace<S: Storage> {
    replica: Shared<Replica<S>>,
    sync: SyncHandle,
    tree: TagTree,
    index: SearchIndex,
    /// Each attached editor's note, to refuse edits to trashed notes.
    editors: BTreeMap<EditorSessionId, NoteId>,
    changes: mpsc::UnboundedSender<Changed>,
}

impl<S: Storage> Workspace<S> {
    /// Builds the tag tree and search index from every note in `replica`.
    /// Returns the workspace and its change stream.
    pub fn open(
        replica: Shared<Replica<S>>,
        sync: SyncHandle,
    ) -> (Self, mpsc::UnboundedReceiver<Changed>) {
        let (changes, changes_rx) = mpsc::unbounded();
        let mut workspace = Self {
            replica,
            sync,
            tree: TagTree::default(),
            index: SearchIndex::new(),
            editors: BTreeMap::new(),
            changes,
        };
        workspace.rebuild();
        (workspace, changes_rx)
    }

    pub fn replica(&self) -> &Shared<Replica<S>> {
        &self.replica
    }

    pub fn sync(&self) -> &SyncHandle {
        &self.sync
    }

    /// The tag tree of notes that are neither archived nor trashed.
    pub fn tree(&self) -> &TagTree {
        &self.tree
    }

    /// Every note, archived and trashed included (for export).
    pub fn all_notes(&self) -> Vec<Note> {
        self.replica.lock().notes()
    }

    pub fn note(&self, id: NoteId) -> Option<Note> {
        self.replica.lock().note(id)
    }

    /// The selection's notes, most recently updated first, that match
    /// `query` (see [`crate::search`]).
    pub fn list(&self, selection: &Selection, query: &str) -> Vec<ListEntry> {
        let notes = self.replica.lock().notes();
        let ids = match selection {
            Selection::View(view) => view.select(&notes),
            Selection::Tag(path) => self
                .tree
                .find(path)
                .map(|node| node.notes.clone())
                .unwrap_or_default(),
        };
        let by_id: BTreeMap<NoteId, &Note> = notes.iter().map(|n| (n.id(), n)).collect();
        self.index
            .filter(&ids, query)
            .into_iter()
            .filter_map(|id| by_id.get(&id).map(|note| entry(note)))
            .collect()
    }

    /// Creates a note. With a tag selected, its body starts with that tag
    /// on its own line, so the note is listed under it.
    pub fn create(&mut self, selection: &Selection, now: Timestamp) -> Result<NoteId> {
        let body = match selection {
            Selection::Tag(path) => {
                // The tree's spelling, so the new tag reads like the others.
                let path = self.tree.find(path).map_or(path, |node| &node.path);
                tag_line(path)
            }
            Selection::View(_) => String::new(),
        };
        let id = self.replica.lock().create_note(&body, now)?;
        self.refresh(&[id]);
        Ok(id)
    }

    /// Replaces a note's body. Refused for trashed notes.
    pub fn set_body(&mut self, id: NoteId, body: &str, now: Timestamp) -> Result<()> {
        self.check_editable(id)?;
        self.replica.lock().set_body(id, body, now)?;
        self.refresh(&[id]);
        Ok(())
    }

    pub fn set_pinned(&mut self, id: NoteId, pinned: bool) -> Result<()> {
        self.replica.lock().set_pinned(id, pinned)?;
        self.refresh(&[id]);
        Ok(())
    }

    pub fn set_archived(&mut self, id: NoteId, archived: bool) -> Result<()> {
        self.replica.lock().set_archived(id, archived)?;
        self.refresh(&[id]);
        Ok(())
    }

    /// Moves a note to the trash or restores it. Restoring keeps its other
    /// flags, so an archived note returns to Archive.
    pub fn set_trashed(&mut self, id: NoteId, trashed: bool) -> Result<()> {
        self.replica.lock().set_trashed(id, trashed)?;
        self.refresh(&[id]);
        Ok(())
    }

    /// Starts permanently deleting a trashed note. The returned future
    /// deletes it on the server, then locally, and needs a connection: it
    /// fails with [`SyncError::Offline`] when there is none, and then
    /// nothing changes. Once it succeeds, call [`refresh`](Self::refresh)
    /// with the note's id.
    pub fn purge(&self, id: NoteId) -> Result<impl Future<Output = Result<()>> + use<S>> {
        match self.note(id) {
            None => return Err(WorkspaceError::NotFound(id)),
            Some(note) if !note.is_trashed() => return Err(WorkspaceError::NotTrashed(id)),
            Some(_) => {}
        }
        let sync = self.sync.clone();
        Ok(async move {
            sync.purge(id).await.map_err(|err| match err {
                SyncError::NotTrashed(id) => WorkspaceError::NotTrashed(id),
                SyncError::NoteNotFound(id) => WorkspaceError::NotFound(id),
                err => WorkspaceError::Sync(err),
            })
        })
    }

    /// Renames or moves tag `old` to `new` in every note carrying it or a
    /// descendant, archived and trashed notes included. Returns how many
    /// notes changed. An invalid name changes nothing; a storage failure
    /// part way leaves the notes before it renamed.
    pub fn rename_tag(&mut self, old: &str, new: &str, now: Timestamp) -> Result<usize> {
        let notes = self.replica.lock().notes();
        let changes = tags::rename(&notes, old, new)?;
        let mut done = Vec::with_capacity(changes.len());
        let mut result = Ok(());
        {
            let mut replica = self.replica.lock();
            for (id, body) in &changes {
                if let Err(err) = replica.set_body(*id, body, now) {
                    result = Err(err.into());
                    break;
                }
                done.push(*id);
            }
        }
        self.refresh(&done);
        result.map(|()| done.len())
    }

    /// Attaches an editor to note `id` (see [`Replica::editor_attach`]).
    pub fn editor_attach(&mut self, id: NoteId) -> Result<(EditorSessionId, EditorUpdate)> {
        let (session, state) = self.replica.lock().editor_attach(id)?;
        self.editors.insert(session, id);
        Ok((session, state))
    }

    /// Applies an edit from `session`'s editor. Refused while the note is
    /// trashed, so a stale editor can't write to it.
    pub fn apply_editor_update(
        &mut self,
        session: EditorSessionId,
        update: &EditorUpdate,
        now: Timestamp,
    ) -> Result<()> {
        let id = *self.editors.get(&session).ok_or(WorkspaceError::Replica(
            ReplicaError::EditorSessionNotFound(session),
        ))?;
        self.check_editable(id)?;
        if self
            .replica
            .lock()
            .apply_editor_update(session, update, now)?
        {
            self.refresh(&[id]);
        }
        Ok(())
    }

    /// What `session`'s editor lacks, if anything (see
    /// [`Replica::editor_pending`]).
    pub fn editor_pending(&mut self, session: EditorSessionId) -> Result<Option<EditorUpdate>> {
        Ok(self.replica.lock().editor_pending(session)?)
    }

    /// The note `session`'s editor is attached to, if it still is.
    pub fn editor_note(&self, session: EditorSessionId) -> Option<NoteId> {
        self.editors.get(&session).copied()
    }

    /// The editor sessions attached to any of `notes`.
    pub fn editors_of(&self, notes: &[NoteId]) -> Vec<EditorSessionId> {
        self.editors
            .iter()
            .filter(|(_, id)| notes.contains(id))
            .map(|(session, _)| *session)
            .collect()
    }

    pub fn editor_detach(&mut self, session: EditorSessionId) {
        self.editors.remove(&session);
        self.replica.lock().editor_detach(session);
    }

    /// Takes in an engine event: remote changes update the tree and index
    /// and are reported like local ones.
    pub fn apply_sync_event(&mut self, event: &SyncEvent) {
        if let SyncEvent::NotesChanged(ids) = event {
            self.refresh(ids);
        }
    }

    /// The replica changed `ids` (or removed them): updates the index and
    /// tree and reports the change.
    pub fn refresh(&mut self, ids: &[NoteId]) {
        if ids.is_empty() {
            return;
        }
        let notes = {
            let replica = self.replica.lock();
            for &id in ids {
                match replica.note(id) {
                    Some(note) => self.index.upsert(id, note.body()),
                    None => self.index.remove(id),
                }
            }
            replica.notes()
        };
        let gone: BTreeSet<NoteId> = ids
            .iter()
            .copied()
            .filter(|id| !notes.iter().any(|n| n.id() == *id))
            .collect();
        self.editors.retain(|_, id| !gone.contains(id));
        let tree = TagTree::build(&notes);
        let tree_changed = tree != self.tree;
        self.tree = tree;
        let _ = self.changes.unbounded_send(Changed {
            notes: ids.to_vec(),
            tree_changed,
        });
    }

    /// Rebuilds the tree and index from scratch, as after the replica was
    /// rebuilt from storage, and reports every note as changed.
    pub fn reload(&mut self) {
        let before: BTreeSet<NoteId> = self.index_ids();
        self.rebuild();
        let mut ids: BTreeSet<NoteId> = self.index_ids();
        ids.extend(before);
        let _ = self.changes.unbounded_send(Changed {
            notes: ids.into_iter().collect(),
            tree_changed: true,
        });
    }

    fn rebuild(&mut self) {
        let notes = self.replica.lock().notes();
        self.index = SearchIndex::new();
        for note in &notes {
            self.index.upsert(note.id(), note.body());
        }
        self.tree = TagTree::build(&notes);
    }

    fn index_ids(&self) -> BTreeSet<NoteId> {
        self.replica.lock().notes().iter().map(Note::id).collect()
    }

    fn check_editable(&self, id: NoteId) -> Result<()> {
        match self.note(id) {
            None => Err(WorkspaceError::NotFound(id)),
            Some(note) if note.is_trashed() => Err(WorkspaceError::Trashed(id)),
            Some(_) => Ok(()),
        }
    }
}

/// `#path` on its own line; multi-word paths are closed with `#`.
fn tag_line(path: &TagPath) -> String {
    let close = if path.segments().iter().any(|s| s.contains(' ')) {
        "#"
    } else {
        ""
    };
    format!("#{path}{close}\n")
}

fn entry(note: &Note) -> ListEntry {
    ListEntry {
        id: note.id(),
        title: note.title().to_owned(),
        preview: preview(after_title(note.body())),
        updated: note.updated(),
        pinned: note.is_pinned(),
        archived: note.is_archived(),
        trashed: note.is_trashed(),
    }
}

/// The text with block markers (headings, quotes, list bullets, task boxes)
/// and code fences dropped, whitespace collapsed, and cut to
/// [`PREVIEW_CHARS`].
fn preview(text: &str) -> String {
    let mut out = String::new();
    let mut chars = 0;
    'lines: for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            continue;
        }
        for word in strip_markers(line).split_whitespace() {
            let len = word.chars().count();
            if !out.is_empty() {
                out.push(' ');
                chars += 1;
            }
            if chars + len > PREVIEW_CHARS {
                out.extend(word.chars().take(PREVIEW_CHARS.saturating_sub(chars)));
                break 'lines;
            }
            out.push_str(word);
            chars += len;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

fn strip_markers(mut line: &str) -> &str {
    loop {
        let before = line;
        let hashes = line.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            line = &line[hashes..];
        }
        for marker in ["> ", ">", "- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
            if let Some(rest) = line.strip_prefix(marker) {
                line = rest;
                break;
            }
        }
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0
            && let Some(rest) = line[digits..]
                .strip_prefix(". ")
                .or_else(|| line[digits..].strip_prefix(") "))
        {
            line = rest;
        }
        line = line.trim_start();
        if line == before {
            return line;
        }
    }
}

#[cfg(test)]
mod tests;
