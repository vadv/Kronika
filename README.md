# Kronika

[Русская версия](README.ru.md)

Kronika records the history of a machine and the databases on it, the way
`atop` records system history, and replays it later. The collector takes
periodic snapshots of operating-system and PostgreSQL metrics, parses logs,
and turns notable log events into metrics. Web reads the recordings and shows
them to people in a browser and to LLM clients over MCP.

![Kronika architecture](docs/images/architecture.svg)

The collector runs all the time on the monitored host; its peak RSS stays
under 25 MiB, and every segment write logs the measured `rss_kib`. Everything
it collects lands in one directory: windows append to `active.wal`, and the
journal is written out as segments named `YYYY/MM/DD/N.zms`. Each segment is
independent and self-contained — opening one requires no other file, no
external schema, no registry lookup. `kronika-web` opens the same directory
read-only: finished segments plus the valid prefix of `active.wal`, all
through the one `kronika-reader` crate.

## Try the demo

Requires Docker with Compose v2.

```sh
make demo-up
```

This builds one container with PostgreSQL 15, PgBouncer, a workload
generator, the collector, and web, then waits until it is healthy. Open
<http://127.0.0.1:8080/> and log in as `demo` / `forensics`. `make demo-stop`
keeps the collected segments; `make demo-clean` removes containers and data.
Details: [bins/kronika-demo/README.md](bins/kronika-demo/README.md).

## Run it on a host

Build with the pinned Rust toolchain (1.96.0, selected automatically by
`rust-toolchain.toml`). npm is not needed: `kronika-web` compiles from the
committed interface artifact.

```sh
cargo build --release --locked -p kronika-collector -p kronika-web
```

The workspace builds static x86-64 Linux binaries by default
(`.cargo/config.toml`; requires `musl-gcc`), so the binaries land in
`target/x86_64-unknown-linux-musl/release/`. To build for the current host
instead, add `--target "$(rustc -vV | sed -n 's/^host: //p')"`.

Start the collector. `KRONIKA_OUT_DIR` is the only required variable; add
`KRONIKA_PG_DSNS` to record PostgreSQL:

```sh
KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_PG_DSNS='host=/var/run/postgresql user=postgres' \
kronika-collector
```

Start web against the same directory:

```sh
KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
KRONIKA_WEB_SOURCES=3 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD=secret \
kronika-web
```

Every variable, interval, and cap is documented in
[bins/kronika-collector/README.md](bins/kronika-collector/README.md) and
[bins/kronika-web/README.md](bins/kronika-web/README.md).

## Connect an LLM

`kronika-web` serves MCP at `POST /mcp`: stateless Streamable HTTP, the same
Basic credentials as the interface, fourteen tools that read what was
recorded and never query the monitored host. The MCP panel in the web
interface's top bar produces a ready-to-paste setup prompt;
[docs/mcp-clients.md](docs/mcp-clients.md) covers Claude Code, Codex CLI,
Cursor, and the `mcp-remote` bridge.

## Documentation

- [DESIGN.md](DESIGN.md) — what Kronika is and the rules it is built under.
- [bins/kronika-collector/README.md](bins/kronika-collector/README.md) —
  collector configuration and operation.
- [bins/kronika-web/README.md](bins/kronika-web/README.md) — web
  configuration, endpoints, MCP.
- [bins/kronika-demo/README.md](bins/kronika-demo/README.md) — the demo
  image and the `kronika-demo` binary.
- [docs/mcp-clients.md](docs/mcp-clients.md) — MCP client setup.
- [docs/type-registry/](docs/type-registry/) — every recorded section and
  field: [OS](docs/type-registry/os.md),
  [PostgreSQL](docs/type-registry/postgresql.md),
  [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md),
  [PgBouncer](docs/type-registry/pgbouncer.md).
- [crates/kronika-format/README.md](crates/kronika-format/README.md) — the
  segment binary format.

Diagrams are generated: sources live in [docs/diagrams/](docs/diagrams/),
`make diagrams` rebuilds the committed SVGs (requires the
[draw.io](https://www.drawio.com) app).

MIT license.
