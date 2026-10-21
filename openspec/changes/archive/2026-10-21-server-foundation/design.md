# Design

## Context

See proposal.md (Why). `notebook-server` is a stub `main.rs` that logs the core version. The workspace already pins `axum` 0.8, `tokio`, `tracing`, and `tracing-subscriber`, and `tower-http` 0.6 is in the lockfile through axum. `local-sqlite-storage` uses `rusqlite` with bundled SQLite on clients. `opaque-accounts`, `sync-server`, and `invite-codes` will add routes, tables, rate-limited endpoints, and CLI subcommands on top of this change; `browser-web-client` will put its Vite build in the static directory. Requirements are in `specs/server-hosting/spec.md`.

The server is meant to run behind a TLS-terminating reverse proxy (Caddy or nginx), as one binary plus one SQLite file, backed up by Litestream.

## Goals / Non-Goals

**Goals:**
- A router, state, and middleware stack that later changes extend by adding routes, migrations, and subcommands, without changing the hardening.
- Everything testable in-process: integration tests drive the router with `tower::ServiceExt::oneshot` against a temporary database, with no sockets.

**Non-Goals:**
- Built-in TLS. The proxy terminates TLS; the server binds to loopback by default.
- Any domain tables (`users`, `vaults`, ops). They arrive with the changes that use them.
- A single-page-app fallback that serves `index.html` for unknown paths. `browser-web-client` can add it if it uses path-based routing.
- Header-read and idle-connection timeouts (slowloris protection). The reverse proxy owns those; the request timeout here covers handler time.
- Metrics endpoints, admin HTTP APIs, and hot config reload.

## Decisions

### Module layout

```
crates/notebook-server/
  migrations/          sqlx migrations, embedded at compile time
  src/main.rs          parse CLI, init logging, dispatch subcommand
  src/lib.rs           re-exports for integration tests
  src/cli.rs           clap definitions: --config, serve, migrate
  src/config.rs        Config, layered loading, validation
  src/db.rs            pool setup, pragmas, migrate
  src/app.rs           AppState, router(), middleware stack, serve loop
  src/client_ip.rs     peer / X-Forwarded-For resolution
  src/rate_limit.rs    keyed token-bucket limiter and its middleware
  src/security.rs      security header layers
  src/static_files.rs  ServeDir plus cache headers
  tests/               router-level integration tests
```

A `lib.rs` plus thin `main.rs` lets `tests/` build the router directly.

### Dependencies

- `sqlx` 0.9 with `runtime-tokio`, `sqlite` (bundled SQLite), `migrate`, and `macros` (for `migrate!`). The user chose sqlx over rusqlite for async-native access and built-in migrations.
- `tower-http` 0.6 (already resolved through axum) with `fs`, `limit`, `timeout`, `trace`, `set-header`.
- `tower` 0.5 (`util`), `governor` 0.10 for token buckets, `clap` 4 (`derive`, `env`), `toml` for the file, `serde` (workspace), `tracing-subscriber` gains `json` and `env-filter`.
- tokio gains `signal` (and `net`, if not already enabled).
- Dev: `tempfile` for database files, `http-body-util` to read response bodies.

### Configuration

One flat `Config` struct with `#[serde(deny_unknown_fields)]`. Flat keys map one-to-one to environment variables (`NOTEBOOK_` + upper-case key), so no nesting separator is needed.

| key | type | default |
|-----|------|---------|
| `bind` | socket address | `127.0.0.1:8080` |
| `database_path` | path | `notebook.db` |
| `public_url` | URL (`http`/`https`) | required |
| `static_dir` | path | unset (no static hosting) |
| `trusted_proxies` | list of IPs (env: comma-separated) | empty |
| `max_body_bytes` | integer | `1048576` |
| `request_timeout_secs` | integer | `30` |
| `rate_limit_per_second` | integer | `20` |
| `rate_limit_burst` | integer | `100` |
| `log_format` | `text` / `json` | `text` |
| `log_level` | tracing filter directive | `info` |

Loading: parse the file (if any) into a struct of `Option`s, overlay environment values parsed per key, fill defaults, then validate into `Config`. Each parse error carries the key or env var name. This is a little hand-written code instead of a layering crate like `figment`, which is not worth a dependency for eleven flat keys. `public_url` is parsed only enough to check the scheme and host; no `url` crate.

