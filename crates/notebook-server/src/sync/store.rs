//! The sync log in SQLite: push, snapshot upload, purge, and paginated pull.
//!
//! Each write is one `BEGIN IMMEDIATE` transaction that reads `sync_head`,
//! validates, writes, advances the head, and commits, so sequence assignment
//! and compaction never disagree. Each computes its net payload-byte delta
//! and passes it to [`quota::reserve`] just before commit, which keeps the
//! vault's usage exact and rejects growth past its quota. HTTP and
//! live-channel pushes share [`push`].
//!
//! Payloads are stored and returned exactly as received and never logged.

use std::collections::{BTreeMap, HashSet};

use notebook_core::note::NoteId;
use notebook_core::protocol::{
    Bytes, ChangesResponse, Entry, MAX_NOTE_KEY_BYTES, MAX_PUSH_UPDATES, NoteEntry, OpId,
    PushUpdate, Snapshot,
};
use sqlx::{SqliteConnection, SqlitePool};

use super::error::SyncError;
use crate::quota;

/// The payload budget of one pull page.
pub const PAGE_BUDGET: usize = 2 * 1024 * 1024;

/// Counted against the page budget for every item besides its payload, so
/// pages of tiny or empty items stay bounded too.
const ITEM_OVERHEAD: usize = 64;

/// Sequence numbers scanned per query while sizing a page. Each query reads
/// only this range, so its cost doesn't grow with the vault.
const SIZING_WINDOW: i64 = 1024;

/// A committed push.
#[derive(Debug)]
pub struct Pushed {
    /// Each update's sequence number, in request order.
    pub seqs: Vec<u64>,
    pub head: u64,
    /// The newly stored updates, for the live channel. Retried updates that
    /// were already stored are not repeated.
    pub entries: Vec<Entry>,
    /// Net change in stored payload bytes.
    pub delta: i64,
}

/// A committed purge.
#[derive(Debug)]
pub struct Purged {
    /// The tombstone, or `None` if the note was already purged.
    pub entry: Option<Entry>,
    /// Net change in stored payload bytes (never positive).
    pub delta: i64,
}

/// Stored sequence numbers are never negative.
fn seq_out(seq: i64) -> u64 {
    u64::try_from(seq).unwrap_or(0)
}

fn seq_in(seq: u64) -> Result<i64, SyncError> {
    i64::try_from(seq).map_err(|_| SyncError::BAD_REQUEST)
}

fn len(bytes: &[u8]) -> i64 {
    i64::try_from(bytes.len()).unwrap_or(i64::MAX)
}

fn note_id(bytes: Vec<u8>) -> Result<NoteId, SyncError> {
    <[u8; 16]>::try_from(bytes)
        .map(NoteId::from_bytes)
        .map_err(|_| {
            tracing::error!("malformed note ID in the sync tables");
            SyncError::INTERNAL
        })
}

fn op_id(bytes: Vec<u8>) -> Result<OpId, SyncError> {
    <[u8; 16]>::try_from(bytes)
        .map(OpId::from_bytes)
        .map_err(|_| {
            tracing::error!("malformed op ID in the sync tables");
            SyncError::INTERNAL
        })
}

async fn read_head(conn: &mut SqliteConnection, vault: &[u8]) -> Result<u64, SyncError> {
    let head: Option<i64> = sqlx::query_scalar("SELECT sync_head FROM vaults WHERE vault_id = ?")
        .bind(vault)
        .fetch_optional(conn)
        .await?;
    head.map(seq_out).ok_or_else(|| {
        tracing::error!("sync request for a vault that does not exist");
        SyncError::INTERNAL
    })
}

async fn write_head(conn: &mut SqliteConnection, vault: &[u8], head: u64) -> Result<(), SyncError> {
    sqlx::query("UPDATE vaults SET sync_head = ? WHERE vault_id = ?")
        .bind(seq_in(head)?)
        .bind(vault)
        .execute(conn)
        .await?;
    Ok(())
}

