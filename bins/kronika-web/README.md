# kronika-web

[Русская версия](README.ru.md)

`kronika-web` serves recorded Kronika data: an embedded single-page
interface for people and an MCP endpoint for LLM clients. It reads finished
segments and the valid prefix of `active.wal` through `kronika-reader`,
opens the data directory read-only, and keeps no cache between requests. It
has no public command-line interface; environment variables provide its
configuration.

## Configuration

Every variable is read once, at startup. A value that does not parse stops
the server with a message naming the variable and the invalid value.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_OUT_DIR` | — | Data root written by the collector: the journal, the finished segments, and the derived indexes. |
| `KRONIKA_WEB_LISTEN` | `127.0.0.1:8080` | Socket address the HTTP server binds. |
| `KRONIKA_WEB_SOURCES` | — | Decimal bitset of the source families recorded in derived indexes: `1` = OS, `2` = PostgreSQL, `3` = both. |
| `KRONIKA_WEB_USER` | — | Basic-auth user name; non-empty, required even when auth is disabled. |
| `KRONIKA_WEB_PASSWORD` | — | Basic-auth password; non-empty, required even when auth is disabled. |
| `KRONIKA_WEB_AUTH` | `required` | `required` enforces authentication on `/api` and `/mcp`; `disabled` admits every request. |
| `KRONIKA_WEB_COOKIE_SECURE` | `false` | `true` marks the browser session cookie `Secure`, for serving behind TLS. |
| `KRONIKA_WEB_DEMO` | unset | The only accepted value is `synthetic`: marks the catalog and the interface with the synthetic-demo badge. Data still comes from `KRONIKA_OUT_DIR`. |

## Run

```sh
KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
KRONIKA_WEB_SOURCES=3 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD=secret \
kronika-web
```

On success the process prints `ready <addr>` to stdout and serves HTTP/1.1
until stopped.

Authentication accepts either an `Authorization: Basic` header or the
browser session cookie issued by `POST /auth/session`. A request with wrong
or missing credentials gets `401`.

## Endpoints

- `/` — the interface, one self-contained HTML document embedded in the
  binary at build time.
- `/auth/session` — browser login: `GET` checks, `POST` with Basic
  credentials issues the cookie, `DELETE` clears it.
- `/api/*` — the GET-only JSON and NDJSON API the interface uses. Responses
  carry `ETag` and honor `If-None-Match` and `Accept-Encoding`.
- `/mcp` — the MCP endpoint: stateless Streamable HTTP, `POST` with JSON
  responses, tools only. Fourteen tools read what was recorded — ranking
  over an interval, the section catalog, instance metadata, per-section
  finders, row detail, events — and never query the monitored host.
  Requests carrying an `Origin` header are rejected, so browsers cannot
  call it. Client setup: [docs/mcp-clients.md](../../docs/mcp-clients.md);
  the MCP panel in the interface's top bar produces a ready-to-paste
  setup prompt.
