//! Encrypted whole-vault backup files.
//!
//! A backup is one self-contained file that rebuilds a notebook on another
//! computer with only the recovery key. The full format, with restore steps,
//! is in `docs/backup-format.md`.
//!
//! # Layout
//!
//! ```text
//! offset  size  field
//! 0       8     magic "NBBACKUP"
//! 8       1     format version (1)
//! 9       16    vault id
//! 25      2     recovery entry length r (u16 BE)
//! 27      r     recovery entry (RecoveryEntry::to_bytes)
//! 27+r    …     payload envelope (rest of file)
//! ```
//!
//! The first `27 + r` bytes are the cleartext **header**. The payload is one
//! envelope sealed under the vault key, bound to the vault ID, with its own
//! kind (48). Its plaintext is:
//!
//! ```text
//! 32       SHA-256 of the header bytes
//! 8        created_at (Unix ms, i64 BE)
//! 4        note count (u32 BE)
//! repeat:  note id (16) | wrapped key len (u16 BE) | wrapped key
//!          | snapshot len (u32 BE) | encrypted snapshot
//! ```
//!
//! # What is visible
//!
//! Only the format version, the vault ID, and the recovery entry (which opens
//! only with the recovery key) are in the clear. Note IDs, the note count, and
//! per-note sizes are inside the payload. The header hash inside the payload
//! makes the header tamper-evident too: changing any byte of the file fails
//! [`open`]. Each snapshot is still encrypted under its own note key, so
//! restore never handles plaintext outside a
//! [`NoteDocument`].
//!
//! # Opening
//!
//! [`read_header`] needs only the header, so a folder of backups can be
//! sorted by vault without reading whole files. The vault key comes either
//! from the device (a backup of the open vault) or from
//! [`BackupHeader::vault_key`] with the recovery key. [`open`] then checks and
//! parses the payload, and [`OpenedBackup::verify`] loads every note before
//! anything is changed.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub mod folder;
mod name;

use std::collections::BTreeSet;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::crypto::{CryptoError, RecoveryEntry, RecoveryKey, VaultId, VaultKey};
use crate::document::{DocumentError, EncryptedSnapshot, NoteDocument};
use crate::note::{NoteId, Timestamp};

pub use name::{BACKUP_EXTENSION, BackupFileName, file_name};

/// The first eight bytes of every backup file.
pub const MAGIC: &[u8; 8] = b"NBBACKUP";
/// The format version this build writes and reads.
pub const FORMAT_VERSION: u8 = 1;
/// Header bytes before the recovery entry.
const FIXED_HEADER_LEN: usize = 8 + 1 + 16 + 2;
/// The longest possible header. Reading this many bytes of a file (or the
/// whole file, if shorter) is enough for [`read_header`].
pub const MAX_HEADER_LEN: usize = FIXED_HEADER_LEN + u16::MAX as usize;

const HASH_LEN: usize = 32;
/// The smallest encoded note: an ID and two empty length fields.
const MIN_NOTE_LEN: usize = 16 + 2 + 4;

/// Why a backup can't be built or opened. Never carries file contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupError {
    /// The file doesn't start with [`MAGIC`].
    NotABackup,
    /// The file uses a format version newer than this build knows.
    TooNew(u8),
    /// The file is truncated, altered, or otherwise not a valid backup.
    Damaged,
    /// The recovery key doesn't open this backup.
    WrongKey,
    /// A note's wrapped key or snapshot, or the note count, doesn't fit the
    /// format's length fields.
    TooLarge,
    /// Encryption failed while building, such as the random source failing.
    Crypto(CryptoError),
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotABackup => f.write_str("not a Notebook backup"),
            Self::TooNew(v) => write!(f, "backup format {v} needs a newer version of Notebook"),
            Self::Damaged => f.write_str("backup is damaged"),
            Self::WrongKey => f.write_str("the recovery key does not open this backup"),
            Self::TooLarge => f.write_str("note is too large for the backup format"),
            Self::Crypto(err) => write!(f, "backup encryption failed: {err}"),
        }
    }
}

impl std::error::Error for BackupError {}

/// One note as stored in a backup: what
/// [`Storage::import_note`](crate::storage::Storage::import_note) takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupNote {
    pub id: NoteId,
    pub wrapped_key: Vec<u8>,
    pub snapshot: EncryptedSnapshot,
}

/// The cleartext start of a backup file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupHeader {
    pub version: u8,
    pub vault: VaultId,
    pub recovery: RecoveryEntry,
}

impl BackupHeader {
    /// Recovers the backup's vault key with the recovery key. A key from
    /// another notebook is [`BackupError::WrongKey`].
    pub fn vault_key(&self, recovery: &RecoveryKey) -> Result<VaultKey, BackupError> {
        self.recovery.open(recovery).map_err(|err| match err {
            CryptoError::Decrypt => BackupError::WrongKey,
            _ => BackupError::Damaged,
        })
    }
}

/// A backup whose payload opened and parsed. Its notes are still ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedBackup {
    pub vault: VaultId,
    pub created_at: Timestamp,
    /// In the order they were written; IDs are unique.
    pub notes: Vec<BackupNote>,
}

impl OpenedBackup {
    /// Unwraps every note key and loads every snapshot into a throwaway
    /// [`NoteDocument`], checking each yields a note. Restore calls this
    /// before changing anything.
    pub fn verify(&self, vault_key: &VaultKey) -> Result<(), BackupError> {
        for note in &self.notes {
            let key = vault_key
                .unwrap_note_key(self.vault, note.id, &note.wrapped_key)
                .map_err(|_| BackupError::Damaged)?;
            let mut doc =
                NoteDocument::open(self.vault, note.id, key).map_err(|err| match err {
                    DocumentError::Crypto(err) => BackupError::Crypto(err),
                    _ => BackupError::Damaged,
                })?;
            doc.load_snapshot(note.snapshot.as_bytes())
                .map_err(|_| BackupError::Damaged)?;
            if doc.note().is_none() {
                return Err(BackupError::Damaged);
            }
        }
        Ok(())
    }
}

