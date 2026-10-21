//! Tag rename and move as a pure body rewrite.

use std::fmt;

use super::{TagForm, TagPath, parse};
use crate::note::{Note, NoteId};

/// Error returned by [`rename`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// The given name is not a valid tag name.
    InvalidName(String),
}

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenameError::InvalidName(name) => write!(f, "invalid tag name: {name:?}"),
        }
    }
}

impl std::error::Error for RenameError {}

/// Renames or moves tag `old` to `new` across `notes`.
///
/// Every occurrence of `old` and of its descendants (`old/...`) is rewritten,
/// matching case-insensitively; descendants keep their own suffix. Only the
/// tag text changes. Returns the identifier and new body of each note whose
/// body changed; the notes themselves are not modified.
pub fn rename(notes: &[Note], old: &str, new: &str) -> Result<Vec<(NoteId, String)>, RenameError> {
    let old = TagPath::parse(old).ok_or_else(|| RenameError::InvalidName(old.to_owned()))?;
    let new = TagPath::parse(new).ok_or_else(|| RenameError::InvalidName(new.to_owned()))?;
    Ok(notes
        .iter()
        .filter_map(|note| {
            let body = rewrite(note.body(), &old, &new)?;
            (body != note.body()).then(|| (note.id(), body))
        })
        .collect())
}

