//! Bear-style inline tags.
//!
//! Tags are written in the note body and everything else is derived from it:
//! the tag tree, the special views, and tag rename/move as a body rewrite.
//!
//! # Grammar
//!
//! - A tag starts with `#` at the start of the text or after whitespace, and
//!   the next character must not be whitespace or `#`.
//! - **Single-word** form: the name runs until whitespace or `#`. A `#`
//!   directly after the name closes it when followed by whitespace, the end
//!   of the text, or trailing punctuation.
//! - **Multi-word** form: `#name with spaces#` on one line. The name contains
//!   no `#`, its last character is not whitespace, and the closing `#` is
//!   followed by whitespace, the end of the line, or trailing punctuation.
//!   Its first word must itself be a valid single-word tag.
//! - Trailing punctuation (`. , ; : ! ? " ' ) ] }`), `/`, and whitespace at
//!   the end of a name are not part of the tag.
//! - `/` separates nested segments. Empty segments are dropped and runs of
//!   whitespace inside a segment collapse to one space.
//! - Names made only of digits (`#42`) are not tags.
//! - Nothing inside inline code spans, fenced code blocks, or indented code
//!   blocks is a tag.
//!
//! Tag identity is case-insensitive: see [`TagPath`].

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

mod parse;
mod rename;
mod tree;
mod views;

pub use parse::{TagForm, TagOccurrence, parse, tags_of};
pub use rename::{RenameError, rename};
pub use tree::{TagNode, TagTree};
pub use views::View;

/// Characters that end a sentence and are never the last character of a tag.
const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', '"', '\'', ')', ']', '}'];

fn is_trailing_punctuation(c: char) -> bool {
    TRAILING_PUNCTUATION.contains(&c)
}

/// A normalized tag path such as `work/acme`.
///
/// Keeps the segments as written for display, and compares, hashes, and
/// orders by their Unicode-lowercased form, so `#Work` and `#work` are the
/// same tag.
#[derive(Debug, Clone)]
pub struct TagPath {
    segments: Vec<String>,
    key: Vec<String>,
}

impl TagPath {
    /// Parses a tag name typed by a user, such as a rename target.
    ///
    /// Fails when the name contains `#` or a line break, has no segments,
    /// consists only of digits, or ends in trailing punctuation, since any of
    /// those would not read back as the same tag.
    pub fn parse(name: &str) -> Option<Self> {
        if name.contains(['#', '\n', '\r']) {
            return None;
        }
        let path = Self::from_raw(name)?;
        let last = path.segments.last()?;
        if last.ends_with(is_trailing_punctuation) {
            return None;
        }
        Some(path)
    }

    /// Normalizes raw tag text: splits on `/`, collapses and trims whitespace
    /// in each segment, drops empty segments, and rejects digit-only names.
    fn from_raw(raw: &str) -> Option<Self> {
        let segments: Vec<String> = raw
            .split('/')
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
            .collect();
        if segments.is_empty()
            || segments
                .iter()
                .all(|s| s.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        Some(Self::from_segments(segments))
    }

    fn from_segments(segments: Vec<String>) -> Self {
        let key = segments.iter().map(|s| s.to_lowercase()).collect();
        Self { segments, key }
    }

    /// The segments as written, outermost first.
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Number of segments.
    pub fn depth(&self) -> usize {
        self.segments.len()
    }

    /// Whether `self` equals `other` or is one of its ancestors.
    pub fn is_prefix_of(&self, other: &TagPath) -> bool {
        other.key.starts_with(&self.key)
    }

    fn key(&self) -> &[String] {
        &self.key
    }
}

impl fmt::Display for TagPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.segments.join("/"))
    }
}

impl PartialEq for TagPath {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for TagPath {}

impl Hash for TagPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl PartialOrd for TagPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TagPath {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(p: &TagPath) -> Vec<&str> {
        p.segments().iter().map(String::as_str).collect()
    }

    #[test]
    fn normalizes_segments() {
        let p = TagPath::parse("  reading   list // to  buy/ ").unwrap();
        assert_eq!(segs(&p), ["reading list", "to buy"]);
        assert_eq!(p.to_string(), "reading list/to buy");
        assert_eq!(p.depth(), 2);
    }

