# kronika-web

[Русская версия](README.ru.md)

`kronika-web` serves recorded Kronika data through a browser interface, an
HTTP API, and MCP. It reads `active.wal` and finished `.zms` segments through
`kronika-reader`, and creates derived `.idx` files in the data directory. The
server is configured only through environment variables.

## Configuration

Missing required variables and invalid values stop startup before the listener
binds.

| Variable | Default | Description |
| --- | ---: | --- |
| `KRONIKA_OUT_DIR` | — | Required data directory. It must contain the collector output and allow web to create and replace `.idx` files and `.kronika-index.owner.lock`. |
| `KRONIKA_WEB_LISTEN` | `127.0.0.1:8080` | HTTP listen address. |
| `KRONIKA_WEB_SOURCES` | — | Required decimal bitset. Bit 0 enables OS data; bit 1 enables PostgreSQL data. Unsupported bits are rejected. |
| `KRONIKA_WEB_USER` | — | Required non-empty Basic user name, including when authentication is disabled. |
| `KRONIKA_WEB_PASSWORD` | — | Required non-empty Basic password, including when authentication is disabled. |
| `KRONIKA_WEB_AUTH` | `required` | `required` protects `/api/*` and `/mcp`; `disabled` removes the credential check. |
| `KRONIKA_WEB_COOKIE_SECURE` | `false` | `true` adds the `Secure` attribute to the browser session cookie. |
| `KRONIKA_WEB_DEMO` | unset | Accepts only `synthetic`. It marks responses and the interface as synthetic; data still comes from `KRONIKA_OUT_DIR`. |

## Run

Install `rustup` and `musl-gcc`, then run from the repository root:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked -p kronika-web

sudo env KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_WEB_SOURCES=1 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
target/x86_64-unknown-linux-musl/release/kronika-web
```

The data directory must already exist. On success, the process prints
`ready <addr>` and starts its HTTP/1.1 listener.

Open <http://127.0.0.1:8080/> and sign in as `kronika` with the configured
password. For network access, use a TLS-terminating reverse proxy unless you
explicitly trust the network, and set `KRONIKA_WEB_COOKIE_SECURE=true`.

Set every `KRONIKA_WEB_SOURCES` bit whose data web should expose: bit 0 for OS
and bit 1 for PostgreSQL. Combine the bits to expose both.

With `KRONIKA_WEB_AUTH=required`, protected requests accept either Basic
credentials or the browser session cookie issued by `POST /auth/session`.

## Endpoints

- `/` — embedded browser interface; accepts `GET` and `HEAD`.
- `/auth/session` — checks a browser session with `GET`, creates one from Basic
  credentials with `POST`, and clears it with `DELETE`.
- `/api/*` — JSON and NDJSON resources used by the interface; accepts `GET`.
- `/mcp` — stateless Streamable HTTP endpoint; accepts `POST`. A query string or
  an `Origin` header is rejected. MCP reads stored Kronika data. See
  [MCP client setup](../../docs/mcp-clients.md).
