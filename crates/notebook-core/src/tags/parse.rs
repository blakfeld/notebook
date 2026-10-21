//! Finding tag occurrences in a note body.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag};

use super::{TagPath, is_trailing_punctuation};

/// How a tag occurrence was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagForm {
    /// `#name`
    Bare,
    /// `#name#`, a single-word tag with a closing `#`.
    Closed,
    /// `#name with spaces#`
    MultiWord,
}

/// One tag written in a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagOccurrence {
    /// Byte range of the tag in the body, from the opening `#` through the
    /// name and any closing `#`. Trailing punctuation is outside the range.
    pub range: Range<usize>,
    pub path: TagPath,
    pub form: TagForm,
}

/// Returns every tag occurrence in `body`, in source order.
pub fn parse(body: &str) -> Vec<TagOccurrence> {
    let excluded = code_ranges(body);
    let mut excluded = excluded.iter().peekable();
    let mut occurrences = Vec::new();
    let mut i = 0;
    while let Some(c) = body[i..].chars().next() {
        while excluded.next_if(|r| r.end <= i).is_some() {}
        if let Some(r) = excluded.peek().filter(|r| r.start <= i) {
            i = r.end;
            continue;
        }
        if c == '#'
            && body[..i]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
        {
            let line_end = body[i..].find('\n').map_or(body.len(), |n| i + n);
            let limit = excluded.peek().map_or(line_end, |r| r.start.min(line_end));
            if let Some(occurrence) = scan_tag(body, i, limit) {
                i = occurrence.range.end;
                occurrences.push(occurrence);
                continue;
            }
        }
        i += c.len_utf8();
    }
    occurrences
}

/// Returns the distinct tags in `body`, in order of first occurrence. When a
/// tag is written in several spellings, the first one is kept.
pub fn tags_of(body: &str) -> Vec<TagPath> {
    let mut tags: Vec<TagPath> = Vec::new();
    for occurrence in parse(body) {
        if !tags.contains(&occurrence.path) {
            tags.push(occurrence.path);
        }
    }
    tags
}

/// Reads a tag starting at the `#` at `start`, looking no further than
/// `limit` (the end of the line or the start of the next code range).
fn scan_tag(body: &str, start: usize, limit: usize) -> Option<TagOccurrence> {
    let rest = &body[start + 1..limit];
    let first = rest.chars().next()?;
    if first.is_whitespace() || first == '#' {
        return None;
    }

    // Multi-word form: the next `#` on the line closes the tag if it follows
    // a non-whitespace character and precedes a boundary.
    if let Some(close) = rest.find('#') {
        let content = &rest[..close];
        if content.contains(char::is_whitespace)
            && !content.ends_with(char::is_whitespace)
            && is_boundary(rest[close + 1..].chars().next())
            && let Some(path) = TagPath::from_raw(trim_name(content))
        {
            return Some(TagOccurrence {
                range: start..start + 1 + close + 1,
                path,
                form: TagForm::MultiWord,
            });
        }
    }

    let word_end = rest
        .find(|c: char| c.is_whitespace() || c == '#')
        .unwrap_or(rest.len());
    let name = trim_name(&rest[..word_end]);
    let path = TagPath::from_raw(name)?;
    let mut end = start + 1 + name.len();
    let mut form = TagForm::Bare;
    if name.len() == word_end
        && rest[word_end..].starts_with('#')
        && is_boundary(rest[word_end + 1..].chars().next())
    {
        end += 1;
        form = TagForm::Closed;
    }
    Some(TagOccurrence {
        range: start..end,
        path,
        form,
    })
}

/// Whether the character after a closing `#` lets it close a tag.
fn is_boundary(next: Option<char>) -> bool {
    next.is_none_or(|c| c.is_whitespace() || is_trailing_punctuation(c))
}

/// Strips trailing punctuation, `/`, and whitespace from a raw name.
fn trim_name(name: &str) -> &str {
    name.trim_end_matches(|c: char| c == '/' || c.is_whitespace() || is_trailing_punctuation(c))
}

