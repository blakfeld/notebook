//! The client-server sync protocol: request, response, and live-frame types
//! shared by `notebook-server` and the sync engine, so the two can't disagree.
//! This module has no networking.
//!
//! # Endpoints
//!
//! Every endpoint takes `Authorization: Bearer <access token>`; the vault is
//! the token's, never named in the request.
//!
//! | method + path | body | success |
//! |---|---|---|
//! | `POST /sync/v1/push` | JSON [`PushRequest`] | `200` JSON [`PushResponse`] |
//! | `GET /sync/v1/changes?after=<seq>` | none | `200` JSON [`ChangesResponse`] |
//! | `PUT /sync/v1/notes/{note_id}/snapshot?covers=<seq>` | raw ciphertext | `204` |
//! | `DELETE /sync/v1/notes/{note_id}` | none | `204` |
//! | `GET /sync/v1/live` | WebSocket upgrade | `101` |
//!
//! Errors are `{"error": "<code>"}` ([`ErrorBody`]). Payload bytes are
//! standard base64 with padding ([`Bytes`]); IDs are 26-character ULID text;
//! sequence numbers are JSON integers.
//!
//! # Cursors and pagination
//!
//! Every change in a vault has a sequence number: an update's `seq`, a
//! purge's `seq`, a snapshot's `covers`, and a note registration's first
//! update. `changes?after=C` returns the changes keyed above `C` in key order,
//! about 2 MiB per page; a page never splits the changes sharing one key, and
//! a single change larger than the budget comes alone. Apply the page, store
//! its `cursor`, and pull again while `more` is true. `after=0` is bootstrap.
//! A cursor above the vault's head is `409 cursor_ahead`: the server was
//! restored from a backup, so bootstrap again.
//!
//! Sequence numbers increase but are not contiguous. A note's wrapped key
//! ([`NoteEntry::key`]) arrives in the same page as, or an earlier page than,
//! any of that note's data.
//!
//! # Live channel
//!
//! Open `/sync/v1/live`, send [`ClientFrame::Auth`] within 10 seconds, and
//! wait for [`ServerFrame::Ready`]. Then pull from the stored cursor; every
//! change above it arrives by that pull or as a [`ServerFrame::Entry`].
//! Entries at or below the cursor are duplicates to drop. Send another `auth`
//! frame with a refreshed token before the current one expires.
//!
//! The server closes with one of these codes:
//!
//! | code | meaning |
//! |---|---|
//! | [`CLOSE_AUTH`] `4001` | authentication failed or the token expired: refresh and reconnect |
//! | [`CLOSE_RESYNC`] `4002` | fell behind: pull, then reconnect |
//! | [`CLOSE_TOO_MANY`] `4003` | the vault has too many live connections |
//! | [`CLOSE_SHUTDOWN`] `4004` | the server is shutting down |

use std::fmt;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ulid::Ulid;

use crate::auth::api::Token;
use crate::note::NoteId;

/// The most updates one push may carry; larger batches are `400
/// bad_request`, so split them. The server holds its write lock for a whole
/// batch, and a batch must fit the live channel's buffer.
pub const MAX_PUSH_UPDATES: usize = 128;

/// The largest accepted wrapped note key (a real one is 32 bytes plus
/// envelope overhead); a longer one is `400 bad_request`.
pub const MAX_NOTE_KEY_BYTES: usize = 256;

/// Authentication failed or the token expired.
pub const CLOSE_AUTH: u16 = 4001;
/// The connection fell behind; pull, then reconnect.
pub const CLOSE_RESYNC: u16 = 4002;
/// The vault already has the maximum number of live connections.
pub const CLOSE_TOO_MANY: u16 = 4003;
/// The server is shutting down.
pub const CLOSE_SHUTDOWN: u16 = 4004;

/// A client-generated update ID, used to make pushes idempotent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpId(Ulid);