    #[test]
    fn identity_is_case_insensitive() {
        let a = TagPath::parse("Work/ACME").unwrap();
        let b = TagPath::parse("work/acme").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "Work/ACME");
        assert_eq!(
            TagPath::parse("Café").unwrap(),
            TagPath::parse("CAFÉ").unwrap()
        );
    }

    #[test]
    fn prefixes() {
        let work = TagPath::parse("work").unwrap();
        assert!(work.is_prefix_of(&TagPath::parse("Work/acme").unwrap()));
        assert!(work.is_prefix_of(&work));
        assert!(!work.is_prefix_of(&TagPath::parse("workshop").unwrap()));
        assert!(!TagPath::parse("work/acme").unwrap().is_prefix_of(&work));
    }

    #[test]
    fn rejects_invalid_names() {
        for name in [
            "", "/", " // ", "a#b", "a\nb", "a\rb", "42", "2024/12", "job!", "a/b.",
        ] {
            assert_eq!(TagPath::parse(name), None, "{name:?}");
        }
    }

    #[test]
    fn accepts_digits_with_other_characters() {
        assert!(TagPath::parse("2024/q1").is_some());
        assert!(TagPath::parse("3d").is_some());
        assert!(TagPath::parse("a./b").is_some());
    }

    #[test]
    fn rejects_every_trailing_punctuation_character() {
        for &c in TRAILING_PUNCTUATION {
            assert_eq!(TagPath::parse(&format!("job{c}")), None, "{c:?}");
            assert!(TagPath::parse(&format!("a{c}b")).is_some(), "{c:?}");
        }
        assert!(TagPath::parse("job-").is_some());
        assert!(TagPath::parse("job_").is_some());
    }

    #[test]
    fn tabs_and_unicode_whitespace_collapse() {
        let p = TagPath::parse("\treading\t\u{a0} list\u{3000}").unwrap();
        assert_eq!(segs(&p), ["reading list"]);
    }

    #[test]
    fn single_segment_path() {
        let p = TagPath::parse("work").unwrap();
        assert_eq!(segs(&p), ["work"]);
        assert_eq!(p.depth(), 1);
        assert_eq!(p.to_string(), "work");
    }

    #[test]
    fn digit_only_segments_are_allowed_under_a_named_parent() {
        let p = TagPath::parse("year/2024").unwrap();
        assert_eq!(segs(&p), ["year", "2024"]);
    }

    #[test]
    fn prefix_matching_is_case_insensitive_and_segment_wise() {
        let upper = TagPath::parse("WORK").unwrap();
        assert!(upper.is_prefix_of(&TagPath::parse("work/acme").unwrap()));
        let acme = TagPath::parse("work/acme").unwrap();
        assert!(!acme.is_prefix_of(&TagPath::parse("work/acmecorp").unwrap()));
        assert!(!acme.is_prefix_of(&TagPath::parse("acme").unwrap()));
    }

    #[test]
    fn ordering_and_hashing_follow_the_lowercase_key() {
        use std::collections::HashSet;

        let a = TagPath::parse("Alpha").unwrap();
        let b = TagPath::parse("beta").unwrap();
        let a_child = TagPath::parse("alpha/x").unwrap();
        assert!(a < b);
        assert!(a < a_child);
        assert!(a_child < b);
        assert_eq!(a.cmp(&TagPath::parse("ALPHA").unwrap()), Ordering::Equal);

        let set: HashSet<TagPath> = ["Work", "work", "WORK", "work/acme"]
            .into_iter()
            .map(|s| TagPath::parse(s).unwrap())
            .collect();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn unicode_lowercasing_beyond_ascii() {
        assert_eq!(
            TagPath::parse("ΣΟΦΊΑ").unwrap(),
            TagPath::parse("σοφία").unwrap()
        );
        assert_ne!(
            TagPath::parse("cafe").unwrap(),
            TagPath::parse("café").unwrap()
        );
    }
}
