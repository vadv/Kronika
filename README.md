# Kronika

[Русская версия](README.ru.md)

Kronika records periodic snapshots of a Linux host and its databases for later
inspection. The collector reads operating-system and PostgreSQL metrics and
parses PostgreSQL and PgBouncer logs. `kronika-web` serves the stored data
through a browser interface and MCP. `kronika-report` turns one finished
segment into a self-contained HTML file that opens without a server.

Open the [one-hour synthetic demo report](https://vadv.github.io/Kronika/).
The Pages workflow builds the full production report as one self-contained
HTML file with no external assets.

![Kronika architecture](docs/images/architecture.svg)

The collector writes the current journal to `active.wal` and finished segments
to `YYYY/MM/DD/N.zms` under `KRONIKA_STORAGE_DIR`. `kronika-web` reads those files
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
cargo build --release --locked -p kronika-collector -p kronika-dump -p kronika-report -p kronika-web
```

## Create a time slice

`kronika-dump slice` reads storage from `KRONIKA_STORAGE_DIR`. Both endpoints
are inclusive whole seconds in RFC 3339 form. The command writes exactly the
`.zms` path passed to `--out` and refuses to overwrite an existing file.

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika \
target/x86_64-unknown-linux-musl/release/kronika-dump slice \
  --from 2024-02-29T00:00:00Z \
  --to 2024-02-29T00:59:59Z \
  --out incident.zms
```

The result is one finished standalone ZMS. It can contain up to 30 seconds of
sampling context before and after the requested interval. A range with no
recorded row is an error. The command prints the requested and actual bounds,
row count, section count, and byte length after validating the finished file.
The package library exposes the same slicer to in-process callers, which supply
their own disk-backed scratch file and output sink. The CLI places scratch on
the output filesystem.

## Generate a standalone report

The command accepts one finished ZMS with any `.zms` basename and one `.html`
output path. It derives the internal segment identity from the validated ZMS
catalog:

```sh
target/x86_64-unknown-linux-musl/release/kronika-report \
  /path/to/incident.zms \
  report.html
```

The command builds the isolated canonical IDX and atomically replaces the
output with one deterministic document. That document contains the production
interface, its WebAssembly query engine, the ZMS and the IDX. It uses no
storage root, earlier segment, server, external sidecar, authentication, MCP,
live refresh or network request. A first rate that needs an earlier sample is
`null`. See [bins/kronika-report/README.md](bins/kronika-report/README.md).
The package library also writes this document to a caller-owned sink from ZMS
bytes and an explicit `SegmentId`; the CLI derives that identity locally from
the validated input catalog.

Create the PostgreSQL login used below:

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE kronika_monitor LOGIN PASSWORD 'replace-with-password';
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
SQL
```

This role can read the base PostgreSQL metrics, enumerate databases, inspect
installed `pg_stat_statements` and `pg_store_plans` extensions, and discover the
current PostgreSQL log. This setup assumes PostgreSQL's standard `pg_catalog`
access. Restricted database and extension ACLs are covered in [PostgreSQL
role](bins/kronika-collector/README.md#postgresql-role).

Start the collector. This example collects PostgreSQL and host data; omit the
`KRONIKA_PG_DSNS` line to collect only the host.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
KRONIKA_RETENTION=2147483648 \
KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
target/x86_64-unknown-linux-musl/release/kronika-collector
```

`KRONIKA_RETENTION=2147483648` sets a fixed 2 GiB retention budget; [collector
storage configuration](bins/kronika-collector/README.md#storage) describes the
fixed and automatic modes.

In one measurement with roughly 500 tables and 3,000 indexes, Kronika wrote
about 184 MB/day. A 2 GiB budget therefore keeps roughly 11 days; `active.wal` and
`.idx` files share that budget.

In another terminal, start web with the same data directory.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
KRONIKA_WEB_SOURCES=3 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
target/x86_64-unknown-linux-musl/release/kronika-web
```

Open <http://127.0.0.1:8080/> and sign in as `kronika` with the configured
password. `KRONIKA_WEB_SOURCES` declares which source families the web catalog
marks as configured: `0` neither, `1` OS, `2` PostgreSQL, `3` both. It does not
enable collection or filter stored data. The collector reads PostgreSQL
metrics only when `KRONIKA_PG_DSNS` contains at least one DSN.

Collector and web configuration:

- [bins/kronika-collector/README.md](bins/kronika-collector/README.md)
- [bins/kronika-web/README.md](bins/kronika-web/README.md)

## MCP

`kronika-web` serves MCP at `POST /mcp` and uses the same Basic credentials as
the API. MCP reads stored Kronika data; it does not connect to PostgreSQL or
read current host state. See [docs/mcp-clients.md](docs/mcp-clients.md) for
Claude Code, Codex CLI, Cursor, and `mcp-remote` setup.

## Documentation

- [bins/kronika-demo/README.md](bins/kronika-demo/README.md) — demo commands.
- [bins/kronika-report/README.md](bins/kronika-report/README.md) — standalone
  HTML reports.
- [docs/mcp-clients.md](docs/mcp-clients.md) — MCP client configuration.
- [docs/type-registry/](docs/type-registry/) — recorded sections and fields:
  [OS](docs/type-registry/os.md),
  [PostgreSQL](docs/type-registry/postgresql.md),
  [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md), and
  [PgBouncer](docs/type-registry/pgbouncer.md).
- [crates/kronika-format/README.md](crates/kronika-format/README.md) — segment
  format.

Kronika is licensed under the MIT License.
