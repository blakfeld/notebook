-- Accounts, vaults, key bundles, device sessions, and server metadata.
-- IDs are 16-byte ULID blobs; timestamps are Unix seconds.

CREATE TABLE users (
    user_id BLOB PRIMARY KEY NOT NULL,
    -- Normalized (lowercase) username; also the OPAQUE credential identifier.
    username TEXT NOT NULL UNIQUE,
    opaque_record BLOB NOT NULL,
    -- Which OPAQUE server key the record belongs to (always 1 until rotation).
    server_key_id INTEGER NOT NULL DEFAULT 1,
    -- SHA-256 of the client's recovery verifier.
    recovery_verifier_hash BLOB NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- One vault per user; the vault ID is chosen by the client and bound into its
-- key bundle.
CREATE TABLE vaults (
    vault_id BLOB PRIMARY KEY NOT NULL,
    user_id BLOB NOT NULL UNIQUE REFERENCES users (user_id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE key_bundles (
    vault_id BLOB PRIMARY KEY NOT NULL REFERENCES vaults (vault_id) ON DELETE CASCADE,
    bundle BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- One row per device login. Tokens are stored only as SHA-256 hashes.
CREATE TABLE sessions (
    session_id BLOB PRIMARY KEY NOT NULL,
    vault_id BLOB NOT NULL REFERENCES vaults (vault_id) ON DELETE CASCADE,
    user_id BLOB NOT NULL REFERENCES users (user_id) ON DELETE CASCADE,
    device_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('native', 'browser')),
    access_hash BLOB NOT NULL UNIQUE,
    access_expires_at INTEGER NOT NULL,
    refresh_hash BLOB NOT NULL UNIQUE,
    refresh_expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL
) STRICT;

CREATE INDEX sessions_vault_id ON sessions (vault_id);
CREATE INDEX sessions_user_id ON sessions (user_id);
CREATE INDEX sessions_refresh_expires_at ON sessions (refresh_expires_at);

-- Every refresh token a session has exchanged, so presenting any of them again
-- (reuse) ends the session. Rows go with their session.
CREATE TABLE refresh_history (
    hash BLOB PRIMARY KEY NOT NULL,
    session_id BLOB NOT NULL REFERENCES sessions (session_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX refresh_history_session_id ON refresh_history (session_id);

CREATE TABLE server_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value BLOB NOT NULL
) STRICT;
