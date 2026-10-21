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

    const ALL: [View; 5] = [
        View::Notes,
        View::Untagged,
        View::Pinned,
        View::Archive,
        View::Trash,
    ];

    fn with(pinned: bool, archived: bool, trashed: bool, body: &str) -> Note {
        let mut n = Note::new(id(1), body, Timestamp(1));
        n.set_pinned(pinned);
        n.set_archived(archived);
        n.set_trashed(trashed);
        n
    }

    #[test]
    fn membership_for_every_flag_combination() {
        for bits in 0..8 {
            let (pinned, archived, trashed) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            for body in ["#tag", "plain"] {
                let n = with(pinned, archived, trashed, body);
                let active = !archived && !trashed;
                let tagged = body == "#tag";
                let expected = [
                    active,
                    active && !tagged,
                    active && pinned,
                    archived && !trashed,
                    trashed,
                ];
                for (view, expected) in ALL.into_iter().zip(expected) {
                    assert_eq!(view.contains(&n), expected, "{view:?} {n:?}");
                }
            }
        }
    }

    #[test]
    fn every_note_is_in_exactly_one_of_notes_archive_and_trash() {
        for bits in 0..8 {
            let n = with(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, "x");
            let homes = [View::Notes, View::Archive, View::Trash]
                .into_iter()
                .filter(|v| v.contains(&n))
                .count();
            assert_eq!(homes, 1, "{n:?}");
        }
    }

    #[test]
    fn heading_and_numbers_do_not_make_a_note_tagged() {
        let n = Note::new(id(1), "# Title\nIssue #42", Timestamp(1));
        assert!(View::Untagged.contains(&n));
    }

    #[test]
    fn select_orders_by_updated_then_id() {
        let notes = [
            Note::new(id(3), "a", Timestamp(5)),
            Note::new(id(1), "b", Timestamp(5)),
            Note::new(id(2), "c", Timestamp(9)),
            Note::new(id(4), "d", Timestamp(1)),
        ];
        assert_eq!(View::Notes.select(&notes), [id(2), id(1), id(3), id(4)]);
    }

    #[test]
    fn order_uses_updated_not_created() {
        let mut edited = Note::new(id(1), "old", Timestamp(1));
        edited.set_body("new", Timestamp(10));
        let notes = [edited, Note::new(id(2), "x", Timestamp(5))];
        assert_eq!(View::Notes.select(&notes), [id(1), id(2)]);
    }

    #[test]
    fn select_on_no_notes_is_empty() {
        for view in ALL {
            assert!(view.select(&[]).is_empty());
        }
    }
}