/// Applies a write's net payload-byte `delta` to the vault's usage, or fails
/// with `quota_exceeded`, which rolls the write back.
async fn reserve(conn: &mut SqliteConnection, vault: &[u8], delta: i64) -> Result<(), SyncError> {
    if quota::reserve(conn, vault, delta).await? {
        Ok(())
    } else {
        Err(SyncError::QUOTA_EXCEEDED)
    }
}

/// The vault's head sequence number.
pub async fn head(pool: &SqlitePool, vault: &[u8]) -> Result<u64, SyncError> {
    read_head(&mut *pool.acquire().await?, vault).await
}

/// A registry row.
struct NoteRow {
    wrapped_key: Option<Vec<u8>>,
    created_seq: i64,
    purged: bool,
}

async fn note_row(
    conn: &mut SqliteConnection,
    vault: &[u8],
    note: &[u8],
) -> Result<Option<NoteRow>, SyncError> {
    let row: Option<(Option<Vec<u8>>, i64, Option<i64>)> = sqlx::query_as(
        "SELECT wrapped_key, created_seq, purged_seq FROM sync_notes \
         WHERE vault_id = ? AND note_id = ?",
    )
    .bind(vault)
    .bind(note)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(wrapped_key, created_seq, purged_seq)| NoteRow {
        wrapped_key,
        created_seq,
        purged: purged_seq.is_some(),
    }))
}

