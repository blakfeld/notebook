//! The note entity: identity, markdown body, derived title, timestamps, and
//! pinned/archived/trashed states.
//!
//! The note model does no I/O. Callers supply the current time and, when
//! creating identifiers, the random bits, so the same code runs natively and
//! on `wasm32-unknown-unknown` with deterministic tests.

use std::fmt;
use std::str::FromStr;

use ulid::Ulid;

/// Stable identifier of a note: a ULID assigned once at creation.
///
/// Identifiers order by creation time first. The text form is the
/// 26-character Crockford base32 ULID encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(Ulid);

impl NoteId {
    /// Builds an identifier from a creation time in Unix milliseconds and 80
    /// bits of caller-supplied randomness (higher bits are ignored).
    pub fn from_parts(timestamp_ms: u64, random: u128) -> Self {
        Self(Ulid::from_parts(timestamp_ms, random))
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error returned when text is not a valid note identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseNoteIdError;

impl fmt::Display for ParseNoteIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid note identifier")
    }
}

impl std::error::Error for ParseNoteIdError {}

impl FromStr for NoteId {
    type Err = ParseNoteIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(Self).map_err(|_| ParseNoteIdError)
    }
}

/// A UTC instant in Unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Returns the instant as Unix milliseconds.
    pub fn as_millis(self) -> i64 {
        self.0
    }
}

/// A note: a markdown body plus metadata.
///
/// `updated` changes only when the body changes; pinning, archiving, and
/// trashing leave it alone. The three state flags are independent, so a note
/// restored from the trash returns to its previous pinned and archived state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    id: NoteId,
    body: String,
    created: Timestamp,
    updated: Timestamp,
    pinned: bool,
    archived: bool,
    trashed: bool,
}

impl Note {
    /// Creates a note at `now` with all state flags cleared.
    pub fn new(id: NoteId, body: impl Into<String>, now: Timestamp) -> Self {
        Self {
            id,
            body: body.into(),
            created: now,
            updated: now,
            pinned: false,
            archived: false,
            trashed: false,
        }
    }

    pub fn id(&self) -> NoteId {
        self.id
    }

    /// The markdown body, exactly as written.
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn created(&self) -> Timestamp {
        self.created
    }

