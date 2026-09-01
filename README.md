# Kronika

[Русская версия](README.ru.md)

Kronika records periodic snapshots of a Linux host and its databases for later
inspection. The collector reads operating-system and PostgreSQL metrics and
parses PostgreSQL and PgBouncer logs. `kronika-web` serves the stored data
through a browser interface and MCP.

![Kronika architecture](docs/images/architecture.svg)

The collector writes the current journal to `active.wal` and finished segments
to `YYYY/MM/DD/N.zms` under `KRONIKA_OUT_DIR`. `kronika-web` reads those files
and creates derived `N.idx` files in the same directory.

## Run the demo

The demo requires Docker with Compose v2 on amd64 or arm64 Linux.

```sh
make demo-up
```

Open <http://127.0.0.1:8080/> and sign in with user `demo` and password
`forensics`. `make demo-stop` preserves the collected data; `make demo-clean`
removes the demo containers and data. See
[bins/kronika-demo/README.md](bins/kronika-demo/README.md) for details.

## Build and run from source

The default build produces static x86-64 Linux binaries and requires `rustup`
and `musl-gcc`. The repository selects Rust 1.96.0 through
`rust-toolchain.toml`.

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked -p kronika-collector -p kronika-web
```

Start the collector. `KRONIKA_PG_DSNS` enables PostgreSQL collection; omit it
to collect only the host.

```sh
sudo env KRONIKA_OUT_DIR=/var/lib/kronika \
target/x86_64-unknown-linux-musl/release/kronika-collector
```

In another terminal, start web with the same data directory.

```sh
sudo env KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_WEB_SOURCES=1 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
target/x86_64-unknown-linux-musl/release/kronika-web
```

Open <http://127.0.0.1:8080/> and sign in as `kronika` with the configured
password. Set bit 0 in `KRONIKA_WEB_SOURCES` for OS data and bit 1 for
PostgreSQL data; set both bits (`3`) to expose both.

Collector and web configuration:

- [bins/kronika-collector/README.md](bins/kronika-collector/README.md)
- [bins/kronika-web/README.md](bins/kronika-web/README.md)

## MCP

`kronika-web` serves MCP at `POST /mcp` and uses the same Basic credentials as
the API. MCP reads stored Kronika data; it does not connect to PostgreSQL or
read current host state. See [docs/mcp-clients.md](docs/mcp-clients.md) for
Claude Code, Codex CLI, Cursor, and `mcp-remote` setup.

## Documentation

- [DESIGN.md](DESIGN.md) — architecture and product rules.
- [bins/kronika-demo/README.md](bins/kronika-demo/README.md) — demo commands.
- [docs/mcp-clients.md](docs/mcp-clients.md) — MCP client configuration.
- [docs/type-registry/](docs/type-registry/) — recorded sections and fields:
  [OS](docs/type-registry/os.md),
  [PostgreSQL](docs/type-registry/postgresql.md),
  [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md), and
  [PgBouncer](docs/type-registry/pgbouncer.md).
- [crates/kronika-format/README.md](crates/kronika-format/README.md) — segment
  format.

Kronika is licensed under the MIT License.
