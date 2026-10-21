//! The tag tree derived from note bodies.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use super::{TagPath, tags_of};
use crate::note::{Note, NoteId, Timestamp};

/// Sort key for notes: most recently updated first, then by identifier.
pub(super) type NoteOrder = (Reverse<Timestamp>, NoteId);

pub(super) fn note_order(note: &Note) -> NoteOrder {
    (Reverse(note.updated()), note.id())
}

/// The nested tag tree for a set of notes.
///
/// Built from scratch from note bodies; archived and trashed notes do not
/// contribute. A note tagged `#a/b` appears under both `a` and `a/b`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagTree {
    roots: Vec<TagNode>,
}

/// One node of the [`TagTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagNode {
    /// Display spelling of this node's segment.
    pub name: String,
    /// Full path to this node, in display spelling.
    pub path: TagPath,
    /// Notes carrying this tag or any descendant tag, most recently updated
    /// first.
    pub notes: Vec<NoteId>,
    /// Child tags, ordered by name (case-insensitive).
    pub children: Vec<TagNode>,
}

#[derive(Default)]
struct Builder {
    /// Spelling from the note that sorts first by [`NoteOrder`].
    spelling: Option<(NoteOrder, String)>,
    notes: Vec<NoteOrder>,
    children: BTreeMap<String, Builder>,
}

impl TagTree {
    /// Builds the tree for `notes`.
    pub fn build(notes: &[Note]) -> Self {
        let mut root = Builder::default();
        for note in notes.iter().filter(|n| !n.is_archived() && !n.is_trashed()) {
            let order = note_order(note);
            for tag in tags_of(note.body()) {
                let mut node = &mut root;
                for (segment, key) in tag.segments().iter().zip(tag.key()) {
                    node = node.children.entry(key.clone()).or_default();
                    if node.spelling.as_ref().is_none_or(|(o, _)| order < *o) {
                        node.spelling = Some((order, segment.clone()));
                    }
                    // A note visits a node once per tag under it; keep one entry.
                    if node.notes.last() != Some(&order) {
                        node.notes.push(order);
                    }
                }
            }
        }
        Self {
            roots: finish(root.children, &[]),
        }
    }

    /// Top-level tags.
    pub fn roots(&self) -> &[TagNode] {
        &self.roots
    }

    /// The node for `path`, if any note carries it.
    pub fn find(&self, path: &TagPath) -> Option<&TagNode> {
        let mut nodes = &self.roots;
        let mut found = None;
        for key in path.key() {
            let node = nodes.iter().find(|n| n.path.key().last() == Some(key))?;
            nodes = &node.children;
            found = Some(node);
        }
        found
    }
}

