-- The sync log: per-vault sequence head, note-key registry with purge
-- tombstones, encrypted updates, and the latest snapshot per note.
-- Payload columns are named `payload` in both `ops` and `snapshots`, which
-- storage accounting sums over.

-- The last sequence number assigned in the vault. Stored, not computed as
-- MAX(seq), because compaction and purge delete ops.
ALTER TABLE vaults ADD COLUMN sync_head INTEGER NOT NULL DEFAULT 0;

CREATE TABLE sync_notes (
    vault_id BLOB NOT NULL REFERENCES vaults (vault_id) ON DELETE CASCADE,
    note_id BLOB NOT NULL,
    -- The note key wrapped by the vault key; NULL once purged.
    wrapped_key BLOB,
    -- Sequence number of the update that registered the note.
    created_seq INTEGER NOT NULL,
    -- Sequence number of the purge (the tombstone); NULL unless purged.
    purged_seq INTEGER,
    PRIMARY KEY (vault_id, note_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE ops (
    vault_id BLOB NOT NULL REFERENCES vaults (vault_id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    op_id BLOB NOT NULL,
    note_id BLOB NOT NULL,
    payload BLOB NOT NULL,
    PRIMARY KEY (vault_id, seq)
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX ops_op_id ON ops (vault_id, op_id);
CREATE INDEX ops_note ON ops (vault_id, note_id, seq);

CREATE TABLE snapshots (
    vault_id BLOB NOT NULL,
    note_id BLOB NOT NULL,
    covers INTEGER NOT NULL,
    payload BLOB NOT NULL,
    PRIMARY KEY (vault_id, note_id),
    FOREIGN KEY (vault_id, note_id) REFERENCES sync_notes (vault_id, note_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX snapshots_covers ON snapshots (vault_id, covers);
CREATE INDEX sync_notes_created ON sync_notes (vault_id, created_seq);
CREATE INDEX sync_notes_purged ON sync_notes (vault_id, purged_seq) WHERE purged_seq IS NOT NULL;
