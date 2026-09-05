# Kronika

[Русская версия](README.ru.md)

**Go back to a slowdown in Linux and PostgreSQL.** Kronika records processes,
resource usage, database activity, SQL queries, execution plans, and log events.
Find when CPU, I/O, or query load changed, then select a process, query, or plan
to inspect what it was doing at that time.

**Export your own data as one interactive HTML file.** In Kronika's web
interface, choose an interval and click **Export**. Send the downloaded `.html` to a
colleague: they open it in a browser, with no Kronika installation, server,
or internet connection. Tables, heatmaps, search, and charts remain interactive.

**[Open the interactive demo →](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html)**
Try that same HTML experience with a full-hour recording of synthetic Linux
and PostgreSQL workloads: 5 September 2026, 19:00–20:00 UTC.
No installation or login; save the file to use it offline.

![Processes: CPU activity heatmap above the process table](docs/images/processes.png)

*Processes in the synthetic demo. Find a busy interval in the heatmap, then
select a process to inspect its CPU, memory, and I/O history.*

## Try it locally

For a live demo, use Docker with Compose v2 on Linux amd64 or arm64.
These commands select the current review branch with the features shown here:

```sh
git clone --branch fix/events-count-scope https://github.com/vadv/Kronika.git kronika
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

## Find the queries and plans from that interval

**Statements: find the SQL that was busy.** Rank queries by execution load,
calls, buffer activity, or WAL. Select a row to read the SQL text and see
how its activity changed during the interval.

![Statements: query activity heatmap, SQL text, and history in the synthetic demo](docs/images/statements.png)

**Plans: inspect how a query ran.** Open the recorded plans for a Query ID,
compare their execution metrics, and read the selected plan alongside its SQL.
Statements and Plans use the history collected from `pg_stat_statements` and
`pg_store_plans` when those extensions are installed.

![Plans: recorded execution plan and related SQL in the synthetic demo](docs/images/plans.png)

*The selected plan and SQL are from this same hour. In this recording,
Plans Activity is unavailable because one recorded plan has duplicate
identifiers; the table and Inspector remain usable.*

For the same time period, inspect **backend waits and blocking processes**,
**Vacuum progress**, **table and index activity**, and **PostgreSQL log events**.
On Linux, follow **disk, network, memory, and CPU history**; in containers,
inspect **cgroup usage, limits, and throttling**.

## Record your own host

Run the collector on your Linux host and point web at its recording directory.
Kronika stores the history in local files; web serves the browser interface
and MCP from the same recording.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.svg">
  <img alt="Linux and PostgreSQL feed the collector; web reads its recording for the browser and MCP clients" src="docs/images/architecture.svg">
</picture>

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
the existing release and building an archive from source.

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
recording. Installed extensions are discovered automatically; log files must
be readable locally. See the [collector guide](bins/kronika-collector/README.md) for database
access, extensions, log paths, and collection intervals.

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

## Export an interval from the command line

The **Export** button is the quickest way to share your own recording.
For a saved recording or a scripted export, use `kronika-dump slice` and
`kronika-report` to create the HTML file.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/report-export-dark.svg">
  <img alt="Export an interval from web or a saved recording into one HTML file to open offline or share" src="docs/images/report-export.svg">
</picture>

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

Slice uses inclusive RFC 3339 seconds. The report options use Unix microseconds
for **19:00–20:00 UTC**, excluding nearby samples retained for calculations.
Omit the report bounds to show the input file's full time range. See the
[dump](bins/kronika-dump/README.md) and [report](bins/kronika-report/README.md)
guides for formats and output options.

## Storage and implementation

One measured workload with **roughly 500 tables and 3,000 indexes** produced
about **184 MB/day of compressed recordings**, extrapolated from 43 finished
segments averaging 1.92 MB at a 15-minute cadence. The default retention target
is **2 GiB** for recorded data, including journals and indexes.
See [storage configuration](bins/kronika-collector/README.md#storage) for the
measurement and retention settings.

The collector targets **under 25 MiB peak RSS on an ordinary host** and logs
its peak memory use on each segment write. The collector and query engine are
written in Rust; the browser interface uses React. HTML reports embed the data,
interface, and WebAssembly query engine, which runs on the browser's main thread.
See the [HTML report guide](bins/kronika-report/README.md).

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