/// Rewrites `body`, or returns `None` if it has no matching occurrence.
fn rewrite(body: &str, old: &TagPath, new: &TagPath) -> Option<String> {
    let mut out = String::with_capacity(body.len());
    let mut copied = 0;
    for occurrence in parse(body)
        .into_iter()
        .filter(|o| old.is_prefix_of(&o.path))
    {
        out.push_str(&body[copied..occurrence.range.start]);
        let name = new
            .segments()
            .iter()
            .chain(&occurrence.path.segments()[old.depth()..])
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("/");
        out.push('#');
        out.push_str(&name);
        if name.contains(char::is_whitespace) || occurrence.form == TagForm::Closed {
            out.push('#');
        }
        copied = occurrence.range.end;
    }
    if copied == 0 {
        return None;
    }
    out.push_str(&body[copied..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::Timestamp;
    use crate::tags::TagTree;

    fn note(n: u128, body: &str) -> Note {
        Note::new(NoteId::from_parts(1, n), body, Timestamp(1))
    }

    fn id(n: u128) -> NoteId {
        NoteId::from_parts(1, n)
    }

    /// Renames across a single note and returns its new body, if changed.
    fn one(body: &str, old: &str, new: &str) -> Option<String> {
        rename(&[note(1, body)], old, new)
            .unwrap()
            .pop()
            .map(|(_, b)| b)
    }

    #[test]
    fn simple_rename() {
        assert_eq!(
            rename(&[note(1, "#work stuff")], "work", "job").unwrap(),
            [(id(1), "#job stuff".to_owned())]
        );
    }

    #[test]
    fn descendants_follow() {
        assert_eq!(
            one("#work/acme and #work", "work", "job").as_deref(),
            Some("#job/acme and #job")
        );
    }

    #[test]
    fn move_under_new_parent() {
        assert_eq!(
            one("#acme/q3", "acme", "clients/acme").as_deref(),
            Some("#clients/acme/q3")
        );
        assert_eq!(
            one("#clients/acme/q3", "clients/acme", "acme").as_deref(),
            Some("#acme/q3")
        );
    }

    #[test]
    fn sibling_prefixes_untouched() {
        assert_eq!(one("#workshop", "work", "job"), None);
        assert_eq!(one("#home/work", "work", "job"), None);
    }

    #[test]
    fn case_insensitive_match() {
        assert_eq!(
            one("#Work/Acme", "work", "job").as_deref(),
            Some("#job/Acme")
        );
    }

    #[test]
    fn unaffected_notes_omitted() {
        let notes = [note(1, "#home"), note(2, "#work"), note(3, "nothing")];
        assert_eq!(
            rename(&notes, "work", "job").unwrap(),
            [(id(2), "#job".to_owned())]
        );
    }

    #[test]
    fn same_name_is_a_no_op() {
        assert_eq!(one("#work", "work", "work"), None);
        assert_eq!(one("#work", "work", "Work").as_deref(), Some("#Work"));
    }

    #[test]
    fn merge_into_existing_tag() {
        let mut notes = vec![note(1, "#job"), note(2, "#work")];
        for (id, body) in rename(&notes, "job", "work").unwrap() {
            notes
                .iter_mut()
                .find(|n| n.id() == id)
                .unwrap()
                .set_body(body, Timestamp(2));
        }
        let tree = TagTree::build(&notes);
        assert_eq!(tree.roots().len(), 1);
        assert_eq!(tree.roots()[0].name, "work");
        assert_eq!(tree.roots()[0].notes.len(), 2);
    }

    #[test]
    fn code_is_not_rewritten() {
        assert_eq!(
            one("#work and `#work` ", "work", "job").as_deref(),
            Some("#job and `#work` ")
        );
        assert_eq!(
            one("```\n#work\n```\n#work", "work", "job").as_deref(),
            Some("```\n#work\n```\n#job")
        );
    }

    #[test]
    fn to_and_from_multi_word() {
        assert_eq!(
            one("See #ideas.", "ideas", "side projects").as_deref(),
            Some("See #side projects#.")
        );
        assert_eq!(
            one("#side projects# today", "side projects", "ideas").as_deref(),
            Some("#ideas today")
        );
        assert_eq!(
            one("#work/side gig# x", "work", "job").as_deref(),
            Some("#job/side gig# x")
        );
    }

    #[test]
    fn closed_form_keeps_closing_hash() {
        assert_eq!(
            one("#ideas# for later", "ideas", "plans").as_deref(),
            Some("#plans# for later")
        );
    }

    #[test]
    fn archived_and_trashed_notes_are_rewritten() {
        let mut archived = note(1, "#work");
        archived.set_archived(true);
        let mut trashed = note(2, "#work");
        trashed.set_trashed(true);
        assert_eq!(
            rename(&[archived, trashed], "work", "job").unwrap().len(),
            2
        );
    }

    #[test]
    fn invalid_names_are_rejected() {
        let notes = [note(1, "#work")];
        for new in ["/", "a#b", "42", "job!", "a\nb"] {
            assert_eq!(
                rename(&notes, "work", new),
                Err(RenameError::InvalidName(new.to_owned())),
                "{new:?}"
            );
        }
        assert_eq!(
            rename(&notes, "", "job"),
            Err(RenameError::InvalidName(String::new()))
        );
    }

    #[test]
    fn invalid_old_names_are_rejected() {
        let notes = [note(1, "#work")];
        for old in ["/", "a#b", "42", "work.", "a\rb"] {
            assert_eq!(
                rename(&notes, old, "job"),
                Err(RenameError::InvalidName(old.to_owned())),
                "{old:?}"
            );
        }
    }

    #[test]
    fn old_name_is_checked_first() {
        assert_eq!(
            rename(&[], "42", "43"),
            Err(RenameError::InvalidName("42".to_owned()))
        );
    }

    #[test]
    fn error_message() {
        let err = RenameError::InvalidName("a#b".to_owned());
        assert_eq!(err.to_string(), "invalid tag name: \"a#b\"");
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn empty_note_set() {
        assert_eq!(rename(&[], "work", "job").unwrap(), []);
    }

    #[test]
    fn names_are_normalized_before_use() {
        assert_eq!(
            one("#work", "  Work ", " clients //  acme ").as_deref(),
            Some("#clients/acme")
        );
    }

    #[test]
    fn every_occurrence_in_a_note_is_rewritten() {
        assert_eq!(
            one("#work a #WORK b #work/x c #work", "work", "job").as_deref(),
            Some("#job a #job b #job/x c #job")
        );
    }

    #[test]
    fn results_follow_input_order() {
        let notes = [note(3, "#work"), note(1, "x"), note(2, "#work/a")];
        assert_eq!(
            rename(&notes, "work", "job").unwrap(),
            [(id(3), "#job".to_owned()), (id(2), "#job/a".to_owned())]
        );
    }

    #[test]
    fn nested_old_name_only_matches_that_subtree() {
        assert_eq!(
            one("#work #work/acme/q3 #work/beta", "work/acme", "acme").as_deref(),
            Some("#work #acme/q3 #work/beta")
        );
        assert_eq!(one("#work", "work/acme", "acme"), None);
    }

    #[test]
    fn multi_word_segment_anywhere_uses_the_closed_form() {
        assert_eq!(
            one("#work/x y", "work", "side jobs").as_deref(),
            Some("#side jobs/x# y")
        );
        assert_eq!(
            one("#work/acme# x", "work", "job").as_deref(),
            Some("#job/acme# x")
        );
    }

    #[test]
    fn rewritten_tags_use_normalized_spelling() {
        assert_eq!(
            one("#work//acme/ x", "work", "job").as_deref(),
            Some("#job/acme/ x")
        );
        assert_eq!(
            one("#side   gig# x", "side gig", "ideas").as_deref(),
            Some("#ideas x")
        );
    }

    #[test]
    fn punctuation_and_surroundings_are_kept() {
        assert_eq!(
            one("Done: #work). Next", "work", "job").as_deref(),
            Some("Done: #job). Next")
        );
        assert_eq!(
            one("## Plans #work ##", "work", "job").as_deref(),
            Some("## Plans #job ##")
        );
        assert_eq!(
            one("see https://x.com/#work and #work", "work", "job").as_deref(),
            Some("see https://x.com/#work and #job")
        );
    }

    #[test]
    fn tag_only_in_code_is_omitted() {
        assert_eq!(one("`#work` and\n\n    #work\n", "work", "job"), None);
    }

    #[test]
    fn case_only_rename_changes_spelling() {
        assert_eq!(
            one("#work/acme #WORK", "work", "Work").as_deref(),
            Some("#Work/acme #Work")
        );
        assert_eq!(one("#Work", "work", "Work"), None);
    }

    #[test]
    fn notes_are_not_modified() {
        let notes = [note(1, "#work")];
        rename(&notes, "work", "job").unwrap();
        assert_eq!(notes[0].body(), "#work");
    }

    #[test]
    fn round_trip() {
        let bodies = [
            "# Notes #work\nSee #work/acme, #Work/beta# and `#work`.\n\n```\n#work\n```\n#workshop #home/work",
            "#work\n- item #work/side gig# done\n> quote #WORK!",
            "Mixed #café/work #work/日本語 #work/x.",
            "#side projects# and #side projects/q3#?",
        ];
        let renames = [
            ("work", "job"),
            ("work", "side jobs"),
            ("work", "clients/acme"),
            ("side projects", "ideas"),
            ("work/acme", "archive/acme"),
        ];
        for body in bodies {
            for (old, new) in renames {
                let old_path = TagPath::parse(old).unwrap();
                let new_path = TagPath::parse(new).unwrap();
                let before = parse(body);
                let Some(after_body) = one(body, old, new) else {
                    assert!(before.iter().all(|o| !old_path.is_prefix_of(&o.path)));
                    continue;
                };
                let after = parse(&after_body);
                assert_eq!(
                    before.len(),
                    after.len(),
                    "{body:?} {old}->{new}: {after_body:?}"
                );

                let mut gaps_before = Vec::new();
                let mut gaps_after = Vec::new();
                let (mut pb, mut pa) = (0, 0);
                for (b, a) in before.iter().zip(&after) {
                    let expected = if old_path.is_prefix_of(&b.path) {
                        let segments: Vec<String> = new_path
                            .segments()
                            .iter()
                            .chain(&b.path.segments()[old_path.depth()..])
                            .cloned()
                            .collect();
                        TagPath::from_segments(segments)
                    } else {
                        assert_eq!(&body[b.range.clone()], &after_body[a.range.clone()]);
                        b.path.clone()
                    };
                    assert_eq!(a.path, expected, "{body:?} {old}->{new}: {after_body:?}");
                    assert_eq!(a.path.to_string(), expected.to_string());
                    gaps_before.push(&body[pb..b.range.start]);
                    gaps_after.push(&after_body[pa..a.range.start]);
                    (pb, pa) = (b.range.end, a.range.end);
                }
                gaps_before.push(&body[pb..]);
                gaps_after.push(&after_body[pa..]);
                assert_eq!(gaps_before, gaps_after, "{body:?} {old}->{new}");
            }
        }
    }
}
