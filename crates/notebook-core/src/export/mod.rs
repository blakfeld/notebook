//! Markdown export of decrypted notes, the user's own plaintext backup.
//!
//! [`plan`] turns notes, options, and the export time into the exact list of
//! files to write; [`write_zip`] and (natively) [`write_dir`] only serialize a
//! plan, so every output holds the same export. The format is also described
//! in `docs/export-format.md`.
//!
//! # Format
//!
//! All files sit directly in one folder named `notebook-export-YYYYMMDD-HHMMSS`
//! after the export time in UTC; tags never create subfolders. Each note is
//! one UTF-8 `<name>.md` file: a frontmatter block, then the body exactly as
//! stored (inline tags, line endings, and all):
//!
//! ```text
//! ---
//! id: 01JBX3Z7Q8M4N5P6R7S8T9V0W1
//! created: 2024-10-21T14:03:05.123Z
//! updated: 2024-10-22T14:03:05.000Z
//! pinned: true
//! archived: true
//! trashed: false
//! ---
//! <body>
//! ```
//!
//! The keys are always these six, in this order, unquoted. Times are UTC
//! with milliseconds. An importer recovers the body by dropping everything up
//! to and including the second `---\n` line; that works even when the body
//! itself starts with `---`, because the frontmatter never contains one.
//!
//! By default every note that is not trashed (active and archived) is
//! exported; [`ExportOptions::include_trashed`] adds trashed notes.
//!
//! # Names
//!
//! A file's name is the note's title with `/ \ : * ? " < > | # ^ [ ]` and
//! control characters replaced by spaces, whitespace runs collapsed, leading
//! dots and spaces and trailing dots and spaces removed, and cut to 120 UTF-8
//! bytes on a character boundary. An empty result is `Untitled`; a Windows
//! reserved device name (`CON`, `NUL`, `COM1`, …) gets `_` appended. Notes
//! are named in ascending identifier order, and a name already taken
//! (ignoring case and normalization form) gets ` 2`, ` 3`, … with the
//! smallest free number.

mod name;
mod time;
mod zip;

#[cfg(not(target_arch = "wasm32"))]
mod dir;

use std::fmt;

#[cfg(not(target_arch = "wasm32"))]
pub use dir::write_dir;
pub use zip::write_zip;

use crate::note::{Note, NoteId, Timestamp};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Also export trashed notes.
    pub include_trashed: bool,
}

/// One note's file in an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFile {
    /// File name including `.md`, unique within the export ignoring case.
    pub name: String,
    pub contents: String,
    /// The note's updated time, used as the file's modification time.
    pub modified: Timestamp,
    pub id: NoteId,
}

/// Everything one export writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlan {
    /// `notebook-export-YYYYMMDD-HHMMSS`.
    pub folder_name: String,
    /// Sorted by name.
    pub files: Vec<ExportFile>,
    /// When the export was made.
    pub exported: Timestamp,
}

impl ExportPlan {
    /// The file name for the zip output: `<folder name>.zip`.
    pub fn zip_name(&self) -> String {
        format!("{}.zip", self.folder_name)
    }
}

/// Plans an export of `notes` made at `now`.
pub fn plan(notes: &[Note], options: ExportOptions, now: Timestamp) -> ExportPlan {
    let mut selected: Vec<&Note> = notes
        .iter()
        .filter(|n| options.include_trashed || !n.is_trashed())
        .collect();
    selected.sort_by_key(|n| n.id());
    let names = name::assign(selected.iter().map(|n| name::sanitize(n.title())));
    let mut files: Vec<ExportFile> = selected
        .into_iter()
        .zip(names)
        .map(|(note, name)| ExportFile {
            name: format!("{name}.md"),
            contents: render(note),
            modified: note.updated(),
            id: note.id(),
        })
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    ExportPlan {
        folder_name: format!("notebook-export-{}", time::folder_stamp(now)),
        files,
        exported: now,
    }
}

fn render(note: &Note) -> String {
    format!(
        "---\nid: {}\ncreated: {}\nupdated: {}\npinned: {}\narchived: {}\ntrashed: {}\n---\n{}",
        note.id(),
        time::rfc3339(note.created()),
        time::rfc3339(note.updated()),
        note.is_pinned(),
        note.is_archived(),
        note.is_trashed(),
        note.body(),
    )
}

