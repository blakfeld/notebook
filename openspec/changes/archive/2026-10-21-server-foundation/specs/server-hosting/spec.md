# Spec Delta

## Purpose

Defines how the Notebook server runs as a single publicly exposed binary: its configuration, database lifecycle, health reporting, static hosting of the browser client, request limits, rate limiting, security headers, logging hygiene, and the operator command line.

## ADDED Requirements

### Requirement: Layered configuration
The server SHALL read its settings from built-in defaults, then an optional TOML configuration file, then `NOTEBOOK_`-prefixed environment variables, with each later source overriding earlier ones. The file path SHALL be given by a command-line option or the `NOTEBOOK_CONFIG` environment variable.

#### Scenario: Environment overrides the file
- **WHEN** the configuration file sets the bind address to one value and `NOTEBOOK_BIND` sets another
- **THEN** the server listens on the address from `NOTEBOOK_BIND`

#### Scenario: Defaults apply when unset
- **WHEN** neither the file nor the environment sets the maximum request body size
- **THEN** the server uses its documented default

#### Scenario: No configuration file
- **WHEN** no configuration file is given and all required settings come from the environment
- **THEN** the server starts normally

### Requirement: Invalid configuration is rejected
The server SHALL refuse to start, exiting non-zero with a message naming the offending setting, when a required setting is missing, a value cannot be parsed, or the configuration file contains an unknown key.

#### Scenario: Missing public URL
- **WHEN** the server is started without a public URL configured
- **THEN** it exits non-zero and the error names the public URL setting

#### Scenario: Misspelled key in the file
- **WHEN** the configuration file contains a key the server does not recognize
- **THEN** it exits non-zero and the error names that key

#### Scenario: Unparseable value
- **WHEN** `NOTEBOOK_MAX_BODY_BYTES` is set to text that is not a number
- **THEN** it exits non-zero and the error names that setting

### Requirement: Database lifecycle
The server SHALL store its data in a single SQLite file at the configured path, creating it if absent and applying any pending schema migrations before accepting requests. Applying migrations to an up-to-date database SHALL change nothing.

#### Scenario: First start
- **WHEN** the server starts and no database file exists at the configured path
- **THEN** it creates the file, applies all migrations, and begins serving

#### Scenario: Restart is idempotent
- **WHEN** the server is restarted against a database it already migrated
- **THEN** it starts without altering the schema

### Requirement: Newer database schema is refused
The server SHALL refuse to start, exiting non-zero, when the database records a migration that this server binary does not know, so an older binary never runs against a schema written by a newer one.

#### Scenario: Downgraded binary
- **WHEN** the database contains a migration unknown to the running binary
- **THEN** the server exits non-zero with an error saying the database schema is newer than the binary

### Requirement: Health endpoint
`GET /healthz` SHALL respond `200` when the server can query its database and `503` when it cannot. The response SHALL NOT reveal version numbers, configuration, or internal error details.

#### Scenario: Healthy
- **WHEN** a client requests `GET /healthz` and the database is reachable
- **THEN** the response status is `200`

#### Scenario: Database unavailable
- **WHEN** a client requests `GET /healthz` and the database cannot be queried
- **THEN** the response status is `503` and the body contains no error details

### Requirement: Static client hosting
When a static directory is configured, the server SHALL serve files from it at matching request paths, serving `index.html` for `/`, with a content type matching each file's extension. Requests for missing files SHALL get `404`, and no request SHALL read a file outside the static directory.

#### Scenario: Index page
- **WHEN** a client requests `GET /` and the static directory contains `index.html`
- **THEN** the server responds `200` with that file as `text/html`

#### Scenario: Missing file
- **WHEN** a client requests a path that has no file in the static directory
- **THEN** the server responds `404`

#### Scenario: Path traversal
- **WHEN** a client requests a path containing `..` segments or encoded equivalents that would resolve outside the static directory
- **THEN** the server does not return any file from outside the static directory

#### Scenario: No static directory configured
- **WHEN** no static directory is configured and a client requests `GET /`
- **THEN** the server responds `404` and its other endpoints keep working

### Requirement: Client asset caching
Files under the static directory's `assets/` path, whose names contain a content hash, SHALL be served with `Cache-Control: public, max-age=31536000, immutable`. All other static files, including `index.html`, SHALL be served with `Cache-Control: no-cache` so clients always revalidate the entry point.

#### Scenario: Hashed asset
- **WHEN** a client requests a file under `/assets/`
- **THEN** the response carries `Cache-Control: public, max-age=31536000, immutable`

#### Scenario: Entry point
- **WHEN** a client requests `/` or `/index.html`
- **THEN** the response carries `Cache-Control: no-cache`

### Requirement: Request body size limit
The server SHALL reject any request whose body exceeds the configured maximum (default 1 MiB) with `413`, whether the size is declared up front or only discovered while reading the body.

#### Scenario: Declared oversize body
- **WHEN** a request declares a `Content-Length` larger than the maximum
- **THEN** the server responds `413` without reading the body

#### Scenario: Streamed oversize body
- **WHEN** a request without a `Content-Length` streams more bytes than the maximum
- **THEN** the server responds `413`

