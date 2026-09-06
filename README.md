# Kronika

[Русская версия](README.ru.md)

Kronika records Linux process and host metrics, PostgreSQL statistics, query
plans, and PostgreSQL/PgBouncer log events. The collector writes local journals
and compressed segments; web displays a selected hour, snapshots at a time
cursor, and object histories.

![Process CPU activity and the process snapshot for a recorded hour](docs/images/processes.png)

[Open the interactive preview](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html).

## Install and run

Install a [prebuilt Linux archive](INSTALL.md), or
[build from source](docs/build.md). The archive contains `kronika-collector`,
`kronika-web`, `kronika-dump`, and `kronika-report`.

After installation, start collection on the monitored host:

```sh
sudo install -d -m 0700 /var/lib/kronika
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

In a second terminal, start web over that directory:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

Open <http://127.0.0.1:8080/> and sign in. Web reads the active journal while
collection runs. `Ctrl+C` stops either process and retains the recording.
[Systemd units](docs/services.md) run both programs as services.

### Local and remote PostgreSQL

After [monitoring role setup](INSTALL.md#5-postgresql), stop collector and choose
its connection. For local PostgreSQL in the same VM or container resource scope,
omit `KRONIKA_POSTGRES_EFFECTIVE_CPUS`: CPU capacity is calculated from recorded
CPU snapshots or quota/cpuset.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

For remote PostgreSQL or PostgreSQL in a different cgroup, set the CPU capacity
available to that PostgreSQL server. Example for PostgreSQL with 4 CPUs:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  KRONIKA_POSTGRES_EFFECTIVE_CPUS=4 \
  /usr/local/bin/kronika-collector
```

This is a deployment contract; the DSN address does not establish resource
scope. Restart web with `KRONIKA_WEB_SOURCES=3` for OS and PostgreSQL.

### Storage

For a PostgreSQL workload with roughly 500 tables and 3,000 indexes, estimate
**about 200 MB of compressed recordings per day**. Volume depends on collection
intervals and the number of recorded objects and distinct queries.

`KRONIKA_RETENTION=2147483648` sets the default **2 GiB** storage budget,
including journals and indexes. When the target is exceeded, the collector
automatically removes the oldest finished recordings and their indexes.
For **10 GiB**, set `KRONIKA_RETENTION=10737418240` (raw bytes).

`auto` and `auto:P` instead set a used-space percentage target for the whole
backing filesystem. See [storage configuration](bins/kronika-collector/README.md#storage)
for the rotation rules and automatic mode.

## Recorded data and views

| Domain | Views and values | Reference |
| --- | --- | --- |
| Processes | General, Tree, CPU, Memory, Disk; per-PID counters, rates, command and status; hourly activity and history. | [Linux metrics](docs/metrics-linux.md) |
| Host | CPU, memory, PSI, network, disk counters, filesystem capacity, mount/device topology; container cgroup CPU, memory, I/O and threads. | [Linux metrics](docs/metrics-linux.md) |
| PostgreSQL sessions | Overview, Activity, Locks, Vacuum; backend states, waits, blocker chains, transaction/query ages and maintenance progress. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| PostgreSQL SQL | Statements and Plans; calls, execution/planning time, buffers, temporary I/O, WAL, recorded SQL and plan text. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| PostgreSQL objects | Databases, Tables, Indexes and settings; traffic, size, scans, changes, maintenance counters, transaction ages and database/schema/tablespace groups. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| Events | Grouped PostgreSQL/PgBouncer log events, occurrences, durations and recorded context; metric marks. | [Views and controls](docs/features.md) |
| Time and charts | Calendar hour, cursor, sample selection, interval calculations, heatmaps, totals and percentiles. | [Time and calculations](docs/metrics-time.md) |

[Views and controls](docs/features.md) defines navigation, lenses, grouping,
search, sorting, Inspector, charts and Export. The
[operator guide](docs/operator-guide.md) contains four worked examples from
the preview recording.

![Recorded statement, SQL text and interval activity](docs/images/statements.png)

![Recorded execution plan and associated SQL](docs/images/plans.png)

## Collection and access

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.svg">
  <img alt="Linux and PostgreSQL feed the collector; web reads its recording for the browser and MCP clients" src="docs/images/architecture.svg">
</picture>

The default collection intervals are 5 seconds for processes, 10 seconds for
core Linux metrics, 30 seconds for PostgreSQL metrics, and 300 seconds for
relations.
[Collector configuration](bins/kronika-collector/README.md) defines source
scope, intervals, permissions and storage rotation.

Web serves the browser, HTTP API and MCP on one listener. The **AI** panel
provides MCP client connection settings. [MCP tools](docs/features.md#mcp)
read snapshots, rankings, field definitions, events and complete row details.

## Portable HTML export

**Export** saves a selected interval of your recording as one interactive HTML
file. It embeds the interface, data and Rust/WebAssembly query engine, which
runs on the browser's main thread. Opening the file requires no server or
network connection.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/report-export-dark.svg">
  <img alt="Export an interval from web or a saved recording into one interactive offline HTML file" src="docs/images/report-export.svg">
</picture>

[kronika-dump](bins/kronika-dump/README.md) inspects storage and extracts a ZMS
interval; [kronika-report](bins/kronika-report/README.md) converts a standalone
ZMS into HTML. Offline reports provide local tables, search, charts and heatmaps.

## Documentation

- Setup: [Install](INSTALL.md) · [Archives and CI](docs/releases.md) · [Services](docs/services.md) · [Source build](docs/build.md)
- Reference: [Controls](docs/features.md) · [Time](docs/metrics-time.md) · [Linux](docs/metrics-linux.md) · [PostgreSQL](docs/metrics-postgresql.md) · [MCP](docs/mcp-clients.md)
- Programs: [Collector](bins/kronika-collector/README.md) · [Web](bins/kronika-web/README.md) · [Dump](bins/kronika-dump/README.md) · [Report](bins/kronika-report/README.md)
- Recorded fields: [Linux](docs/type-registry/os.md) · [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md) · [PostgreSQL events](docs/type-registry/postgresql.md) · [PgBouncer events](docs/type-registry/pgbouncer.md)
- Design: [Contracts](DESIGN.md) · [Segment format](crates/kronika-format/README.md) · [Development demo](bins/kronika-demo/README.md)

[MIT License](LICENSE).