/// Byte ranges of inline code spans and fenced or indented code blocks,
/// sorted and non-overlapping.
fn code_ranges(body: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (event, range) in Parser::new(body).into_offset_iter() {
        if matches!(event, Event::Code(_) | Event::Start(Tag::CodeBlock(_)))
            && ranges.last().is_none_or(|last| last.end <= range.start)
        {
            ranges.push(range);
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(body: &str) -> Vec<String> {
        parse(body)
            .into_iter()
            .map(|o| o.path.to_string())
            .collect()
    }

    fn distinct(body: &str) -> Vec<String> {
        tags_of(body).into_iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn code_ranges_cover_code() {
        let body = "a `#x` b";
        assert_eq!(
            code_ranges(body)
                .iter()
                .map(|r| &body[r.clone()])
                .collect::<Vec<_>>(),
            ["`#x`"]
        );

        let body = "#real\n```\n#fake\n```\n";
        let ranges = code_ranges(body);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&body[ranges[0].clone()], "```\n#fake\n```");

        let body = "Intro\n\n    #fake\n\n#real";
        let ranges = code_ranges(body);
        assert_eq!(ranges.len(), 1);
        assert!(body[ranges[0].clone()].contains("#fake"));
        assert!(!body[ranges[0].clone()].contains("#real"));
    }

    #[test]
    fn single_word_tags() {
        assert_eq!(names("Notes for #work today"), ["work"]);
        assert_eq!(names("#recipes\nPancakes"), ["recipes"]);
        assert_eq!(
            names("Filed under #reading, then #todo."),
            ["reading", "todo"]
        );
        assert_eq!(names("#café and #日本語"), ["café", "日本語"]);
        assert_eq!(names("(#todo)!"), Vec::<String>::new());
        assert_eq!(names("see #todo)."), ["todo"]);
    }

    #[test]
    fn closing_hash_on_single_word() {
        let occ = parse("#ideas# for later");
        assert_eq!(occ.len(), 1);
        assert_eq!(occ[0].path.to_string(), "ideas");
        assert_eq!(occ[0].range, 0..7);
        assert_eq!(occ[0].form, TagForm::Closed);
    }

    #[test]
    fn occurrence_ranges_exclude_punctuation() {
        let body = "See #ideas.";
        let occ = parse(body);
        assert_eq!(&body[occ[0].range.clone()], "#ideas");
        assert_eq!(occ[0].form, TagForm::Bare);
    }

    #[test]
    fn not_tags() {
        for body in [
            "# Weekly review",
            "See https://example.com/page#section and <https://example.com/#top>",
            "C# and F#minor",
            "\\#notatag",
            "Fixed in #42 and #2024",
            "# ",
            "##",
            "#.",
        ] {
            assert_eq!(names(body), Vec::<String>::new(), "{body:?}");
        }
    }

    #[test]
    fn tags_in_heading_text() {
        assert_eq!(names("## Standup #work"), ["work"]);
        assert_eq!(names("# Title ##"), Vec::<String>::new());
    }

    #[test]
    fn digits_with_other_characters() {
        assert_eq!(names("#2024/q1 and #3d"), ["2024/q1", "3d"]);
    }

    #[test]
    fn code_is_ignored() {
        assert_eq!(names("Use `#define` here and #c"), ["c"]);
        assert_eq!(names("#real\n```\n#fake\n```\n"), ["real"]);
        assert_eq!(names("Intro\n\n    #fake\n\n#real"), ["real"]);
        assert_eq!(names("#a `b` #c"), ["a", "c"]);
    }

    #[test]
    fn nested_tags() {
        let occ = parse("#work/acme/q3");
        assert_eq!(occ[0].path.segments(), ["work", "acme", "q3"]);
        assert_eq!(names("#a//b/ and #/"), ["a/b"]);
    }

    #[test]
    fn multi_word_tags() {
        let body = "Ideas for #side project# this week";
        let occ = parse(body);
        assert_eq!(occ.len(), 1);
        assert_eq!(occ[0].path.to_string(), "side project");
        assert_eq!(&body[occ[0].range.clone()], "#side project#");
        assert_eq!(occ[0].form, TagForm::MultiWord);

        assert_eq!(
            parse("#reading list/to buy#")[0].path.segments(),
            ["reading list", "to buy"]
        );
        assert_eq!(names("#side   project#."), ["side project"]);
        assert_eq!(names("#side projects.# ok"), ["side projects"]);
    }

    #[test]
    fn multi_word_fallbacks() {
        assert_eq!(names("#work and then #home"), ["work", "home"]);
        assert_eq!(names("#first line\nsecond#"), ["first"]);
        assert_eq!(names("#foo bar#baz"), ["foo"]);
        assert_eq!(names("#a `b c#`"), ["a"]);
    }

    #[test]
    fn duplicate_tags_are_distinct_once() {
        assert_eq!(distinct("#todo first, #TODO second"), ["todo"]);
        assert_eq!(distinct("#b #a #b"), ["b", "a"]);
    }
}
