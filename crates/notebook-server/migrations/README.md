# Migrations

sqlx migrations, embedded into the binary at compile time with
`sqlx::migrate!("./migrations")` and applied by both `serve` and `migrate`.

Name files `<version>_<description>.sql`; never edit a migration once it has
shipped.

## The `vault_id` convention

Every table that holds user data has a `vault_id` column (a 16-byte ULID
`BLOB`) that is either part of its primary key or the leading column of an
index. Per-vault isolation and per-vault purge then stay cheap, indexed
queries.