/// Builds a backup file of `notes` for the recovery entry's vault, sealed
/// under `vault_key`, which must be the key that entry wraps.
pub fn build(
    recovery: &RecoveryEntry,
    vault_key: &VaultKey,
    created_at: Timestamp,
    notes: &[BackupNote],
) -> Result<Vec<u8>, BackupError> {
    let vault = recovery.vault_id();
    let entry = recovery.to_bytes();
    let entry_len = u16::try_from(entry.len()).map_err(|_| BackupError::TooLarge)?;
    let mut out = Vec::with_capacity(FIXED_HEADER_LEN + entry.len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&vault.to_bytes());
    out.extend_from_slice(&entry_len.to_be_bytes());
    out.extend_from_slice(&entry);

    let count = u32::try_from(notes.len()).map_err(|_| BackupError::TooLarge)?;
    let mut plaintext = Vec::new();
    plaintext.extend_from_slice(&Sha256::digest(&out));
    plaintext.extend_from_slice(&created_at.as_millis().to_be_bytes());
    plaintext.extend_from_slice(&count.to_be_bytes());
    for note in notes {
        let key_len = u16::try_from(note.wrapped_key.len()).map_err(|_| BackupError::TooLarge)?;
        let snapshot = note.snapshot.as_bytes();
        let snapshot_len = u32::try_from(snapshot.len()).map_err(|_| BackupError::TooLarge)?;
        plaintext.extend_from_slice(&note.id.to_bytes());
        plaintext.extend_from_slice(&key_len.to_be_bytes());
        plaintext.extend_from_slice(&note.wrapped_key);
        plaintext.extend_from_slice(&snapshot_len.to_be_bytes());
        plaintext.extend_from_slice(snapshot);
    }

    let payload = vault_key
        .seal_backup(vault, &plaintext)
        .map_err(BackupError::Crypto)?;
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Parses the cleartext header. `bytes` may be the whole file or any prefix
/// holding the header (at most [`MAX_HEADER_LEN`] bytes).
pub fn read_header(bytes: &[u8]) -> Result<BackupHeader, BackupError> {
    parse_header(bytes).map(|(header, _)| header)
}

/// Opens and parses a backup with its vault key. Any change to the file, a
/// truncation, or the wrong key is [`BackupError::Damaged`]; nothing partial
/// is returned.
pub fn open(bytes: &[u8], vault_key: &VaultKey) -> Result<OpenedBackup, BackupError> {
    let (header, header_len) = parse_header(bytes)?;
    let plaintext = vault_key
        .open_backup(header.vault, &bytes[header_len..])
        .map_err(|_| BackupError::Damaged)?;
    let mut r = Reader(&plaintext);
    let hash = r.take(HASH_LEN)?;
    if hash != Sha256::digest(&bytes[..header_len]).as_slice() {
        return Err(BackupError::Damaged);
    }
    let created_at = Timestamp(i64::from_be_bytes(r.array()?));
    let count = u32::from_be_bytes(r.array()?) as usize;
    // Bound the allocation by what the bytes can actually hold.
    let mut notes = Vec::with_capacity(count.min(r.0.len() / MIN_NOTE_LEN));
    let mut ids = BTreeSet::new();
    for _ in 0..count {
        let id = NoteId::from_bytes(r.array()?);
        if !ids.insert(id) {
            return Err(BackupError::Damaged);
        }
        let key_len = u16::from_be_bytes(r.array()?) as usize;
        let wrapped_key = r.take(key_len)?.to_vec();
        let snapshot_len = u32::from_be_bytes(r.array()?) as usize;
        let snapshot = EncryptedSnapshot::from_bytes(r.take(snapshot_len)?.to_vec());
        notes.push(BackupNote {
            id,
            wrapped_key,
            snapshot,
        });
    }
    if !r.0.is_empty() {
        return Err(BackupError::Damaged);
    }
    Ok(OpenedBackup {
        vault: header.vault,
        created_at,
        notes,
    })
}

/// The header and its length in bytes.
fn parse_header(bytes: &[u8]) -> Result<(BackupHeader, usize), BackupError> {
    if bytes.len() < MAGIC.len() {
        // A cut-off backup is damaged; anything else isn't ours.
        return Err(if !bytes.is_empty() && MAGIC.starts_with(bytes) {
            BackupError::Damaged
        } else {
            BackupError::NotABackup
        });
    }
    let mut r = Reader(bytes);
    if r.take(MAGIC.len())? != MAGIC {
        return Err(BackupError::NotABackup);
    }
    let [version] = r.array()?;
    if version > FORMAT_VERSION {
        return Err(BackupError::TooNew(version));
    }
    if version != FORMAT_VERSION {
        return Err(BackupError::Damaged);
    }
    let vault = VaultId::from_bytes(r.array()?);
    let entry_len = u16::from_be_bytes(r.array()?) as usize;
    let recovery =
        RecoveryEntry::from_bytes(r.take(entry_len)?).map_err(|_| BackupError::Damaged)?;
    if recovery.vault_id() != vault {
        return Err(BackupError::Damaged);
    }
    Ok((
        BackupHeader {
            version,
            vault,
            recovery,
        },
        FIXED_HEADER_LEN + entry_len,
    ))
}

/// Reads from the front of a byte slice; running out is
/// [`BackupError::Damaged`].
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], BackupError> {
        if self.0.len() < n {
            return Err(BackupError::Damaged);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], BackupError> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }
}

#[cfg(test)]
mod tests;
