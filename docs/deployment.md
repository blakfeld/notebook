# Deploying the Notebook server

A deployment is one `notebook-server` binary and one SQLite database file,
behind a TLS-terminating reverse proxy, with the database streamed to object
storage by [Litestream](https://litestream.io). The server only ever holds
ciphertext, but treat the database as sensitive anyway: it is the only copy of
every user's encrypted vault.

## Build

```sh
# Browser client bundle (served as static files)
(cd web && npm ci && npm run build)

# Server binary
cargo build --release -p notebook-server
```

Install `target/release/notebook-server` as `/usr/local/bin/notebook-server` and
copy `web/dist/` to `/usr/local/share/notebook/web/`, owned by root and not
writable by the service user, so a compromised server process can't rewrite
the code it serves:

```sh
install -d -m 0755 /usr/local/share/notebook/web
cp -R web/dist/. /usr/local/share/notebook/web/
```

## Configure

Settings come from built-in defaults, then a TOML file (`--config <path>` or
`NOTEBOOK_CONFIG`), then `NOTEBOOK_<KEY>` environment variables, each
overriding the last. Unknown keys and unparseable values stop the server with
an error naming the setting.

Sample `/etc/notebook/notebook.toml`:

```toml
# The origin users reach the server at. `https` turns on HSTS.
public_url = "https://notes.example.com"

# Loopback only; the reverse proxy is the only thing that talks to the server.
bind = "127.0.0.1:8080"

database_path = "/var/lib/notebook/notebook.db"
static_dir = "/usr/local/share/notebook/web"

# The reverse proxy's address. Its X-Forwarded-For is believed; nobody else's
# is. Leave this empty and every client shares the proxy's rate limit.
trusted_proxies = ["127.0.0.1", "::1"]

max_body_bytes = 1048576
request_timeout_secs = 30
rate_limit_per_second = 20
rate_limit_burst = 100

log_format = "json"
log_level = "info"
```

| key | default | env |
|-----|---------|-----|
| `public_url` | required | `NOTEBOOK_PUBLIC_URL` |
| `bind` | `127.0.0.1:8080` | `NOTEBOOK_BIND` |
| `database_path` | `notebook.db` | `NOTEBOOK_DATABASE_PATH` |
| `static_dir` | unset (no static hosting) | `NOTEBOOK_STATIC_DIR` |
| `trusted_proxies` | empty | `NOTEBOOK_TRUSTED_PROXIES` (comma-separated) |
| `max_body_bytes` | `1048576` | `NOTEBOOK_MAX_BODY_BYTES` |
| `request_timeout_secs` | `30` | `NOTEBOOK_REQUEST_TIMEOUT_SECS` |
| `rate_limit_per_second` | `20` | `NOTEBOOK_RATE_LIMIT_PER_SECOND` |
| `rate_limit_burst` | `100` | `NOTEBOOK_RATE_LIMIT_BURST` |
| `log_format` | `text` | `NOTEBOOK_LOG_FORMAT` (`text` or `json`) |
| `log_level` | `info` | `NOTEBOOK_LOG_LEVEL` (a `tracing` filter) |

`trusted_proxies` takes exact addresses, not CIDR ranges. Trusting a loopback
proxy also trusts any other local process that connects to the server, so
don't run untrusted software on the same host.

IPv6 clients are limited per /64, with additional shared limits for each /56
(16× the per-client quota) and /48 (64×).

## Commands

```sh
notebook-server --config /etc/notebook/notebook.toml migrate   # apply migrations, exit
notebook-server --config /etc/notebook/notebook.toml serve     # migrate, then serve
```

`serve` stops accepting connections on `SIGTERM` or `SIGINT`, gives in-flight
requests up to 10 seconds, closes the database, and exits 0. `GET /healthz`
answers `200` when the database is reachable and `503` when it is not.

## Service user and file permissions

```sh
useradd --system --home /var/lib/notebook --shell /usr/sbin/nologin notebook
install -d -o notebook -g notebook -m 0700 /var/lib/notebook
install -d -o root -g notebook -m 0750 /etc/notebook
install -o root -g notebook -m 0640 notebook.toml /etc/notebook/notebook.toml
```

The database and its `-wal` and `-shm` files are created by the service user
inside `/var/lib/notebook`, which only that user can enter. The `UMask=0077`
below keeps new files private as well.

## systemd

`/etc/systemd/system/notebook.service`:

```ini
[Unit]
Description=Notebook sync server
After=network-online.target
Wants=network-online.target

[Service]
User=notebook
Group=notebook
ExecStart=/usr/local/bin/notebook-server --config /etc/notebook/notebook.toml serve
Restart=on-failure
UMask=0077
TimeoutStopSec=20

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/notebook

[Install]
WantedBy=multi-user.target
```

```sh
systemctl daemon-reload
systemctl enable --now notebook
```

## Caddy

Caddy terminates TLS (obtaining certificates automatically) and proxies to the
loopback address. It sends the real client address as the right-most
`X-Forwarded-For` entry, which the server reads because `127.0.0.1` is in
`trusted_proxies`.

`/etc/caddy/Caddyfile`:

```caddy
{
	servers {
		timeouts {
			read_header 10s
			idle 2m
		}
	}
}

notes.example.com {
	reverse_proxy 127.0.0.1:8080
}
```

The proxy owns slow-client protection: the header-read and idle timeouts above
stop slow or idle clients from holding connections open. The server's
`request_timeout_secs` covers handler time only. No write timeout is set, since
it would cut off long-lived sync connections.

## Backups with Litestream

The server runs SQLite in WAL mode, which Litestream requires. Run Litestream
as its own service, replicating to object storage. It must run as the
`notebook` user: the packaged unit runs as root, and root-owned `-wal` or
`-shm` files would stop the server from opening its database.

`/etc/systemd/system/litestream.service.d/override.conf`:

```ini
[Service]
User=notebook
Group=notebook
UMask=0077
EnvironmentFile=/etc/litestream.env
```

`/etc/litestream.yml` (owned `root:notebook`, mode `0640`):

```yaml
dbs:
  - path: /var/lib/notebook/notebook.db
    replicas:
      - type: s3
        bucket: example-notebook-backups
        path: notebook
        region: us-east-1
```

Put the credentials, `LITESTREAM_ACCESS_KEY_ID` and
`LITESTREAM_SECRET_ACCESS_KEY`, in `/etc/litestream.env`, owned by root with
mode `0600`; systemd reads it before dropping to the service user. Then
`systemctl daemon-reload && systemctl restart litestream`.

### Restore

```sh
systemctl stop notebook litestream
mv /var/lib/notebook/notebook.db /var/lib/notebook/notebook.db.broken
rm -f /var/lib/notebook/notebook.db-wal /var/lib/notebook/notebook.db-shm
set -a; . /etc/litestream.env; set +a      # as root: load the credentials
sudo --preserve-env=LITESTREAM_ACCESS_KEY_ID,LITESTREAM_SECRET_ACCESS_KEY -u notebook \
  litestream restore -config /etc/litestream.yml /var/lib/notebook/notebook.db
systemctl start notebook litestream
```

Add `-timestamp <RFC 3339 time>` to restore to a point in time.

## Upgrades and rollback

Migrations only move forward, and an older binary refuses to start against a
database migrated by a newer one ("database schema is newer than this binary").
So before every upgrade, note the time and take a backup you can return to:

```sh
systemctl stop notebook
date -u +%Y-%m-%dT%H:%M:%SZ        # keep this for a point-in-time restore
sudo -u notebook sqlite3 /var/lib/notebook/notebook.db \
  ".backup /var/lib/notebook/pre-upgrade.db"
install notebook-server /usr/local/bin/notebook-server
systemctl start notebook
```

To roll back: stop the service, put the previous binary back, and restore the
pre-upgrade database, either by copying `pre-upgrade.db` over `notebook.db`
(and deleting its `-wal` and `-shm` files) or with `litestream restore
-timestamp <the time you noted>` as above. Then start the service. Writes made
after the upgrade are lost.