impl OpId {
    /// Builds an ID from a time in Unix milliseconds and 80 bits of
    /// caller-supplied randomness (higher bits are ignored).
    pub fn from_parts(timestamp_ms: u64, random: u128) -> Self {
        Self(Ulid::from_parts(timestamp_ms, random))
    }

    pub fn to_bytes(self) -> [u8; 16] {
        self.0.to_bytes()
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Ulid::from_bytes(bytes))
    }
}

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for OpId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(Self)
    }
}

/// Serializes as ULID text.
fn ser_text<T: fmt::Display, S: Serializer>(value: &T, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(value)
}

fn de_text<'de, T, D>(d: D) -> Result<T, D::Error>
where
    T: FromStr,
    T::Err: fmt::Display,
    D: Deserializer<'de>,
{
    let text = String::deserialize(d)?;
    text.parse().map_err(serde::de::Error::custom)
}

impl Serialize for OpId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ser_text(self, s)
    }
}

impl<'de> Deserialize<'de> for OpId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        de_text(d)
    }
}

impl Serialize for NoteId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ser_text(self, s)
    }
}

impl<'de> Deserialize<'de> for NoteId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        de_text(d)
    }
}

/// Opaque payload bytes, serialized as standard base64 with padding.
/// `Debug` shows only the length, so payloads can't reach logs by accident.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD
            .decode(text)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// `POST /sync/v1/push`: one to [`MAX_PUSH_UPDATES`] updates, accepted or
/// rejected together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRequest {
    pub updates: Vec<PushUpdate>,
}

/// One encrypted CRDT update. `note_key` (the note key wrapped by the vault
/// key) is required on a note's first update and optional afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushUpdate {
    pub id: OpId,
    pub note: NoteId,
    pub data: Bytes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_key: Option<Bytes>,
}

/// Each update's sequence number in request order, and the vault's head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    pub seqs: Vec<u64>,
    pub head: u64,
}

/// One page of `GET /sync/v1/changes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesResponse {
    /// Wrapped keys of notes registered after the cursor, and snapshots
    /// replacing updates the client would otherwise miss.
    pub notes: Vec<NoteEntry>,
    /// Updates and purges, in ascending `seq`.
    pub entries: Vec<Entry>,
    /// Pull from here next.
    pub cursor: u64,
    /// Whether another page follows.
    pub more: bool,
}

/// A note's wrapped key, its snapshot, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteEntry {
    pub note: NoteId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
}

/// An encrypted snapshot that replaces the note's updates up to `covers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub covers: u64,
    pub data: Bytes,
}

/// A log entry, from a pull or the live channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    /// `note_key` is set on the live channel when this update registered its
    /// note; pulls carry keys in [`ChangesResponse::notes`] instead.
    Update {
        seq: u64,
        id: OpId,
        note: NoteId,
        data: Bytes,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note_key: Option<Bytes>,
    },
    /// The note was permanently purged at `seq`.
    Purge { seq: u64, note: NoteId },
}

impl Entry {
    pub fn seq(&self) -> u64 {
        match self {
            Entry::Update { seq, .. } | Entry::Purge { seq, .. } => *seq,
        }
    }
}

/// A live-channel message from the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    /// First frame, and again with each refreshed token.
    Auth { token: Token },
    /// The same semantics as `POST /sync/v1/push`.
    Push {
        request_id: String,
        updates: Vec<PushUpdate>,
    },
    /// Any other `type`; ignored.
    #[serde(other)]
    Unknown,
}

/// A live-channel message from the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// Subscribed; every change above `head` will be sent as an entry.
    Ready {
        head: u64,
    },
    Entry(Entry),
    PushResult {
        request_id: String,
        #[serde(flatten)]
        outcome: PushOutcome,
    },
}

/// The result of a push over the live channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PushOutcome {
    Accepted(PushResponse),
    Rejected(ErrorBody),
}