`trusted_proxies` takes exact IP addresses, not CIDR ranges. A proxy is almost always on loopback or one fixed address, and exact matches avoid an `ipnet` dependency.

### Database

`SqlitePoolOptions` over `SqliteConnectOptions` with `create_if_missing(true)`, `journal_mode(Wal)` (required by Litestream, and lets readers proceed during writes), `synchronous(Normal)`, `foreign_keys(true)`, and `busy_timeout(5s)`. Migrations are embedded with `sqlx::migrate!("./migrations")` and run by both `serve` (before binding) and `migrate`. sqlx's migrator fails with `VersionMissing` when the database has an applied migration the binary lacks; that error is mapped to the "schema is newer than this binary" message.

No tables are created here. The `migrations/` directory starts with only a README that states the convention every later migration follows: every table holding user data has a `vault_id` column (16-byte ULID blob) in its primary key or a leading index, so per-vault isolation and per-vault purge are cheap queries from day one. Tests for the newer-schema refusal insert a fake row into `_sqlx_migrations`.

`/healthz` runs `SELECT 1` on the pool with a short acquire timeout.

### Router and middleware order

`app::router(state: AppState) -> Router`, where `AppState` holds `SqlitePool`, `Arc<Config>`, and the global limiter. Later changes add `.merge(auth::routes())` and similar inside this function. Layers, outermost first:

1. Security headers (so every response, including rejections from inner layers, gets them)
2. Trace (logs the final status of everything)
3. Client IP resolution (inserts a `ClientIp` extension)
4. Global rate limit (`429` before any work is done)
5. Request timeout (`408`)
6. Body limit (`413`)
7. Routes: `/healthz`, then `fallback_service` to the static files

Static files go through the same stack. A first page load is a handful of requests, well inside the default burst.

### Client IP

