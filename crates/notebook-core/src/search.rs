//! In-memory full-text search over decrypted note bodies.
//!
//! The index holds each note's lowercased body and nothing else, and never
//! touches disk. A query is split on whitespace into terms; a note matches
//! when every term occurs in its body as a substring, ignoring case. That
//! handles text without word boundaries (CJK) with no tokenizer. A linear
//! scan is well within a frame for a personal vault.

use std::collections::HashMap;

use crate::note::NoteId;

#[derive(Debug, Default)]
pub struct SearchIndex {
    bodies: HashMap<NoteId, String>,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds note `id`, or replaces what was indexed for it.
    pub fn upsert(&mut self, id: NoteId, body: &str) {
        self.bodies.insert(id, body.to_lowercase());
    }

    pub fn remove(&mut self, id: NoteId) {
        self.bodies.remove(&id);
    }

    /// Whether note `id` matches `query`. Every note matches an empty
    /// query; a note that isn't indexed matches only that.
    pub fn matches(&self, id: NoteId, query: &Query) -> bool {
        if query.terms.is_empty() {
            return true;
        }
        self.bodies
            .get(&id)
            .is_some_and(|body| query.terms.iter().all(|t| body.contains(t.as_str())))
    }

    /// The ids in `ordered` that match `query`, in the same order.
    pub fn filter(&self, ordered: &[NoteId], query: &str) -> Vec<NoteId> {
        let query = Query::parse(query);
        ordered
            .iter()
            .copied()
            .filter(|&id| self.matches(id, &query))
            .collect()
    }
}

/// A parsed query: lowercased terms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    terms: Vec<String>,
}

impl Query {
    pub fn parse(query: &str) -> Self {
        Self {
            terms: query.split_whitespace().map(str::to_lowercase).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> NoteId {
        NoteId::from_parts(n, 0)
    }

    fn index(notes: &[(u64, &str)]) -> SearchIndex {
        let mut index = SearchIndex::new();
        for &(n, body) in notes {
            index.upsert(id(n), body);
        }
        index
    }

    #[test]
    fn all_terms_must_match() {
        let index = index(&[
            (1, "call the plumber about the sink"),
            (2, "plumber invoice"),
        ]);
        assert_eq!(index.filter(&[id(1), id(2)], "plumber sink"), [id(1)]);
        assert_eq!(index.filter(&[id(1), id(2)], "  plumber  "), [id(1), id(2)]);
    }

    #[test]
    fn case_insensitive() {
        let index = index(&[(1, "buy grocery items"), (2, "Grocery list")]);
        assert_eq!(index.filter(&[id(1), id(2)], "GROCERY"), [id(1), id(2)]);
    }

    #[test]
    fn empty_query_keeps_everything_in_order() {
        let index = index(&[(1, "a"), (2, "b")]);
        assert_eq!(index.filter(&[id(2), id(1)], ""), [id(2), id(1)]);
        assert_eq!(index.filter(&[id(2), id(1)], " \t\n"), [id(2), id(1)]);
    }

    #[test]
    fn results_keep_the_given_order() {
        let index = index(&[(1, "x"), (2, "x"), (3, "x")]);
        assert_eq!(
            index.filter(&[id(3), id(1), id(2)], "x"),
            [id(3), id(1), id(2)]
        );
    }

    #[test]
    fn non_ascii_terms() {
        let index = index(&[(1, "Un CAFÉ au lait"), (2, "日本語のメモ"), (3, "cafe")]);
        let all = [id(1), id(2), id(3)];
        assert_eq!(index.filter(&all, "café"), [id(1)]);
        assert_eq!(index.filter(&all, "日本語"), [id(2)]);
        assert_eq!(index.filter(&all, "本語"), [id(2)]);
    }

    #[test]
    fn update_and_removal() {
        let mut index = index(&[(1, "old text")]);
        index.upsert(id(1), "new text");
        assert!(index.filter(&[id(1)], "old").is_empty());
        assert_eq!(index.filter(&[id(1)], "new"), [id(1)]);
        index.remove(id(1));
        assert!(index.filter(&[id(1)], "new").is_empty());
        // An unindexed note still passes an empty query.
        assert_eq!(index.filter(&[id(1)], ""), [id(1)]);
    }
}