/// Stores a batch of updates, all or nothing. A retried update ID returns
/// its original sequence number; the same ID with a different note or bytes
/// is `op_id_conflict`.
pub async fn push(
    pool: &SqlitePool,
    vault: &[u8],
    updates: &[PushUpdate],
    max_update_bytes: usize,
) -> Result<Pushed, SyncError> {
    if updates.is_empty() || updates.len() > MAX_PUSH_UPDATES {
        return Err(SyncError::BAD_REQUEST);
    }
    if updates
        .iter()
        .filter_map(|u| u.note_key.as_ref())
        .any(|k| k.0.len() > MAX_NOTE_KEY_BYTES)
    {
        return Err(SyncError::BAD_REQUEST);
    }
    if updates.iter().any(|u| u.data.0.len() > max_update_bytes) {
        return Err(SyncError::UPDATE_TOO_LARGE);
    }
    let mut ids = HashSet::new();
    if !updates.iter().all(|u| ids.insert(u.id)) {
        return Err(SyncError::BAD_REQUEST);
    }

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let start = read_head(&mut tx, vault).await?;
    let mut head = start;
    let mut seqs = Vec::with_capacity(updates.len());
    let mut entries = Vec::new();
    let mut delta = 0i64;
    for update in updates {
        let note = update.note.to_bytes();
        let registers = match note_row(&mut tx, vault, &note).await? {
            Some(row) if row.purged => return Err(SyncError::NOTE_PURGED),
            Some(row) => {
                if let Some(key) = &update.note_key
                    && row.wrapped_key.as_deref() != Some(key.0.as_slice())
                {
                    return Err(SyncError::NOTE_KEY_CONFLICT);
                }
                false
            }
            None if update.note_key.is_none() => return Err(SyncError::MISSING_NOTE_KEY),
            None => true,
        };

        let op = update.id.to_bytes();
        let stored: Option<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT seq, note_id, payload FROM ops WHERE vault_id = ? AND op_id = ?",
        )
        .bind(vault)
        .bind(op.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((seq, stored_note, payload)) = stored {
            if stored_note != note || payload != update.data.0 {
                return Err(SyncError::OP_ID_CONFLICT);
            }
            seqs.push(seq_out(seq));
            continue;
        }

        head += 1;
        let seq = seq_in(head)?;
        if registers {
            sqlx::query(
                "INSERT INTO sync_notes (vault_id, note_id, wrapped_key, created_seq) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(vault)
            .bind(note.as_slice())
            .bind(update.note_key.as_ref().map(|k| k.0.as_slice()))
            .bind(seq)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO ops (vault_id, seq, op_id, note_id, payload) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(vault)
        .bind(seq)
        .bind(op.as_slice())
        .bind(note.as_slice())
        .bind(update.data.0.as_slice())
        .execute(&mut *tx)
        .await?;
        delta += len(&update.data.0);
        seqs.push(head);
        entries.push(Entry::Update {
            seq: head,
            id: update.id,
            note: update.note,
            data: update.data.clone(),
            note_key: if registers {
                update.note_key.clone()
            } else {
                None
            },
        });
    }
    if head != start {
        write_head(&mut tx, vault, head).await?;
    }
    reserve(&mut tx, vault, delta).await?;
    tx.commit().await?;
    Ok(Pushed {
        seqs,
        head,
        entries,
        delta,
    })
}

/// Stores `data` as `note`'s only snapshot, covering sequence `covers`, and
/// deletes the note's updates at or below it. Returns the net byte delta.
pub async fn snapshot(
    pool: &SqlitePool,
    vault: &[u8],
    note: NoteId,
    covers: u64,
    data: &[u8],
) -> Result<i64, SyncError> {
    let note = note.to_bytes();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let head = read_head(&mut tx, vault).await?;
    let row = note_row(&mut tx, vault, &note)
        .await?
        .ok_or(SyncError::UNKNOWN_NOTE)?;
    if row.purged {
        return Err(SyncError::NOTE_PURGED);
    }
    // A snapshot can't cover past the head, or before the note existed (its
    // key must reach clients no later than the snapshot does).
    if covers > head || seq_in(covers)? < row.created_seq {
        return Err(SyncError::BAD_REQUEST);
    }
    let covers = seq_in(covers)?;
    let replaced: Option<(i64, i64)> = sqlx::query_as(
        "SELECT covers, length(payload) FROM snapshots WHERE vault_id = ? AND note_id = ?",
    )
    .bind(vault)
    .bind(note.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    if replaced.is_some_and(|(stored, _)| stored >= covers) {
        return Err(SyncError::SNAPSHOT_STALE);
    }
    let compacted: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(length(payload)), 0) FROM ops \
         WHERE vault_id = ? AND note_id = ? AND seq <= ?",
    )
    .bind(vault)
    .bind(note.as_slice())
    .bind(covers)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO snapshots (vault_id, note_id, covers, payload) VALUES (?, ?, ?, ?) \
         ON CONFLICT (vault_id, note_id) DO UPDATE SET \
         covers = excluded.covers, payload = excluded.payload",
    )
    .bind(vault)
    .bind(note.as_slice())
    .bind(covers)
    .bind(data)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM ops WHERE vault_id = ? AND note_id = ? AND seq <= ?")
        .bind(vault)
        .bind(note.as_slice())
        .bind(covers)
        .execute(&mut *tx)
        .await?;
    let delta = len(data) - replaced.map_or(0, |(_, bytes)| bytes) - compacted;
    // Usually negative, which is always allowed.
    reserve(&mut tx, vault, delta).await?;
    tx.commit().await?;
    Ok(delta)
}

