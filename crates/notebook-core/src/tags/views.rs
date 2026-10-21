//! Special views derived from note state and tags.

use super::parse;
use super::tree::note_order;
use crate::note::{Note, NoteId};

/// The built-in views shown alongside the tag tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    /// Notes that are neither archived nor trashed.
    Notes,
    /// Notes in [`View::Notes`] that carry no tags.
    Untagged,
    /// Notes in [`View::Notes`] that are pinned.
    Pinned,
    /// Archived notes that are not trashed.
    Archive,
    /// Trashed notes.
    Trash,
}

impl View {
    /// Whether `note` belongs to this view.
    pub fn contains(self, note: &Note) -> bool {
        let active = !note.is_archived() && !note.is_trashed();
        match self {
            View::Notes => active,
            View::Untagged => active && parse(note.body()).is_empty(),
            View::Pinned => active && note.is_pinned(),
            View::Archive => note.is_archived() && !note.is_trashed(),
            View::Trash => note.is_trashed(),
        }
    }

    /// The notes in this view, most recently updated first.
    pub fn select(self, notes: &[Note]) -> Vec<NoteId> {
        let mut selected: Vec<&Note> = notes.iter().filter(|n| self.contains(n)).collect();
        selected.sort_by_key(|n| note_order(n));
        selected.into_iter().map(Note::id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::Timestamp;

    fn id(n: u128) -> NoteId {
        NoteId::from_parts(1, n)
    }

    #[test]
    fn view_membership() {
        let mut a = Note::new(id(1), "#work", Timestamp(1));
        a.set_pinned(true);
        let b = Note::new(id(2), "plain", Timestamp(2));
        let mut c = Note::new(id(3), "old", Timestamp(3));
        c.set_archived(true);
        let mut d = Note::new(id(4), "gone", Timestamp(4));
        d.set_archived(true);
        d.set_trashed(true);
        let notes = [a, b, c, d];

        assert_eq!(View::Notes.select(&notes), [id(2), id(1)]);
        assert_eq!(View::Untagged.select(&notes), [id(2)]);
        assert_eq!(View::Pinned.select(&notes), [id(1)]);
        assert_eq!(View::Archive.select(&notes), [id(3)]);
        assert_eq!(View::Trash.select(&notes), [id(4)]);
    }

    #[test]
    fn hash_only_in_code_is_untagged() {
        let notes = [Note::new(id(1), "Use `#define`", Timestamp(1))];
        assert_eq!(View::Untagged.select(&notes), [id(1)]);
    }
}
