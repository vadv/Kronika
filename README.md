# Kronika

[Русская версия](README.ru.md)

**Inspect what happened on Linux and PostgreSQL, at the same recorded moment.**
Kronika keeps process and resource history alongside database activity, queries,
execution plans, locks, and log events. Pick an hour, find the busy interval in
an activity heatmap, then follow a process into its PostgreSQL backend, related
statements, and plans without losing your place in time.

**[Open the interactive browser demo →](https://vadv.github.io/kronika-reports/reports/kronika-container-demo-20min-77c422e.html)**
No installation or login. Explore a recording of synthetic PostgreSQL and Linux
workloads in the real Kronika interface, or save the HTML and open it offline.

**Processes — see who used CPU, memory, and I/O, and when.** Activity heatmaps
put the hour's distribution above the process table; select a row or cell to
inspect its history and recorded PostgreSQL activity.

![Process activity heatmap and recorded process details](docs/images/processes.png)

**Statements — go from a busy interval to the SQL behind it.** Compare execution
time, calls, buffer activity, and WAL, with query text and history beside the
selected row.

![PostgreSQL statements with activity heatmap and query details](docs/images/statements.png)

**Plans — inspect the plans that were actually recorded.** Follow a Query ID
from Statements to Plans and compare execution metrics alongside the stored
plan text.

![Recorded PostgreSQL execution plans and their metrics](docs/images/plans.png)

## A machine's history, ready to inspect

Kronika is built for the moment after a slowdown, when the processes have moved
on and the database's current views no longer show what you needed to see.
The collector records continuously; the browser and MCP read that history.

- **One time cursor across Linux and PostgreSQL.** Resource pressure, process
  CPU and I/O, running and waiting backends, locks, and events remain in the
  same hour as you move between views.
- **Heatmaps make dense history readable.** Rank processes, statements, plans,
  tables, and indexes by the metric you are investigating. Select an interval
  to inspect its rows; open one entity's chart for the detail.
- **Useful depth without a separate metrics stack.** The collector writes local
  files. Web serves those files with its interface embedded in the binary and
  releases request data when idle. No external database is needed for Kronika's
  own storage.
- **An investigation can travel as one file.** Export the selected interval as
  a self-contained HTML report with the production interface and query engine.
  The recipient needs only a browser.

| Recorded area | What you can inspect |
| --- | --- |
| Linux processes | CPU, RSS, swap, disk and logical I/O, scheduler delays, page faults, context switches, users, command lines |
| Linux resources | Per-CPU activity, memory, PSI, disk throughput and latency, mounts and capacity, network interfaces and TCP counters, CPU frequency and topology |
| Containers | Cgroup CPU and throttling, memory and limits, I/O by device and mount, threads; container, network namespace, and host readings retain their own scope |
| PostgreSQL activity | Backend state and waits, transactions, blocker chains, Vacuum progress, databases and settings |
| Statements and plans | `pg_stat_statements` and `pg_store_plans` history: calls, execution and planning time, rows, buffer activity, WAL, query and plan text where the installed layout supplies them |
| Tables and indexes | Size, scans, buffer activity, row changes, Vacuum and Analyze statistics, transaction age; inspect by object, schema, database, or tablespace |
| Log events | PostgreSQL errors, slow queries, checkpoints, Autovacuum, lock waits and lifecycle events; PgBouncer log events |

Available fields depend on the kernel, PostgreSQL version, installed extensions,
and permissions. The [metric references](#documentation) list the actual
sections and layouts. Missing readings stay missing.

## Try it locally

For a live demo, use Docker with Compose v2 on Linux amd64 or arm64. From a
checkout of this repository:

```sh
git clone https://github.com/vadv/Kronika.git kronika
cd kronika
docker compose --file compose.demo.yml up --build --wait
```

Open **<http://127.0.0.1:8080/>**, user **`demo`**, password **`forensics`**.
The first run builds the image. It includes PostgreSQL 15, PgBouncer, collector,
web, and a bounded synthetic workload with OLTP traffic, changing plans, lock
waits, Vacuum, and Linux CPU, memory, disk, and loopback activity. No external
database is required.

```sh
docker compose --file compose.demo.yml logs --follow --tail=100
docker compose --file compose.demo.yml stop
```

Stopping preserves the recorded history. See the
[demo guide](bins/kronika-demo/README.md) for another port, workload controls,
and removing the demo's data.

## Record your own host

### Build the current binaries

The supported static native target is **x86-64 Linux with musl**. Install
`rustup`, a C build toolchain, and `musl-gcc` (`build-essential` and `musl-tools`
on Debian/Ubuntu), then run from the repository root:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump -p kronika-report
```

The repository pins Rust 1.96.0. The browser interface and report WebAssembly
are already bundled: a normal Cargo build needs no Node.js installation.

The published [v1.0.0 archive](https://github.com/vadv/Kronika/releases/tag/v1.0.0)
contains collector, web, dump, and demo; **it predates `kronika-dump slice`,
`kronika-report`, and HTML export**. Use the source build above for the features
on this page until an updated archive is published. There is no prebuilt arm64
archive. [Packaging and verified download commands](docs/releases.md) describe
both the existing release and the prepared artifacts.

### Start the collector

For PostgreSQL, create a monitoring login on the server:

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE kronika_monitor LOGIN PASSWORD 'replace-with-password';
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
SQL
```

Run the collector on the monitored Linux host. Replace the sample password in
both commands. It needs access to process details and local log files; `sudo`
is the straightforward local setup.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  ./target/x86_64-unknown-linux-musl/release/kronika-collector
```

`KRONIKA_PG_DSNS` enables PostgreSQL collection on its own. Omit it for a Linux-only
recording. The first DSN supplies metrics across its connectable databases;
installed extensions are discovered automatically. Log files must be readable
locally. Optional log paths, extension permissions, intervals, and PostgreSQL
health capacity are in the [collector guide](bins/kronika-collector/README.md).

### Open web and MCP

In another terminal, use the same data directory:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_SOURCES=3 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  ./target/x86_64-unknown-linux-musl/release/kronika-web
```

Open **<http://127.0.0.1:8080/>** and sign in with the configured credentials.
Web binds to loopback by default. `KRONIKA_WEB_SOURCES=3` marks OS and PostgreSQL
as configured source families (`1` for OS only); it neither enables collection
nor filters stored data. The [web guide](bins/kronika-web/README.md) covers
configuration and API access.

MCP is served by this same process at **`http://127.0.0.1:8080/mcp`**, with the
same authentication. It lets an MCP client retrieve recorded rankings, rows,
histories, and events for an investigation. No separate MCP server is needed.
For example, list the tools using the web credentials (`curl` prompts for the
password):

```sh
curl --fail --silent --show-error --user kronika \
  --header 'Content-Type: application/json' \
  --header 'Accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
  http://127.0.0.1:8080/mcp
```

For Claude Code, Codex CLI, and Cursor, use the web interface's MCP panel or the
[MCP client guide](docs/mcp-clients.md). MCP reads the recording; it does not
query the monitored PostgreSQL server or current host state. Static HTML
reports have no MCP endpoint.

## Keep and share an interval

Use **Export** in web to download one `.html` for a selected interval. The
report embeds the production React interface, the Rust `kronika-query` engine
compiled to WebAssembly and running single-threaded in a Web Worker, the
selected ZMS data, and its canonical IDX. Open it directly in a browser: it
needs no running server, network connection, external assets, or sidecar files.
It retains interactive tables, heatmaps, search, and charts.

The command-line utilities work with the same recording. Inspect section sizes:

```sh
sudo ./target/x86_64-unknown-linux-musl/release/kronika-dump /var/lib/kronika
```

Create a slice and turn it into a report; replace the times with an interval in
your recording:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  ./target/x86_64-unknown-linux-musl/release/kronika-dump slice \
  --from 2026-09-05T19:00:00Z --to 2026-09-05T19:59:59Z \
  --out incident.zms
sudo chown "$(id -u):$(id -g)" incident.zms

./target/x86_64-unknown-linux-musl/release/kronika-report \
  --from 1788634800000000 --to-exclusive 1788638400000000 \
  incident.zms incident.html
```

The dump storage root must be a real directory, not a standalone file or a
symlink. Slice endpoints are inclusive whole seconds in RFC 3339; the new ZMS
may retain up to 30 seconds of nearby samples for interval calculations. The
report bounds above are Unix microseconds for exactly **19:00–20:00 UTC** and
keep that context out of navigation. Without those options, report shows the
entire input's time range. Slice refuses an existing output; report atomically
replaces its HTML output. See [dump](bins/kronika-dump/README.md) and
[report](bins/kronika-report/README.md) for details.

## Collection cost

An observed workload with **roughly 500 tables and 3,000 indexes** produced
about **184 MB/day of compressed ZMS**, extrapolated from 43 finished segments
averaging 1.92 MB at a 15-minute cadence. This measures the finished recording,
not the whole storage directory or every workload.

The default retention target is **2 GiB**, shared by finished ZMS, `active.wal`,
IDX, and recognized temporary files. The number of retained days depends on
those costs and your workload. Set `KRONIKA_RETENTION` to a byte budget or an
automatic filesystem target; details and the measurement are in
[storage configuration](bins/kronika-collector/README.md#storage).

The collector's design bound is **under 25 MiB peak RSS on an ordinary host**;
a [container collection scenario](crates/kronika-bdd/features/container.feature)
checks that threshold. Each segment write logs peak RSS as `rss_kib`. This is
not a universal memory guarantee for every process count or PostgreSQL workload.

## Documentation

- [Collector](bins/kronika-collector/README.md) · [Web](bins/kronika-web/README.md)
  · [MCP clients](docs/mcp-clients.md)
- [Demo](bins/kronika-demo/README.md) · [Dump](bins/kronika-dump/README.md)
  · [HTML reports](bins/kronika-report/README.md) · [Release archives](docs/releases.md)
- Recorded fields: [Linux](docs/type-registry/os.md),
  [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md),
  [PostgreSQL log events](docs/type-registry/postgresql.md),
  [PgBouncer log events](docs/type-registry/pgbouncer.md)
- [Segment format](crates/kronika-format/README.md)

Kronika is open source under the [MIT License](LICENSE).