/// Permanently purges `note`: deletes its updates, snapshot, and wrapped
/// key, and records a tombstone at a new sequence number. Purging again
/// changes nothing.
pub async fn purge(pool: &SqlitePool, vault: &[u8], note: NoteId) -> Result<Purged, SyncError> {
    let bytes = note.to_bytes();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let head = read_head(&mut tx, vault).await?;
    let row = note_row(&mut tx, vault, &bytes)
        .await?
        .ok_or(SyncError::UNKNOWN_NOTE)?;
    if row.purged {
        return Ok(Purged {
            entry: None,
            delta: 0,
        });
    }
    let freed: i64 = sqlx::query_scalar(
        "SELECT (SELECT coalesce(sum(length(payload)), 0) FROM ops \
                 WHERE vault_id = ?1 AND note_id = ?2) \
              + (SELECT coalesce(sum(length(payload)), 0) FROM snapshots \
                 WHERE vault_id = ?1 AND note_id = ?2)",
    )
    .bind(vault)
    .bind(bytes.as_slice())
    .fetch_one(&mut *tx)
    .await?;
    for sql in [
        "DELETE FROM ops WHERE vault_id = ? AND note_id = ?",
        "DELETE FROM snapshots WHERE vault_id = ? AND note_id = ?",
    ] {
        sqlx::query(sql)
            .bind(vault)
            .bind(bytes.as_slice())
            .execute(&mut *tx)
            .await?;
    }
    let seq = head + 1;
    sqlx::query(
        "UPDATE sync_notes SET wrapped_key = NULL, purged_seq = ? \
         WHERE vault_id = ? AND note_id = ?",
    )
    .bind(seq_in(seq)?)
    .bind(vault)
    .bind(bytes.as_slice())
    .execute(&mut *tx)
    .await?;
    write_head(&mut tx, vault, seq).await?;
    let delta = -freed;
    reserve(&mut tx, vault, delta).await?;
    tx.commit().await?;
    Ok(Purged {
        entry: Some(Entry::Purge { seq, note }),
        delta,
    })
}

/// One page of changes after `after`, bounded by [`PAGE_BUDGET`].
pub async fn changes(
    pool: &SqlitePool,
    vault: &[u8],
    after: u64,
) -> Result<ChangesResponse, SyncError> {
    changes_with_budget(pool, vault, after, PAGE_BUDGET).await
}

