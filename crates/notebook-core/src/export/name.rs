//! File names for exported notes: sanitizing titles and resolving
//! collisions.

use std::collections::HashSet;

use unicode_normalization::UnicodeNormalization;

/// Longest name, in UTF-8 bytes, before the collision suffix and `.md`.
const MAX_NAME_BYTES: usize = 120;

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters illegal in Windows file names, plus those that break Obsidian
/// wikilinks.
fn is_unsafe(c: char) -> bool {
    matches!(
        c,
        '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']'
    ) || c.is_control()
}

fn trim(name: &str) -> &str {
    name.trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
}

/// Turns a note title into a file name stem that is safe on Windows, macOS,
/// and Linux. Never empty.
pub(crate) fn sanitize(title: &str) -> String {
    let replaced: String = title
        .chars()
        .map(|c| if is_unsafe(c) { ' ' } else { c })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name = trim(&collapsed);
    if name.len() > MAX_NAME_BYTES {
        let mut end = MAX_NAME_BYTES;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name = trim(&name[..end]);
    }
    if name.is_empty() {
        return "Untitled".to_owned();
    }
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(name)) {
        return format!("{name}_");
    }
    name.to_owned()
}

/// How names compare for collisions: ignoring case and Unicode normalization
/// form, as case-insensitive filesystems do (APFS treats `é` and `e` +
/// combining acute as the same name). Upper- then lowercasing folds the
/// multi-character cases plain lowercasing misses (`ß` and `SS`, `σ` and
/// `ς`). The name itself is kept as typed.
fn collision_key(name: &str) -> String {
    let nfc: String = name.nfc().collect();
    nfc.to_uppercase().to_lowercase().nfc().collect()
}

/// Gives each stem, in order, its own name: the stem itself unless already
/// taken (see [`collision_key`]), otherwise the stem plus ` N` for the
/// smallest free N ≥ 2.
pub(crate) fn assign(stems: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut taken = HashSet::new();
    stems
        .into_iter()
        .map(|stem| {
            let mut name = stem.clone();
            let mut n = 2;
            while !taken.insert(collision_key(&name)) {
                name = format!("{stem} {n}");
                n += 1;
            }
            name
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assigned(stems: &[&str]) -> Vec<String> {
        assign(stems.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn plain_title() {
        assert_eq!(sanitize("Grocery list"), "Grocery list");
    }

    #[test]
    fn unsafe_characters() {
        assert_eq!(sanitize("Q3: plan/budget?"), "Q3 plan budget");
        assert_eq!(sanitize(r#"a\b*c"d<e>f|g^h[i]j"#), "a b c d e f g h i j");
        assert_eq!(sanitize("tab\there\u{7}bell"), "tab here bell");
    }

    #[test]
    fn tag_in_the_title() {
        assert_eq!(sanitize("#work standup notes"), "work standup notes");
    }

    #[test]
    fn non_ascii_title() {
        assert_eq!(sanitize("Café 日本語"), "Café 日本語");
    }

    #[test]
    fn case_is_kept() {
        assert_eq!(sanitize("MiXeD Case"), "MiXeD Case");
    }

    #[test]
    fn whitespace_and_dots_trimmed() {
        assert_eq!(sanitize("  a   b  "), "a b");
        assert_eq!(sanitize("..hidden"), "hidden");
        assert_eq!(sanitize(". . x"), "x");
        assert_eq!(sanitize("end..."), "end");
        assert_eq!(sanitize("middle.dots.kept"), "middle.dots.kept");
    }

    #[test]
    fn empty_title() {
        assert_eq!(sanitize(""), "Untitled");
        assert_eq!(sanitize("   "), "Untitled");
        assert_eq!(sanitize("#?/..."), "Untitled");
    }

    #[test]
    fn reserved_device_names() {
        assert_eq!(sanitize("con"), "con_");
        assert_eq!(sanitize("NUL"), "NUL_");
        assert_eq!(sanitize("Com7"), "Com7_");
        assert_eq!(sanitize("lpt9"), "lpt9_");
        assert_eq!(sanitize("COM10"), "COM10");
        assert_eq!(sanitize("console"), "console");
    }

    #[test]
    fn long_title() {
        let title = "é".repeat(150); // 300 bytes
        let name = sanitize(&title);
        assert!(name.len() <= MAX_NAME_BYTES);
        assert_eq!(name, "é".repeat(60));

        // A cut that lands just before a space is re-trimmed.
        let title = format!("{} {}", "a".repeat(119), "b".repeat(50));
        assert_eq!(sanitize(&title), "a".repeat(119));

        // A cut inside a multi-byte character backs up to its start.
        let title = format!("{}日本", "a".repeat(119));
        assert_eq!(sanitize(&title), "a".repeat(119));
    }

    #[test]
    fn two_notes_with_the_same_title() {
        assert_eq!(assigned(&["Ideas", "Ideas"]), ["Ideas", "Ideas 2"]);
    }

    #[test]
    fn case_insensitive_collision() {
        let names = assigned(&["todo", "TODO"]);
        assert_ne!(names[0].to_lowercase(), names[1].to_lowercase());
        assert_eq!(names, ["todo", "TODO 2"]);
        assert_eq!(assigned(&["Ärger", "ärger"]), ["Ärger", "ärger 2"]);
        // Folds that lowercasing alone misses.
        assert_eq!(assigned(&["Straße", "STRASSE"]), ["Straße", "STRASSE 2"]);
        assert_eq!(assigned(&["ΟΔΟΣ", "οδοσ"]), ["ΟΔΟΣ", "οδοσ 2"]);
    }

    #[test]
    fn normalization_insensitive_collision() {
        // Composed and decomposed `é` are one name on APFS; names keep their
        // own form.
        let names = assigned(&["Caf\u{e9}", "Cafe\u{301}"]);
        assert_eq!(names, ["Caf\u{e9}", "Cafe\u{301} 2"]);
    }

    #[test]
    fn suffix_does_not_reuse_an_existing_name() {
        assert_eq!(
            assigned(&["Ideas", "Ideas 2", "Ideas"]),
            ["Ideas", "Ideas 2", "Ideas 3"]
        );
    }

    #[test]
    fn repeat_assignment_is_identical() {
        let stems = ["a", "A", "b", "a 2", "a"];
        assert_eq!(assigned(&stems), assigned(&stems));
    }
}