/// The machine-readable reason in a sync error body or rejected push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// `400`: malformed request, empty batch, or invalid cursor.
    BadRequest,
    /// `401`: missing, expired, or revoked access token.
    InvalidToken,
    /// `404`: the note was never registered in this vault.
    UnknownNote,
    /// `409`: an update ID is stored with a different note or bytes.
    OpIdConflict,
    /// `409`: a different wrapped key is stored for the note.
    NoteKeyConflict,
    /// `409`: the stored snapshot covers as much or more.
    SnapshotStale,
    /// `409`: the cursor is above the vault's head; bootstrap again.
    CursorAhead,
    /// `410`: the note was permanently purged.
    NotePurged,
    /// `413`: an update is over the server's maximum update size.
    UpdateTooLarge,
    /// `422`: a note's first update came without its wrapped key.
    MissingNoteKey,
    /// `429`: too many pushes over the live channel; retry later.
    RateLimited,
    /// `507`: the write would put the vault over its storage quota. Clears
    /// only when usage drops or the quota is raised.
    QuotaExceeded,
    /// `500`: a server-side failure; details are only in the server log.
    Internal,
}

/// Every sync error: `{"error": "<code>"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub error: ErrorCode,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn note() -> NoteId {
        "01J9ZQ8B6V1C2D3E4F5G6H7J8K".parse().unwrap()
    }

    fn op() -> OpId {
        "01J9ZQ8B6V1C2D3E4F5G6H7J8M".parse().unwrap()
    }

    fn bytes() -> Bytes {
        Bytes(vec![0xfb, 0xff, 0x00])
    }

    /// Checks that `value` serializes to exactly `expected` and back.
    fn golden<T>(value: &T, expected: Value)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + fmt::Debug,
    {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json, expected);
        let back: T = serde_json::from_value(json).unwrap();
        assert_eq!(&back, value);
    }

    #[test]
    fn push_types() {
        let update = PushUpdate {
            id: op(),
            note: note(),
            data: bytes(),
            note_key: Some(Bytes(b"k".to_vec())),
        };
        golden(
            &PushRequest {
                updates: vec![update.clone()],
            },
            json!({"updates": [{
                "id": "01J9ZQ8B6V1C2D3E4F5G6H7J8M",
                "note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K",
                "data": "+/8A",
                "note_key": "aw==",
            }]}),
        );
        golden(
            &PushUpdate {
                note_key: None,
                ..update
            },
            json!({
                "id": "01J9ZQ8B6V1C2D3E4F5G6H7J8M",
                "note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K",
                "data": "+/8A",
            }),
        );
        golden(
            &PushResponse {
                seqs: vec![1, 2],
                head: 2,
            },
            json!({"seqs": [1, 2], "head": 2}),
        );
    }

    #[test]
    fn changes_types() {
        golden(
            &ChangesResponse {
                notes: vec![
                    NoteEntry {
                        note: note(),
                        key: Some(bytes()),
                        snapshot: Some(Snapshot {
                            covers: 5,
                            data: bytes(),
                        }),
                    },
                    NoteEntry {
                        note: note(),
                        key: None,
                        snapshot: None,
                    },
                ],
                entries: vec![
                    Entry::Update {
                        seq: 7,
                        id: op(),
                        note: note(),
                        data: bytes(),
                        note_key: None,
                    },
                    Entry::Purge {
                        seq: 8,
                        note: note(),
                    },
                ],
                cursor: 8,
                more: false,
            },
            json!({
                "notes": [
                    {
                        "note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K",
                        "key": "+/8A",
                        "snapshot": {"covers": 5, "data": "+/8A"},
                    },
                    {"note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K"},
                ],
                "entries": [
                    {
                        "kind": "update",
                        "seq": 7,
                        "id": "01J9ZQ8B6V1C2D3E4F5G6H7J8M",
                        "note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K",
                        "data": "+/8A",
                    },
                    {"kind": "purge", "seq": 8, "note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K"},
                ],
                "cursor": 8,
                "more": false,
            }),
        );
    }

    #[test]
    fn live_frames() {
        golden(
            &ClientFrame::Auth {
                token: Token("t".into()),
            },
            json!({"type": "auth", "token": "t"}),
        );
        golden(
            &ClientFrame::Push {
                request_id: "r1".into(),
                updates: vec![],
            },
            json!({"type": "push", "request_id": "r1", "updates": []}),
        );
        let unknown: ClientFrame =
            serde_json::from_value(json!({"type": "subscribe", "x": 1})).unwrap();
        assert_eq!(unknown, ClientFrame::Unknown);

        golden(
            &ServerFrame::Ready { head: 3 },
            json!({"type": "ready", "head": 3}),
        );
        golden(
            &ServerFrame::Entry(Entry::Update {
                seq: 4,
                id: op(),
                note: note(),
                data: bytes(),
                note_key: Some(bytes()),
            }),
            json!({
                "type": "entry",
                "kind": "update",
                "seq": 4,
                "id": "01J9ZQ8B6V1C2D3E4F5G6H7J8M",
                "note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K",
                "data": "+/8A",
                "note_key": "+/8A",
            }),
        );
        golden(
            &ServerFrame::Entry(Entry::Purge {
                seq: 5,
                note: note(),
            }),
            json!({"type": "entry", "kind": "purge", "seq": 5, "note": "01J9ZQ8B6V1C2D3E4F5G6H7J8K"}),
        );
        golden(
            &ServerFrame::PushResult {
                request_id: "r1".into(),
                outcome: PushOutcome::Accepted(PushResponse {
                    seqs: vec![6],
                    head: 6,
                }),
            },
            json!({"type": "push_result", "request_id": "r1", "seqs": [6], "head": 6}),
        );
        golden(
            &ServerFrame::PushResult {
                request_id: "r2".into(),
                outcome: PushOutcome::Rejected(ErrorBody {
                    error: ErrorCode::UpdateTooLarge,
                }),
            },
            json!({"type": "push_result", "request_id": "r2", "error": "update_too_large"}),
        );
    }

    #[test]
    fn error_codes() {
        let codes = [
            (ErrorCode::BadRequest, "bad_request"),
            (ErrorCode::InvalidToken, "invalid_token"),
            (ErrorCode::UnknownNote, "unknown_note"),
            (ErrorCode::OpIdConflict, "op_id_conflict"),
            (ErrorCode::NoteKeyConflict, "note_key_conflict"),
            (ErrorCode::SnapshotStale, "snapshot_stale"),
            (ErrorCode::CursorAhead, "cursor_ahead"),
            (ErrorCode::NotePurged, "note_purged"),
            (ErrorCode::UpdateTooLarge, "update_too_large"),
            (ErrorCode::MissingNoteKey, "missing_note_key"),
            (ErrorCode::RateLimited, "rate_limited"),
            (ErrorCode::QuotaExceeded, "quota_exceeded"),
            (ErrorCode::Internal, "internal"),
        ];
        for (code, text) in codes {
            golden(&ErrorBody { error: code }, json!({ "error": text }));
        }
    }

    #[test]
    fn close_codes() {
        assert_eq!(
            [CLOSE_AUTH, CLOSE_RESYNC, CLOSE_TOO_MANY, CLOSE_SHUTDOWN],
            [4001, 4002, 4003, 4004]
        );
    }

    #[test]
    fn malformed_values_fail() {
        for data in ["not base64!", "+/8", "-_8A"] {
            let json = json!({"id": op(), "note": note(), "data": data});
            assert!(
                serde_json::from_value::<PushUpdate>(json).is_err(),
                "{data}"
            );
        }
        let json = json!({"id": "nope", "note": note(), "data": ""});
        assert!(serde_json::from_value::<PushUpdate>(json).is_err());
        let json = json!({"id": op(), "note": "nope", "data": ""});
        assert!(serde_json::from_value::<PushUpdate>(json).is_err());
    }

    #[test]
    fn bytes_debug_hides_contents() {
        assert_eq!(format!("{:?}", Bytes(b"secret".to_vec())), "Bytes(6 bytes)");
    }
}
