# kronika-web

[Русская версия](README.ru.md)

`kronika-web` serves recorded Kronika data through a browser interface, an
HTTP API, and MCP. It reads `active.wal` and finished `.zms` segments through
`kronika-reader`, and creates derived `.idx` files in the data directory. The
server is configured only through environment variables.

`kronika-web --version` prints `kronika-web 1.0.0` and exits
without reading configuration, accessing storage, or starting services.
It needs no root access; pass `--version` as the only argument.

## Configuration

Missing required variables and invalid values stop startup before the listener
binds.

| Variable | Default | Description |
| --- | ---: | --- |
| `KRONIKA_STORAGE_DIR` | — | Required data directory. It must contain the collector output and allow web to create and replace `.idx` files and `.kronika-index.owner.lock`. |
| `KRONIKA_WEB_LISTEN` | `127.0.0.1:8080` | HTTP listen address. |
| `KRONIKA_WEB_SOURCES` | — | Required decimal configuration bitset reported by the web catalog: `0` marks neither family configured, `1` marks OS configured (bit 0), `2` marks PostgreSQL configured (bit 1), and `3` marks both configured. Values above `3` are rejected. It does not enable collection or filter stored data. |
| `KRONIKA_WEB_USER` | — | Required non-empty Basic user name, including when authentication is disabled. |
| `KRONIKA_WEB_PASSWORD` | — | Required non-empty Basic password, including when authentication is disabled. |
| `KRONIKA_WEB_AUTH` | `required` | `required` protects `/api/*` and `/mcp`; `disabled` removes the credential check. |
| `KRONIKA_WEB_DEMO` | unset | Accepts only `synthetic`. It marks responses and the interface as synthetic; data still comes from `KRONIKA_STORAGE_DIR`. |

## Run

Install the [portable archive](../../INSTALL.md), then run:

```sh
/usr/local/bin/kronika-web --version
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

The data directory must already exist. On success, the process prints
`ready <addr>` and starts its HTTP/1.1 listener.

Open <http://127.0.0.1:8080/> and sign in as `kronika` with the configured
password.

`KRONIKA_WEB_SOURCES` changes only the `configured` markers in the web catalog.
The browser uses the PostgreSQL marker for its no-data navigation state; the OS
marker currently remains catalog metadata. Stored data remains detected and
readable through the browser, API, and MCP regardless of this value. The
collector reads PostgreSQL metrics only when `KRONIKA_PG_DSNS` contains at
least one DSN.

The roadmap lists ClickHouse, CockroachDB, and MySQL. Their bits are not
assigned yet; values above `3` are rejected.

With `KRONIKA_WEB_AUTH=required`, protected requests accept either Basic
credentials or the browser session cookie issued by `POST /auth/session`.

## Export temporary files

Each export creates two automatically deleted files in the operating system's
standard temporary directory: the sliced ZMS, and a second file used first as
slice scratch space and then as the complete HTML report. On Linux this is
usually `/tmp` when `TMPDIR` is unset. The service account needs write access
to the selected directory and enough free capacity for both files at the same
time.

For a hardened systemd unit that restricts filesystem writes, create a
dedicated directory owned by the service account and allow that same path:

```ini
[Service]
Environment=TMPDIR=/var/tmp/kronika-web
ReadWritePaths=/var/tmp/kronika-web
```

## Endpoints

- `/` — embedded browser interface; accepts `GET` and `HEAD`.
- `/auth/session` — checks a browser session with `GET`, creates one from Basic
  credentials with `POST`, and clears it with `DELETE`. Browser sessions use a
  `Secure` cookie automatically over HTTPS.
- `/api/export?from=<unix_second>&to=<unix_second>` — creates an authenticated
  standalone HTML report for the inclusive range and returns it as an `.html`
  attachment; accepts `GET`.
- Other `/api/*` routes — JSON and NDJSON resources used by the interface;
  accept `GET`.
- `/mcp` — stateless Streamable HTTP endpoint; accepts `POST`. A query string or
  an `Origin` header is rejected. MCP reads stored Kronika data. See
  [MCP client setup](../../docs/mcp-clients.md).