### Requirement: Request timeout
The server SHALL abandon any request whose handling exceeds the configured timeout (default 30 seconds) and respond `408`.

#### Scenario: Slow handler
- **WHEN** handling a request takes longer than the configured timeout
- **THEN** the client receives `408`

### Requirement: Per-client rate limiting
The server SHALL limit the request rate from each client address with a token bucket whose sustained rate and burst size are configurable. A request over the limit SHALL get `429` with a `Retry-After` header and SHALL NOT reach the handler. IPv6 clients SHALL be limited per /64 prefix, and each enclosing /56 and /48 prefix SHALL also be limited, with 16 and 64 times the per-client quota respectively, so rotating addresses within one allocation does not multiply the limit.

#### Scenario: Burst exhausted
- **WHEN** one client sends more requests than the burst size faster than the sustained rate refills
- **THEN** the excess requests get `429` with a `Retry-After` header

#### Scenario: Clients are independent
- **WHEN** one client is being rate limited and another client sends a request
- **THEN** the other client's request is served normally

#### Scenario: IPv6 prefix
- **WHEN** requests arrive from two IPv6 addresses in the same /64 prefix
- **THEN** they draw from the same limit

#### Scenario: Rotating IPv6 prefixes
- **WHEN** one client sends requests from many different /64 prefixes within the same /48
- **THEN** once the /56 or /48 limit is exhausted, the excess requests get `429`

### Requirement: Client address behind a trusted proxy
The server SHALL use the TCP peer address as the client address, except that when the peer is a configured trusted proxy it SHALL use the right-most address in `X-Forwarded-For` that is not itself a trusted proxy. If the right-most entry that is not a trusted proxy is not a valid address, the server SHALL use the peer address rather than any entry further left. Forwarding headers from untrusted peers SHALL be ignored.

#### Scenario: Request through the proxy
- **WHEN** a request arrives from a trusted proxy with `X-Forwarded-For: 203.0.113.7`
- **THEN** the server treats `203.0.113.7` as the client address

#### Scenario: Proxy writes a non-address entry
- **WHEN** a request arrives from a trusted proxy with `X-Forwarded-For: 6.6.6.6, unknown`
- **THEN** the server uses the peer address, not `6.6.6.6`

#### Scenario: Spoofed header from the internet
- **WHEN** a request arrives directly from an untrusted peer with an `X-Forwarded-For` header
- **THEN** the server ignores the header and uses the peer address

### Requirement: Security headers
Every response SHALL carry `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, and a Content-Security-Policy that allows scripts, styles, and network connections only from the server's own origin, forbids inline scripts and `eval` (WebAssembly compilation excepted), and forbids framing and plugins.

#### Scenario: Headers on an API response
- **WHEN** a client requests `GET /healthz`
- **THEN** the response carries all the security headers listed above

#### Scenario: Headers on an error response
- **WHEN** a request is rejected with `404`, `413`, or `429`
- **THEN** the response still carries all the security headers

### Requirement: Strict transport security
When the configured public URL uses `https`, every response SHALL carry `Strict-Transport-Security: max-age=31536000`. When it uses `http`, the header SHALL be omitted.

#### Scenario: HTTPS deployment
- **WHEN** the public URL starts with `https://` and a client makes any request
- **THEN** the response carries `Strict-Transport-Security: max-age=31536000`

#### Scenario: Local HTTP
- **WHEN** the public URL starts with `http://`
- **THEN** responses carry no `Strict-Transport-Security` header

### Requirement: Logging hygiene
The server SHALL log each request's method, path without query string, response status, and duration. Logs SHALL NOT contain request or response bodies, query strings, or the values of `Authorization`, `Cookie`, or `Set-Cookie` headers. Output SHALL be human-readable text or JSON lines, as configured.

#### Scenario: Request is logged without secrets
- **WHEN** a client sends `POST /x?token=abc` with an `Authorization` header and a body containing a marker string
- **THEN** the log records the method, `/x`, the status, and the duration, and contains neither `abc`, the authorization value, nor the marker

#### Scenario: JSON output
- **WHEN** the log format is set to JSON
- **THEN** each log line is a single JSON object

### Requirement: Operator command line
The server binary SHALL expose operator tasks as subcommands: `serve` runs the server, and `migrate` applies pending migrations and exits. Every subcommand SHALL accept the configuration file option. Running the binary with no subcommand or an unknown one SHALL print usage and exit non-zero.

#### Scenario: Migrate only
- **WHEN** an operator runs `notebook-server migrate` against a new database path
- **THEN** the database is created and migrated, the command exits 0, and no port is opened

#### Scenario: No subcommand
- **WHEN** an operator runs `notebook-server` with no arguments
- **THEN** it prints usage listing the subcommands and exits non-zero

### Requirement: Graceful shutdown
On `SIGTERM` or `SIGINT`, the server SHALL stop accepting new connections, let in-flight requests finish for up to a fixed grace period, then exit 0.

#### Scenario: Request in flight during shutdown
- **WHEN** the server receives `SIGTERM` while a request is being handled
- **THEN** that request completes normally and the process then exits 0