/// Children come out of the map ordered by their lowercased key, which is
/// the case-insensitive name order [`TagNode::children`] promises.
fn finish(children: BTreeMap<String, Builder>, parent: &[String]) -> Vec<TagNode> {
    children
        .into_values()
        .map(|mut b| {
            let name = b.spelling.map(|(_, s)| s).unwrap_or_default();
            let mut segments = parent.to_vec();
            segments.push(name.clone());
            b.notes.sort();
            TagNode {
                children: finish(b.children, &segments),
                name,
                path: TagPath::from_segments(segments),
                notes: b.notes.into_iter().map(|(_, id)| id).collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(n: u128, updated: i64, body: &str) -> Note {
        Note::new(NoteId::from_parts(1, n), body, Timestamp(updated))
    }

    fn id(n: u128) -> NoteId {
        NoteId::from_parts(1, n)
    }

    fn path(s: &str) -> TagPath {
        TagPath::parse(s).unwrap()
    }

    fn names(nodes: &[TagNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn ancestors_are_implied() {
        let tree = TagTree::build(&[note(1, 1, "#work/acme")]);
        assert_eq!(names(tree.roots()), ["work"]);
        let work = &tree.roots()[0];
        assert_eq!(names(&work.children), ["acme"]);
        assert_eq!(work.notes, [id(1)]);
        assert_eq!(work.children[0].notes, [id(1)]);
        assert_eq!(work.children[0].path.to_string(), "work/acme");
    }

    #[test]
    fn note_under_several_tags() {
        let tree = TagTree::build(&[note(1, 1, "#home #work/acme")]);
        for p in ["home", "work", "work/acme"] {
            assert_eq!(tree.find(&path(p)).unwrap().notes, [id(1)], "{p}");
        }
    }

    #[test]
    fn directly_tagged_parent() {
        let tree = TagTree::build(&[note(1, 1, "#work"), note(2, 1, "#work/acme")]);
        assert_eq!(tree.find(&path("work")).unwrap().notes, [id(1), id(2)]);
        assert_eq!(tree.find(&path("work/acme")).unwrap().notes, [id(2)]);
    }

    #[test]
    fn note_listed_once_per_node() {
        let tree = TagTree::build(&[note(1, 1, "#work/a #work/b #work")]);
        assert_eq!(tree.find(&path("work")).unwrap().notes, [id(1)]);
    }

    #[test]
    fn archived_and_trashed_are_excluded() {
        let mut old = note(1, 1, "#old");
        old.set_archived(true);
        let mut gone = note(2, 1, "#gone");
        gone.set_trashed(true);
        let tree = TagTree::build(&[old, gone, note(3, 1, "#kept")]);
        assert_eq!(names(tree.roots()), ["kept"]);
    }

    #[test]
    fn case_insensitive_identity_and_display_spelling() {
        let tree = TagTree::build(&[note(1, 1, "#work"), note(2, 2, "#Work")]);
        assert_eq!(names(tree.roots()), ["Work"]);
        assert_eq!(tree.roots()[0].notes, [id(2), id(1)]);
        assert_eq!(tree.find(&path("WORK")).unwrap().name, "Work");
    }

    #[test]
    fn display_spelling_tie_breaks_on_smaller_id() {
        let tree = TagTree::build(&[note(2, 1, "#Work"), note(1, 1, "#work")]);
        assert_eq!(names(tree.roots()), ["work"]);
    }

    #[test]
    fn sibling_order() {
        let tree = TagTree::build(&[note(1, 1, "#zeta #Alpha #beta")]);
        assert_eq!(names(tree.roots()), ["Alpha", "beta", "zeta"]);
    }

    #[test]
    fn note_order_under_a_tag() {
        let tree = TagTree::build(&[
            note(1, 1, "#work"),
            note(2, 2, "#work"),
            note(3, 2, "#work"),
        ]);
        assert_eq!(tree.roots()[0].notes, [id(2), id(3), id(1)]);
    }

    #[test]
    fn find_missing() {
        let tree = TagTree::build(&[note(1, 1, "#work/acme")]);
        assert!(tree.find(&path("home")).is_none());
        assert!(tree.find(&path("work/other")).is_none());
    }

    #[test]
    fn empty_tree() {
        let tree = TagTree::build(&[]);
        assert!(tree.roots().is_empty());
        assert_eq!(tree, TagTree::default());
        assert!(tree.find(&path("work")).is_none());
        assert_eq!(TagTree::build(&[note(1, 1, "no tags")]), TagTree::default());
    }

    #[test]
    fn find_descends_case_insensitively() {
        let tree = TagTree::build(&[note(1, 1, "#Work/Acme/Q3")]);
        let node = tree.find(&path("work/acme/q3")).unwrap();
        assert_eq!(node.name, "Q3");
        assert_eq!(node.path.to_string(), "Work/Acme/Q3");
        assert!(node.children.is_empty());
        assert!(tree.find(&path("work/acme/q3/deeper")).is_none());
        assert!(tree.find(&path("acme")).is_none());
    }

    #[test]
    fn each_segment_picks_its_own_spelling() {
        let tree = TagTree::build(&[
            note(1, 1, "#Work/ACME"),
            note(2, 2, "#work/other"),
            note(3, 0, "#WORK/acme"),
        ]);
        let work = &tree.roots()[0];
        assert_eq!(work.name, "work");
        assert_eq!(names(&work.children), ["ACME", "other"]);
        assert_eq!(work.children[0].path.to_string(), "work/ACME");
    }

    #[test]
    fn spelling_comes_from_the_newest_note_regardless_of_order() {
        let notes = [note(1, 5, "#Tag"), note(2, 1, "#tag"), note(3, 3, "#TAG")];
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            let shuffled: Vec<Note> = order.iter().map(|&i| notes[i].clone()).collect();
            assert_eq!(names(TagTree::build(&shuffled).roots()), ["Tag"]);
        }
    }

    #[test]
    fn parent_lists_notes_from_all_children_in_order() {
        let tree = TagTree::build(&[
            note(1, 1, "#work/a"),
            note(2, 3, "#work/b"),
            note(3, 2, "#work/a #work/b"),
        ]);
        assert_eq!(
            tree.find(&path("work")).unwrap().notes,
            [id(2), id(3), id(1)]
        );
        assert_eq!(tree.find(&path("work/a")).unwrap().notes, [id(3), id(1)]);
        assert_eq!(tree.find(&path("work/b")).unwrap().notes, [id(2), id(3)]);
    }

    #[test]
    fn nested_sibling_order() {
        let tree = TagTree::build(&[note(1, 1, "#p/zeta #p/Alpha #p/beta")]);
        assert_eq!(names(&tree.roots()[0].children), ["Alpha", "beta", "zeta"]);
    }

    #[test]
    fn pinned_notes_contribute_normally() {
        let mut pinned = note(1, 1, "#work");
        pinned.set_pinned(true);
        let tree = TagTree::build(&[pinned, note(2, 2, "#work")]);
        assert_eq!(tree.roots()[0].notes, [id(2), id(1)]);
    }

    #[test]
    fn tags_in_code_do_not_appear() {
        let tree = TagTree::build(&[note(1, 1, "`#code` and\n```\n#fenced\n```\n#real")]);
        assert_eq!(names(tree.roots()), ["real"]);
    }

    #[test]
    fn multi_word_tags_become_nodes() {
        let tree = TagTree::build(&[note(1, 1, "#reading list/to buy#")]);
        let node = tree.find(&path("reading list/to buy")).unwrap();
        assert_eq!(node.name, "to buy");
        assert_eq!(tree.roots()[0].name, "reading list");
    }

    #[test]
    fn archived_note_does_not_affect_spelling() {
        let mut newest = note(2, 9, "#WORK");
        newest.set_archived(true);
        let tree = TagTree::build(&[note(1, 1, "#work"), newest]);
        assert_eq!(names(tree.roots()), ["work"]);
        assert_eq!(tree.roots()[0].notes, [id(1)]);
    }
}