/// Every change in a vault is keyed by a sequence number: an update by its
/// `seq`, a snapshot by its `covers`, a wrapped key by the registering
/// update's seq, a tombstone by the purge's seq. A page takes whole key
/// groups in key order while they fit `budget`; the first group is taken
/// even if it alone is over budget. The whole page is read in one
/// transaction, so it is a consistent view.
pub(crate) async fn changes_with_budget(
    pool: &SqlitePool,
    vault: &[u8],
    after: u64,
    budget: usize,
) -> Result<ChangesResponse, SyncError> {
    let mut tx = pool.begin().await?;
    let head = read_head(&mut tx, vault).await?;
    if after > head {
        return Err(SyncError::CURSOR_AHEAD);
    }
    let after = seq_in(after)?;

    // Size the page from lengths alone, a window of sequence numbers at a
    // time.
    let mut last: Option<i64> = None;
    let mut used = 0usize;
    let mut more = false;
    // Everything at or below `scan` has been sized.
    let mut scan = after;
    'sizing: loop {
        // Skip empty stretches of sequence numbers (compacted or purged).
        let next: Option<i64> = sqlx::query_scalar(
            "SELECT min(k) FROM ( \
               SELECT min(seq) AS k FROM ops WHERE vault_id = ?1 AND seq > ?2 \
               UNION ALL SELECT min(covers) FROM snapshots \
                 WHERE vault_id = ?1 AND covers > ?2 \
               UNION ALL SELECT min(created_seq) FROM sync_notes \
                 WHERE vault_id = ?1 AND created_seq > ?2 AND wrapped_key IS NOT NULL \
               UNION ALL SELECT min(purged_seq) FROM sync_notes \
                 WHERE vault_id = ?1 AND purged_seq > ?2 \
             )",
        )
        .bind(vault)
        .bind(scan)
        .fetch_one(&mut *tx)
        .await?;
        let Some(from) = next else { break };
        let to = from.saturating_add(SIZING_WINDOW - 1);
        let groups: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT k, count(*), sum(n) FROM ( \
               SELECT seq AS k, length(payload) AS n FROM ops \
                 WHERE vault_id = ?1 AND seq BETWEEN ?2 AND ?3 \
               UNION ALL SELECT covers, length(payload) FROM snapshots \
                 WHERE vault_id = ?1 AND covers BETWEEN ?2 AND ?3 \
               UNION ALL SELECT created_seq, length(wrapped_key) FROM sync_notes \
                 WHERE vault_id = ?1 AND created_seq BETWEEN ?2 AND ?3 \
                 AND wrapped_key IS NOT NULL \
               UNION ALL SELECT purged_seq, 0 FROM sync_notes \
                 WHERE vault_id = ?1 AND purged_seq BETWEEN ?2 AND ?3 \
             ) GROUP BY k ORDER BY k",
        )
        .bind(vault)
        .bind(from)
        .bind(to)
        .fetch_all(&mut *tx)
        .await?;
        for (key, count, bytes) in groups {
            let size = usize::try_from(count)
                .unwrap_or(usize::MAX)
                .saturating_mul(ITEM_OVERHEAD)
                .saturating_add(usize::try_from(bytes).unwrap_or(usize::MAX));
            if last.is_some() && used.saturating_add(size) > budget {
                more = true;
                break 'sizing;
            }
            used = used.saturating_add(size);
            last = Some(key);
        }
        scan = to;
    }
    let Some(end) = last else {
        return Ok(ChangesResponse {
            notes: Vec::new(),
            entries: Vec::new(),
            cursor: head,
            more: false,
        });
    };

    let mut entries = Vec::new();
    let ops: Vec<OpRow> = sqlx::query_as(
        "SELECT seq, op_id, note_id, payload FROM ops \
         WHERE vault_id = ? AND seq > ? AND seq <= ?",
    )
    .bind(vault)
    .bind(after)
    .bind(end)
    .fetch_all(&mut *tx)
    .await?;
    for (seq, op, note, payload) in ops {
        entries.push(Entry::Update {
            seq: seq_out(seq),
            id: op_id(op)?,
            note: note_id(note)?,
            data: Bytes(payload),
            note_key: None,
        });
    }
    let purges: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT purged_seq, note_id FROM sync_notes \
         WHERE vault_id = ? AND purged_seq > ? AND purged_seq <= ?",
    )
    .bind(vault)
    .bind(after)
    .bind(end)
    .fetch_all(&mut *tx)
    .await?;
    for (seq, note) in purges {
        entries.push(Entry::Purge {
            seq: seq_out(seq),
            note: note_id(note)?,
        });
    }
    entries.sort_by_key(Entry::seq);

    // Notes in order of their first key on the page.
    let mut notes: BTreeMap<(i64, NoteId), NoteEntry> = BTreeMap::new();
    let mut first_key: BTreeMap<NoteId, i64> = BTreeMap::new();
    let keys: Vec<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT created_seq, note_id, wrapped_key FROM sync_notes \
         WHERE vault_id = ? AND created_seq > ? AND created_seq <= ? \
         AND wrapped_key IS NOT NULL",
    )
    .bind(vault)
    .bind(after)
    .bind(end)
    .fetch_all(&mut *tx)
    .await?;
    let snapshots: Vec<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT covers, note_id, payload FROM snapshots \
         WHERE vault_id = ? AND covers > ? AND covers <= ?",
    )
    .bind(vault)
    .bind(after)
    .bind(end)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut by_note: BTreeMap<NoteId, NoteEntry> = BTreeMap::new();
    for (seq, note, key) in keys {
        let note = note_id(note)?;
        first_key.insert(note, seq);
        by_note.entry(note).or_insert_with(|| empty_note(note)).key = Some(Bytes(key));
    }
    for (covers, note, payload) in snapshots {
        let note = note_id(note)?;
        first_key.entry(note).or_insert(covers);
        by_note
            .entry(note)
            .or_insert_with(|| empty_note(note))
            .snapshot = Some(Snapshot {
            covers: seq_out(covers),
            data: Bytes(payload),
        });
    }
    for (note, entry) in by_note {
        notes.insert((first_key[&note], note), entry);
    }

    Ok(ChangesResponse {
        notes: notes.into_values().collect(),
        entries,
        cursor: if more { seq_out(end) } else { head },
        more,
    })
}

/// `seq, op_id, note_id, payload` of a stored update.
type OpRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>);

fn empty_note(note: NoteId) -> NoteEntry {
    NoteEntry {
        note,
        key: None,
        snapshot: None,
    }
}

#[cfg(test)]
mod tests;
