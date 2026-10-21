# Proposal

## Why

Notebook is hosted publicly. A single server binary has to serve the browser client's static bundle, the sync API, and later the accounts API, while holding only ciphertext. Establishing the server runtime, configuration, database, and baseline hardening first gives accounts and sync a safe place to land.

## What Changes

- An axum server in `notebook-server` that runs as a single binary with:
  - Configuration from file and environment (bind address, database path, public URL, limits).
  - A SQLite database with migrations (`sqlx` or `rusqlite`), keyed by `vault_id` from the start.
  - A health endpoint (`GET /healthz`).
  - Static file serving for the browser client bundle with long-cache hashed assets.
- Baseline hardening suited to public exposure:
  - Request body size limits and timeouts.
  - Rate limiting middleware (per IP) usable by later endpoints.
  - Security headers: strict Content-Security-Policy, HSTS, `X-Content-Type-Options`, frame denial.
  - Structured logging that never logs request bodies or secrets.
- An admin CLI entry point (`notebook-server <subcommand>`) for operator tasks, which later changes extend.
- Deployment documentation: one binary plus one SQLite file, with streaming backups (e.g. Litestream).

## Capabilities

### New Capabilities
- `server-hosting`: Server runtime behavior: configuration, health reporting, static client hosting, request limits, rate limiting, security headers, logging hygiene, and the operator CLI surface.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-server` crate: axum, tower middleware, SQLite driver, `clap` for the CLI.
- Defines the HTTP surface that accounts, invites, and sync extend.
- The operator needs TLS termination (reverse proxy or built-in), documented here.
