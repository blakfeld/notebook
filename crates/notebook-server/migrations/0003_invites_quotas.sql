-- Invite codes that gate registration, and per-vault storage quotas.

-- Single-use invite codes, created by the operator CLI. Codes are stored
-- only as SHA-256 hashes of their normalized form. Status is derived
-- (revoked, then used, then expired by `expires_at`), never stored. Not vault
-- data, so the `vault_id` convention does not apply.
CREATE TABLE invites (
    -- 16-byte ULID.
    id BLOB PRIMARY KEY NOT NULL,
    code_hash BLOB NOT NULL UNIQUE,
    -- Free text from the operator, such as who the code is for.
    note TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    used_at INTEGER,
    -- The user created with it. Deferred, because registration consumes the
    -- invite before inserting that user, in the same transaction.
    used_by BLOB REFERENCES users (user_id) DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- `quota_bytes` is set explicitly from `default_vault_quota_bytes` for new
-- vaults; this constant default (100 MiB) covers existing ones. `used_bytes`
-- is the total payload length of the vault's `ops` and `snapshots`, kept
-- current by every sync write.
ALTER TABLE vaults ADD COLUMN quota_bytes INTEGER NOT NULL DEFAULT 104857600;
ALTER TABLE vaults ADD COLUMN used_bytes INTEGER NOT NULL DEFAULT 0;

UPDATE vaults SET used_bytes =
    (SELECT coalesce(sum(length(payload)), 0) FROM ops WHERE ops.vault_id = vaults.vault_id)
  + (SELECT coalesce(sum(length(payload)), 0) FROM snapshots
     WHERE snapshots.vault_id = vaults.vault_id);
