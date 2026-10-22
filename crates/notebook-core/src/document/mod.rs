//! Per-note CRDT documents.
//!
//! Each note is one [`NoteDocument`]: a [Yrs](yrs) document that every
//! client edits and merges locally, because the server only ever sees
//! ciphertext. Yrs shares Yjs's update format, so the same bytes can be
//! applied by `yjs` in the editor.
//!
//! # Layout
//!
//! A document has two root types. Their names and the `meta` keys are part
//! of the stored format and must never be renamed; new fields are added as
//! new `meta` keys, and readers ignore keys they don't know.
//!
//! | root   | type   | contents                                            |
//! |--------|--------|-----------------------------------------------------|
//! | `body` | `Text` | the markdown body                                   |
//! | `meta` | `Map`  | `created`, `updated` (Unix ms), `pinned`, `archived`, `trashed` (bool) |
//!
//! `created` is written once, at creation. Concurrent writes to one `meta`
//! key resolve last-writer-wins by Yrs's deterministic rule. Timestamps are
//! written as integers, which lib0 stores as float64 above 32 bits; readers
//! accept any integral number. A value of the wrong type reads as its
//! default (see [`NoteDocument::note`]). Text offsets are UTF-16, matching
//! Yjs and CodeMirror.
//!
//! # Updates and snapshots
//!
//! An **update** carries one local change (or the whole creation) and a
//! **snapshot** carries the full state, including updates still waiting on
//! missing dependencies. Both are applied the same way and may overlap:
//! applying anything twice, or in any order, converges to the same state.
//!
//! The plaintext of both is a format byte followed by the payload:
//!
//! ```text
//! offset  size  field
//! 0       1     format (FORMAT_V1 = 1)
//! 1       n     Yrs/Yjs update, v1 encoding
//! ```
//!
//! A snapshot is a full-state update against an empty state vector, not a
//! Yrs `Snapshot`. The plaintext is then encrypted with the note's key
//! ([`NoteKey::encrypt`](crate::crypto::NoteKey::encrypt)) as
//! [`PayloadKind::CrdtUpdate`](crate::crypto::PayloadKind::CrdtUpdate) or
//! [`PayloadKind::CrdtSnapshot`](crate::crypto::PayloadKind::CrdtSnapshot);
//! the kind in the envelope, not the format byte, is what tells an update
//! from a snapshot. A test vector pins the format-1 bytes.
//!
//! # Client IDs
//!
//! Every [`NoteDocument`] draws a fresh random 32-bit client ID from the
//! platform's secure random source. Callers can't choose one: two diverged
//! replicas sharing an ID corrupt each other. Yrs is built with 32-bit
//! client IDs (`small-client`), so a decoded update can't carry one Yrs
//! would reject with a panic; peers must use 32-bit IDs too, as Yjs 13 does.
//! If an applied update shows another replica writing under this instance's
//! ID, the document moves itself to a fresh one.
//!
//! # Ciphertext only
//!
//! A document owns its note key. Every mutator returns an
//! [`EncryptedUpdate`], [`NoteDocument::snapshot`] returns an
//! [`EncryptedSnapshot`], and remote data is accepted only as ciphertext, so
//! nothing here hands plaintext to storage or sync.
//!
//! The one plaintext path is the editor exchange, with the editor replica
//! open on this device: [`NoteDocument::editor_attach`],
//! [`NoteDocument::apply_editor_update`], and
//! [`NoteDocument::editor_pending`]. Its updates are [`EditorUpdate`]s, a
//! separate type with no format byte and no conversion to or from the
//! encrypted types, and every editor update accepted comes back out as an
//! [`EncryptedUpdate`]. Editor updates get the same structural check as
//! decrypted remote data.
//!
//! # Untrusted input
//!
//! An update that decrypts came from a key holder, but it is still
//! untrusted: Yrs 0.28 trusts the structure of what it decodes. So every
//! decrypted payload is checked first, with a strict reader of our own. It
//! must be shaped the way a note document writes one:
//!
//! - at most [`MAX_PLAINTEXT_LEN`] bytes;
//! - counts that fit the bytes present;
//! - clocks within `i32`;
//! - each client listed once, with sorted, disjoint deleted ranges;
//! - no empty blocks;
//! - items parented only by the `body` or `meta` root;
//! - content limited to strings, plain values, and deleted ranges;
//! - nothing after the delete set.
//!
//! Anything else, along with input that fails decryption or has an unknown
//! format, is rejected before the document is touched, so it stays
//! unchanged.
//!
//! Yrs 0.28 still has integration bugs that a crafted update from a key
//! holder could reach and that a stateless check can't rule out. That risk is
//! accepted. Held updates are also retried after every apply, working around
//! a Yrs bug that would otherwise leave them stuck on ordinary out-of-order
//! delivery.

mod doc;
mod format;
mod validate;

pub use doc::{EditorSync, NoteDocument};
pub use format::{DocumentError, EditorUpdate, EncryptedSnapshot, EncryptedUpdate, FORMAT_V1};
pub use validate::MAX_PLAINTEXT_LEN;