`ConnectInfo<SocketAddr>` (from `into_make_service_with_connect_info`) gives the peer. If the peer is in `trusted_proxies`, walk `X-Forwarded-For` right to left (splitting raw bytes, so a non-ASCII entry doesn't hide the others), skip trusted proxies and blank entries, and take the first remaining entry. If it doesn't parse as an IP (a proxy may write `unknown` or `unix:`), fall back to the peer; never read further left. Taking the right-most entry matters because the left-most entries are whatever the client claimed. Addresses are compared in canonical form, so IPv4-mapped IPv6 matches IPv4. Tests inject the peer with axum's `MockConnectInfo`.

### Rate limiting

`governor`'s keyed `RateLimiter<IpKey, DashMapStateStore, ...>` wrapped in a small `KeyedLimiter` type with a `check(ip) -> Result<(), Duration>` method. `IpKey` is the IPv4 address, or the IPv6 address masked to /64. IPv6 requests must also pass buckets keyed on the enclosing /56 (16× quota) and /48 (64× quota): a /48 is cheap to obtain and holds 65,536 /64s, so per-/64 keying alone would multiply an attacker's limit by that much, and later login limiters inherit the same keying. A middleware function reads `ClientIp`, calls `check`, and returns `429` with `Retry-After` (whole seconds, rounded up). A background task calls `retain_recent()` once a minute so the key map doesn't grow without bound.

Later endpoints (login, recovery, invite redemption) build their own `KeyedLimiter` with stricter quotas and attach it with `route_layer`, reusing the same middleware. This is why the limiter is ours rather than `tower_governor`'s: `tower_governor`'s IP extractors trust forwarding headers from any peer, and we need the trusted-proxy rule and the IPv6 prefix keying shared by every limiter.

### Timeouts and limits

`tower_http::timeout::TimeoutLayer` with status `408`, and `RequestBodyLimitLayer` for `413`. axum's own `DefaultBodyLimit` (2 MB, extractor-level) is set to the same configured maximum so the two never disagree. The timeout covers producing the response, so a later WebSocket upgrade (`101`) is not cut off. Routes that need bigger bodies (snapshot upload in `sync-server`) can override the limit per route.

### Static files

`ServeDir::new(static_dir)` with `append_index_html_on_directories(true)`. It already handles content types, `..` rejection, percent-decoding, `HEAD`, and conditional requests. Cache headers are set by a `map_response` that checks the request path: `/assets/` prefix gets `public, max-age=31536000, immutable`, everything else `no-cache`. Vite writes hashed files to `assets/` by default, so no Vite config change is needed. With no `static_dir`, the fallback is a plain `404` handler.

### Security headers

Static values set with `SetResponseHeaderLayer::overriding`:

```
Content-Security-Policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval';
  style-src 'self'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self';
  manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
Referrer-Policy: no-referrer
Strict-Transport-Security: max-age=31536000      (only if public_url is https)
```

`connect-src 'self'` covers same-origin `wss:` in current browsers. `'wasm-unsafe-eval'` is needed for the WASM core and allows only WebAssembly compilation, not JavaScript `eval`. `X-Frame-Options` duplicates `frame-ancestors` for older browsers. HSTS has no `includeSubDomains`, since the operator's other subdomains are not ours to constrain. CodeMirror injects `<style>` elements at runtime, so `browser-web-client` will likely need to relax `style-src` (a nonce or `'unsafe-inline'` for styles only); that change modifies this requirement then.

### Logging

`tracing-subscriber` with an `EnvFilter` from `log_level`, and `.json()` or the default formatter depending on `log_format`. `TraceLayer` is configured with a custom `make_span_with` that records only `method` and `uri.path()`, and `on_response` that logs status and latency. The default span, which includes the full URI and could include headers, is not used. No layer logs bodies or headers. Handler errors are logged with their `Display` and never with request data.

### CLI

```
notebook-server [--config <path>] <command>     (--config also from NOTEBOOK_CONFIG)
  serve      load config, migrate, bind, serve until signalled
  migrate    load config, migrate, exit
```

Defined with clap derive and `arg_required_else_help`, so no subcommand prints usage and exits with code 2. The subcommand enum is where `user add`, `invite create`, and similar commands get added later. Logging is initialized before config validation errors are printed, using `info` and text output when the config can't be read.

### Serve loop and shutdown

`tokio::net::TcpListener::bind`, then `axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(signal)`, where `signal` resolves on `ctrl_c` or Unix `SIGTERM`. The serve future is wrapped so that, once the signal fires, the process exits after a 10-second grace period even if connections (later, WebSockets) are still open. The pool is closed before exit so WAL is checkpointed cleanly.

### Deployment documentation

`docs/deployment.md` covers: building the server and the web bundle, a sample `notebook.toml`, a systemd unit, a Caddy config (TLS, `reverse_proxy` to loopback, with the proxy address set in `trusted_proxies`), Litestream replication of the database file and the restore steps, and file permissions (database readable only by the service user). The README's server section links to it and replaces the "stub" wording.

## Risks / Trade-offs

- [sqlx and `rusqlite` (from `local-sqlite-storage`) both link `libsqlite3-sys`, and Cargo allows only one version of a `links = "sqlite3"` crate per workspace] → Choose a `rusqlite` version whose `libsqlite3-sys` range overlaps sqlx 0.9's when `local-sqlite-storage` is implemented, and note the coupling in both crates' `Cargo.toml`. If they can't be reconciled, move the server to its own workspace.
- [Per-IP limits punish users behind shared NAT, and a misconfigured `trusted_proxies` puts every user behind the proxy's one address] → Defaults are generous; the deployment doc shows the proxy setting explicitly; a startup warning is logged when `trusted_proxies` is empty and `bind` is a loopback address, since that usually means a proxy is in front but not configured.
- [Limiter state is in memory: lost on restart and not shared across instances] → Acceptable for a single-binary deployment; restarts only reset counters.
- [The CSP may block parts of the future browser client] → Expected; `browser-web-client` adjusts it in its own spec delta, and the header lives in one module.
- [The request timeout could cut off a slow but legitimate large upload] → Configurable; `sync-server` can raise it per route.
- [A newer migration can't be rolled back by running the old binary] → That's the intent; the deployment doc says to restore the Litestream backup taken before upgrade if a rollback is needed.

## Migration Plan

New deployment, nothing to migrate. Rollback is replacing the binary; this change creates no tables, so there is no schema to undo.
