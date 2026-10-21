# Tasks

## 1. Dependencies and crate skeleton

- [ ] 1.1 Add `sqlx` 0.9 (`runtime-tokio`, `sqlite`, `migrate`, `macros`), `tower-http` 0.6 (`fs`, `limit`, `timeout`, `trace`, `set-header`), `tower` 0.5, `governor` 0.10, `clap` 4 (`derive`, `env`), and `toml` to the workspace and `notebook-server`; enable tokio `signal`/`net` and tracing-subscriber `json`/`env-filter`; add `tempfile` and `http-body-util` as dev-dependencies; split the crate into `lib.rs` plus a thin `main.rs` with empty `cli`, `config`, `db`, `app`, `client_ip`, `rate_limit`, `security`, and `static_files` modules and a `migrations/README.md` stating the `vault_id` convention; verify `cargo build -p notebook-server` succeeds

## 2. Configuration and CLI

- [ ] 2.1 Implement `Config` with the design's keys and defaults, layered loading (defaults, TOML file with unknown keys rejected, `NOTEBOOK_*` env), and errors naming the offending key or variable; verify unit tests for env-overrides-file, defaults, no file, missing `public_url`, unknown key, unparseable value, and comma-separated `trusted_proxies`
- [ ] 2.2 Implement the clap CLI (`--config`/`NOTEBOOK_CONFIG`, `serve`, `migrate`, usage on no subcommand) and logging init (`log_level` filter, `text`/`json` format); verify `cargo run -p notebook-server` prints usage and exits non-zero, and a test asserts JSON mode emits one JSON object per line

## 3. Database

- [ ] 3.1 Implement pool setup with WAL, `synchronous=NORMAL`, foreign keys, busy timeout, `create_if_missing`, and embedded migrations, mapping an unknown applied migration to the "schema is newer than this binary" error; wire the `migrate` subcommand; verify tests for first-start creation, idempotent re-migration, and refusal after a fake row is inserted into `_sqlx_migrations`, plus `notebook-server migrate` creating a database and exiting 0

## 4. Router, health, and static hosting

- [ ] 4.1 Implement `AppState`, `app::router()` with `GET /healthz` (`200` on `SELECT 1`, `503` with an empty body otherwise) and the static fallback; verify router tests with `oneshot` for healthy and closed-pool cases
- [ ] 4.2 Implement static serving with `ServeDir`, index on `/`, `404` with no `static_dir`, and the `/assets/` immutable versus `no-cache` header rule; verify router tests against a temp directory for index content type, missing file, `..` and percent-encoded traversal, no static dir, and both cache headers

## 5. Hardening middleware

- [ ] 5.1 Implement security headers (CSP, nosniff, frame denial, referrer policy, HSTS only for an `https` public URL) as the outermost layer; verify router tests that `/healthz`, a `404`, a `413`, and a `429` all carry every header, and that HSTS appears for `https` and not for `http`
- [ ] 5.2 Implement body limit (`413`, with axum's `DefaultBodyLimit` matched to it) and request timeout (`408`); verify router tests for a declared oversize `Content-Length`, a streamed oversize body with no length, and a test-only slow route exceeding a short timeout
- [ ] 5.3 Implement client IP resolution (peer address, right-most untrusted `X-Forwarded-For` entry when the peer is a trusted proxy); verify unit tests for untrusted peer with spoofed header, trusted proxy, chained proxies, and garbage header entries
- [ ] 5.4 Implement `KeyedLimiter` (IPv4 key, IPv6 /64 key), the rate-limit middleware returning `429` with a rounded-up `Retry-After`, the periodic `retain_recent()` task, and the global limiter in the stack; verify router tests using `MockConnectInfo` for burst exhaustion, independent clients, the shared /64 limit, and that a rejected request never reaches the handler
- [ ] 5.5 Implement request tracing that records only method, path without query, status, and latency; verify a test that captures log output for `POST /x?token=abc` with an `Authorization` header and a marker body and asserts none of the secrets appear

## 6. Serving and shutdown

- [ ] 6.1 Implement the `serve` subcommand: migrate, bind, serve with connect info, graceful shutdown on `SIGINT`/`SIGTERM` with a 10-second grace cap, close the pool, and log the loopback-without-`trusted_proxies` warning; verify an integration test that starts the server on an ephemeral port, holds a slow request in flight, sends shutdown, and sees the request complete and the server return Ok

## 7. Documentation

- [ ] 7.1 Write `docs/deployment.md` (building server and web bundle, sample `notebook.toml`, systemd unit, Caddy config with `trusted_proxies`, Litestream replication and restore, database file permissions, rollback by restoring the pre-upgrade backup) and update the README's server section to link it and drop the "stub" wording; verify the sample `notebook.toml`, copied to a temp file with `database_path` pointed at a temp location, runs `notebook-server --config <file> migrate` successfully

## 8. Integration checks

- [ ] 8.1 Run `scripts/check.sh` and verify it exits 0 (fmt, clippy with `-D warnings`, workspace tests, wasm32 core build)
- [ ] 8.2 Run `openspec validate server-foundation --strict` and verify it passes

## Workflow follow-up

- Review the change (code-reviewer and security-analyst agents), then archive it with `/opsx:archive` so the `server-hosting` spec lands in `openspec/specs/`.
- Update `TODO.md`: mark `server-foundation` as `[x]` once archived.
- When `local-sqlite-storage` is implemented, pick a `rusqlite` version whose `libsqlite3-sys` is compatible with sqlx 0.9.