/// Error writing an export.
#[derive(Debug)]
pub enum ExportError {
    Io(std::io::Error),
    Zip(::zip::result::ZipError),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Zip(e) => write!(f, "building the export zip failed: {e}"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Zip(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<::zip::result::ZipError> for ExportError {
    fn from(e: ::zip::result::ZipError) -> Self {
        Self::Zip(e)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn id(n: u128) -> NoteId {
        NoteId::from_parts(1_700_000_000_000, n)
    }

    pub(crate) fn note(n: u128, body: &str) -> Note {
        Note::new(id(n), body, Timestamp(1_729_519_385_123))
    }

    /// What an importer does: drop everything up to and including the
    /// closing `---\n` line after the opening one.
    fn read_body(contents: &str) -> &str {
        let rest = contents.strip_prefix("---\n").expect("opening line");
        let end = rest.find("\n---\n").expect("closing line");
        &rest[end + "\n---\n".len()..]
    }

    fn names(plan: &ExportPlan) -> Vec<&str> {
        plan.files.iter().map(|f| f.name.as_str()).collect()
    }

    fn abc() -> Vec<Note> {
        let a = note(1, "A");
        let mut b = note(2, "B");
        b.set_archived(true);
        let mut c = note(3, "C");
        c.set_trashed(true);
        vec![a, b, c]
    }

    #[test]
    fn default_selection() {
        let plan = plan(&abc(), ExportOptions::default(), Timestamp(0));
        assert_eq!(names(&plan), ["A.md", "B.md"]);
    }

    #[test]
    fn including_trashed_notes() {
        let options = ExportOptions {
            include_trashed: true,
        };
        let plan = plan(&abc(), options, Timestamp(0));
        assert_eq!(names(&plan), ["A.md", "B.md", "C.md"]);
        assert!(plan.files[2].contents.contains("\ntrashed: true\n"));
    }

    #[test]
    fn empty_vault() {
        let plan = plan(&[], ExportOptions::default(), Timestamp(0));
        assert!(plan.files.is_empty());
        assert_eq!(plan.folder_name, "notebook-export-19700101-000000");
    }

    #[test]
    fn folder_name_from_export_time() {
        let plan = plan(&[], ExportOptions::default(), Timestamp(1_792_591_385_000));
        assert_eq!(plan.folder_name, "notebook-export-20261021-140305");
        assert_eq!(plan.zip_name(), "notebook-export-20261021-140305.zip");
    }

    #[test]
    fn nested_tags_do_not_create_folders() {
        let plan = plan(
            &[note(1, "Standup #work/acme")],
            ExportOptions::default(),
            Timestamp(0),
        );
        assert_eq!(names(&plan), ["Standup work acme.md"]);
        assert!(!plan.files[0].name.contains('/'));
    }

    #[test]
    fn frontmatter_golden() {
        let id: NoteId = "01JBX3Z7Q8M4N5P6R7S8T9V0W1".parse().unwrap();
        let n = Note::from_parts(
            id,
            "Body".to_owned(),
            Timestamp(1_729_519_385_123),
            Timestamp(1_729_605_785_000),
            true,
            true,
            false,
        );
        let plan = plan(&[n], ExportOptions::default(), Timestamp(0));
        assert_eq!(
            plan.files[0].contents,
            "---\nid: 01JBX3Z7Q8M4N5P6R7S8T9V0W1\ncreated: 2024-10-21T14:03:05.123Z\n\
             updated: 2024-10-22T14:03:05.000Z\npinned: true\narchived: true\n\
             trashed: false\n---\nBody"
        );
        assert_eq!(plan.files[0].modified, Timestamp(1_729_605_785_000));
        assert_eq!(plan.files[0].id, id);
    }

    #[test]
    fn body_preserved_byte_for_byte() {
        let bodies = [
            "Café 日本語  \r\nline two\t \r\n\r\n",
            "Ideas for #side project# and #work/acme",
            "---\nnot: frontmatter\n---\nstill body",
            "",
        ];
        let notes: Vec<Note> = bodies
            .iter()
            .enumerate()
            .map(|(i, b)| note(i as u128, b))
            .collect();
        let plan = plan(&notes, ExportOptions::default(), Timestamp(0));
        for (i, body) in bodies.iter().enumerate() {
            let file = plan.files.iter().find(|f| f.id == id(i as u128)).unwrap();
            assert_eq!(read_body(&file.contents), *body);
            assert!(file.contents.starts_with("---\nid: "));
        }
    }

    #[test]
    fn empty_body_is_frontmatter_only() {
        let plan = plan(&[note(1, "")], ExportOptions::default(), Timestamp(0));
        let file = &plan.files[0];
        assert_eq!(file.name, "Untitled.md");
        assert!(file.contents.ends_with("trashed: false\n---\n"));
        assert_eq!(file.contents.matches("---\n").count(), 2);
    }

    #[test]
    fn files_are_sorted_and_named_in_identifier_order() {
        // Passed out of order; the older note keeps the plain name.
        let notes = [note(3, "Ideas"), note(1, "zebra"), note(2, "Ideas")];
        let plan = plan(&notes, ExportOptions::default(), Timestamp(0));
        assert_eq!(names(&plan), ["Ideas 2.md", "Ideas.md", "zebra.md"]);
        assert_eq!(plan.files[1].id, id(2));
        assert_eq!(plan.files[0].id, id(3));
    }

    #[test]
    fn repeat_export_is_identical() {
        let notes = [note(1, "todo"), note(2, "TODO"), note(3, "x")];
        let a = plan(&notes, ExportOptions::default(), Timestamp(5));
        let b = plan(&notes, ExportOptions::default(), Timestamp(5));
        assert_eq!(a, b);
    }
}