    /// When the body last changed.
    pub fn updated(&self) -> Timestamp {
        self.updated
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn is_archived(&self) -> bool {
        self.archived
    }

    pub fn is_trashed(&self) -> bool {
        self.trashed
    }

    /// Replaces the body. If the text differs, `updated` becomes `now`.
    pub fn set_body(&mut self, body: impl Into<String>, now: Timestamp) {
        let body = body.into();
        if body != self.body {
            self.body = body;
            self.updated = now;
        }
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }

    pub fn set_archived(&mut self, archived: bool) {
        self.archived = archived;
    }

    pub fn set_trashed(&mut self, trashed: bool) {
        self.trashed = trashed;
    }

    /// The title, derived from the first line containing non-whitespace text
    /// with ATX heading markers removed. Empty when there is no such line or
    /// it holds only heading markers; clients choose a placeholder.
    pub fn title(&self) -> &str {
        derive_title(&self.body)
    }
}

fn derive_title(body: &str) -> &str {
    let Some(line) = body.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return "";
    };
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let rest = &line[hashes..];
    let is_heading =
        (1..=6).contains(&hashes) && (rest.is_empty() || rest.starts_with([' ', '\t']));
    if !is_heading {
        return line;
    }
    let text = rest.trim();
    // An ATX closing sequence is a run of `#` that is the whole text or is
    // preceded by whitespace.
    let without_closing = text.trim_end_matches('#');
    if without_closing.is_empty() || without_closing.ends_with([' ', '\t']) {
        without_closing.trim_end()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: Timestamp = Timestamp(1_000);
    const T2: Timestamp = Timestamp(2_000);
    const T3: Timestamp = Timestamp(3_000);

    fn id(n: u128) -> NoteId {
        NoteId::from_parts(1_000, n)
    }

    fn note(body: &str) -> Note {
        Note::new(id(1), body, T1)
    }

    #[test]
    fn id_round_trips_through_text() {
        let id = NoteId::from_parts(1_700_000_000_000, 0xdead_beef);
        let text = id.to_string();
        assert_eq!(text.len(), 26);
        assert_eq!(text.parse::<NoteId>(), Ok(id));
    }

    #[test]
    fn invalid_id_text_is_rejected() {
        for text in [
            "",
            "not-a-ulid",
            "01ARZ3NDEKTSV4RRFFQ69G5FA",
            "01ARZ3NDEKTSV4RRFFQ69G5FAVX",
        ] {
            assert_eq!(text.parse::<NoteId>(), Err(ParseNoteIdError), "{text:?}");
        }
    }

    #[test]
    fn ids_order_by_creation_time() {
        let earlier = NoteId::from_parts(1_000, u128::MAX);
        let later = NoteId::from_parts(2_000, 0);
        assert!(earlier < later);
        assert!(earlier.to_string() < later.to_string());
    }

    #[test]
    fn id_survives_edits_and_state_changes() {
        let mut n = note("a");
        n.set_body("b", T2);
        n.set_pinned(true);
        n.set_archived(true);
        n.set_trashed(true);
        assert_eq!(n.id(), id(1));
    }

    #[test]
    fn body_is_preserved_byte_for_byte() {
        let body = "Héllo wörld  \r\nsecond\t \r\n日本語";
        assert_eq!(note(body).body(), body);
    }

    #[test]
    fn empty_note() {
        let n = note("");
        assert_eq!(n.body(), "");
        assert_eq!(n.title(), "");
    }

    #[test]
    fn new_note_timestamps_and_flags() {
        let n = note("x");
        assert_eq!(n.created(), T1);
        assert_eq!(n.updated(), T1);
        assert!(!n.is_pinned() && !n.is_archived() && !n.is_trashed());
    }

    #[test]
    fn body_edit_bumps_updated_but_not_created() {
        let mut n = note("draft");
        n.set_body("final", T2);
        assert_eq!(n.body(), "final");
        assert_eq!(n.created(), T1);
        assert_eq!(n.updated(), T2);
    }

    #[test]
    fn no_op_edit_leaves_updated_alone() {
        let mut n = note("same");
        n.set_body("same", T2);
        assert_eq!(n.updated(), T1);
    }

    #[test]
    fn state_changes_leave_updated_alone() {
        let mut n = note("x");
        n.set_pinned(true);
        n.set_archived(true);
        n.set_trashed(true);
        assert_eq!(n.updated(), T1);
        n.set_body("y", T3);
        assert_eq!(n.updated(), T3);
    }

    #[test]
    fn pin_and_unpin() {
        let mut n = note("x");
        n.set_pinned(true);
        assert!(n.is_pinned());
        n.set_pinned(false);
        assert!(!n.is_pinned());
    }

    #[test]
    fn archive_and_unarchive() {
        let mut n = note("x");
        n.set_archived(true);
        assert!(n.is_archived());
        n.set_archived(false);
        assert!(!n.is_archived());
    }

    #[test]
    fn restore_returns_archived_note_to_archive() {
        let mut n = note("x");
        n.set_archived(true);
        n.set_trashed(true);
        n.set_trashed(false);
        assert!(!n.is_trashed());
        assert!(n.is_archived());
    }

    #[test]
    fn trashing_keeps_the_pin() {
        let mut n = note("x");
        n.set_pinned(true);
        n.set_trashed(true);
        assert!(n.is_trashed());
        assert!(n.is_pinned());
    }

    #[test]
    fn titles() {
        let cases = [
            ("# Grocery list\n- eggs", "Grocery list"),
            (
                "\n\n  Call the plumber  \nabout the sink",
                "Call the plumber",
            ),
            ("## Ideas ##", "Ideas"),
            ("#work standup notes", "#work standup notes"),
            ("# C#", "C#"),
            ("# Title #", "Title"),
            ("####### seven", "####### seven"),
            ("Setext title\n===", "Setext title"),
            ("  \n\t\n", ""),
            ("#\nsecond line", ""),
            ("### ###", ""),
            ("\r\nWindows\r\n", "Windows"),
        ];
        for (body, title) in cases {
            assert_eq!(note(body).title(), title, "{body:?}");
        }
    }

    #[test]
    fn title_follows_body_edits() {
        let mut n = note("Draft\nbody");
        n.set_body("Final\nbody", T2);
        assert_eq!(n.title(), "Final");
    }
}
