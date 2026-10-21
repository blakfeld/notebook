//! Finding tag occurrences in a note body.

use std::collections::HashSet;
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
    /// name and any closing `#`. Trailing punctuation is outside the range,
    /// except in the multi-word form, where punctuation before the closing
    /// `#` (`#side projects.#`) is inside it.
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
    let mut seen = HashSet::new();
    parse(body)
        .into_iter()
        .map(|o| o.path)
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// Reads a tag starting at the `#` at `start`, looking no further than
/// `limit` (the end of the line or the start of the next code range).
fn scan_tag(body: &str, start: usize, limit: usize) -> Option<TagOccurrence> {
    let rest = &body[start + 1..limit];
    let first = rest.chars().next()?;
    if first.is_whitespace() || first == '#' {
        return None;
    }

    let word_end = rest
        .find(|c: char| c.is_whitespace() || c == '#')
        .unwrap_or(rest.len());
    let name = trim_name(&rest[..word_end]);
    // A multi-word tag must start the way a single-word tag would, so
    // `#1 among C# devs` is not a tag.
    let path = TagPath::from_raw(name)?;

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
pub(super) fn trim_name(name: &str) -> &str {
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
            "Ranked #1 among C# devs",
            "#. and then x#",
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

    fn ranges(body: &str) -> Vec<&str> {
        parse(body).into_iter().map(|o| &body[o.range]).collect()
    }

    fn forms(body: &str) -> Vec<TagForm> {
        parse(body).into_iter().map(|o| o.form).collect()
    }

    #[test]
    fn empty_and_tagless_bodies() {
        assert!(parse("").is_empty());
        assert!(tags_of("").is_empty());
        assert!(parse("just some text").is_empty());
        assert!(parse("ends with #").is_empty());
    }

    #[test]
    fn first_spelling_is_kept() {
        assert_eq!(distinct("#Todo and #todo"), ["Todo"]);
        assert_eq!(distinct("#work/Acme #Work/acme"), ["work/Acme"]);
    }

    #[test]
    fn every_trailing_punctuation_character_is_excluded() {
        for &c in crate::tags::TRAILING_PUNCTUATION {
            let body = format!("#tag{c}");
            assert_eq!(ranges(&body), ["#tag"], "{body:?}");
            let body = format!("#tag{c}{c}{c} more");
            assert_eq!(ranges(&body), ["#tag"], "{body:?}");
        }
    }

    #[test]
    fn punctuation_inside_a_name_is_kept() {
        assert_eq!(names("#e.g.x and #a:b"), ["e.g.x", "a:b"]);
        assert_eq!(
            names("#to-do #snake_case #c++"),
            ["to-do", "snake_case", "c++"]
        );
    }

    #[test]
    fn tag_boundaries() {
        assert_eq!(names("a\t#tab"), ["tab"]);
        assert_eq!(names("line\r\n#crlf\r\nnext"), ["crlf"]);
        assert_eq!(names("nbsp\u{a0}#tag"), ["tag"]);
        assert_eq!(names("#a #b\t#c\n#d"), ["a", "b", "c", "d"]);
        assert_eq!(names("#end"), ["end"]);
        assert_eq!(names("x#y"), Vec::<String>::new());
        assert_eq!(names("[#link](u) \"#quoted\""), Vec::<String>::new());
    }

    #[test]
    fn markdown_containers_do_not_hide_tags() {
        assert_eq!(names("- #item\n* #star\n1. #num"), ["item", "star", "num"]);
        assert_eq!(names("> #quoted"), ["quoted"]);
        assert_eq!(names("**#bold**"), Vec::<String>::new());
        assert_eq!(names("Text **bold** #after"), ["after"]);
    }

    #[test]
    fn ranges_are_byte_offsets() {
        let body = "é日 #tag";
        let occ = parse(body);
        assert_eq!(occ[0].range, 6..10);
        assert_eq!(&body[occ[0].range.clone()], "#tag");

        let body = "#日本語 x";
        assert_eq!(parse(body)[0].range, 0..10);
    }

    #[test]
    fn closed_form_boundaries() {
        assert_eq!(ranges("#ideas#."), ["#ideas#"]);
        assert_eq!(forms("#ideas#."), [TagForm::Closed]);
        assert_eq!(ranges("#ideas#"), ["#ideas#"]);
        assert_eq!(ranges("#ideas#\nnext"), ["#ideas#"]);
        assert_eq!(ranges("#a/b# x"), ["#a/b#"]);
        assert_eq!(forms("#a/b# x"), [TagForm::Closed]);
    }

    #[test]
    fn hash_not_at_a_boundary_ends_a_bare_tag() {
        assert_eq!(ranges("#ideas#x"), ["#ideas"]);
        assert_eq!(forms("#ideas#x"), [TagForm::Bare]);
        assert_eq!(names("#a#b #c"), ["a", "c"]);
    }

    #[test]
    fn trimmed_name_is_not_closed() {
        // The `#` after trimmed punctuation isn't directly after the name.
        assert_eq!(ranges("#ideas.# x"), ["#ideas"]);
        assert_eq!(forms("#ideas.# x"), [TagForm::Bare]);
    }

    #[test]
    fn trailing_slashes_are_outside_the_range() {
        assert_eq!(ranges("#a/b/ x"), ["#a/b"]);
        assert_eq!(ranges("#a/b/. x"), ["#a/b"]);
    }

    #[test]
    fn multi_word_ranges_and_boundaries() {
        assert_eq!(ranges("#side projects.# ok"), ["#side projects.#"]);
        assert_eq!(ranges("(see #big idea#)"), ["#big idea#"]);
        assert_eq!(ranges("#big idea#\r\nnext"), ["#big idea#"]);
        assert_eq!(forms("#big idea#"), [TagForm::MultiWord]);
        assert_eq!(names("#a\tb#"), ["a b"]);
    }

    #[test]
    fn multi_word_candidates_that_fall_back() {
        // Whitespace before the closing `#`.
        assert_eq!(names("#a b # c"), ["a"]);
        // Closing `#` followed by a non-boundary.
        assert_eq!(names("#a b#c d"), ["a"]);
        // A `#` with whitespace before it ends the first word instead.
        assert_eq!(names("#work and #home#"), ["work", "home"]);
    }

    #[test]
    fn multi_word_with_two_tags_on_a_line() {
        assert_eq!(
            names("#one two# and #three four#"),
            ["one two", "three four"]
        );
        assert_eq!(names("#one two# #three"), ["one two", "three"]);
    }

    #[test]
    fn more_code_forms() {
        assert_eq!(names("~~~\n#fake\n~~~\n#real"), ["real"]);
        assert_eq!(names("``#a ` #b``  #c"), ["c"]);
        assert_eq!(names("```\n#open fence never closes"), Vec::<String>::new());
        assert_eq!(names("#before `code` #after"), ["before", "after"]);
        assert_eq!(
            names("- item\n\n      #indented in list\n\n#real"),
            ["real"]
        );
    }

    #[test]
    fn tag_stops_at_adjacent_code() {
        assert_eq!(ranges("#a`b`"), ["#a"]);
        assert_eq!(names("#multi word`#x`#"), ["multi"]);
    }

    #[test]
    fn code_ranges_are_sorted_and_disjoint() {
        let body = "`a` and `b`\n\n```\nc\n```\n\n    d\n";
        let ranges = code_ranges(body);
        assert_eq!(ranges.len(), 4);
        for pair in ranges.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
        assert!(code_ranges("no code here").is_empty());
    }
}
