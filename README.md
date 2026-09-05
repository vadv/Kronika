# Kronika

[Русская версия](README.ru.md)

**Go back to a slowdown in Linux and PostgreSQL.** Kronika records processes,
CPU, memory, storage, network activity, backend waits, SQL, execution plans,
and log events. Open the recorded hour, find where activity changed, and follow
a process, backend, query, or relation through its history.

![A recorded hour: process CPU activity, timeline, and process table](docs/images/processes.png)

*One real hour of a synthetic workload, 5 September 2026, 19:00–20:00 UTC.
The heatmap locates busy intervals; the table and Inspector show the selected
object at the cursor. [Explore this recording](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html)
is a preview of the interface. Install below to record your own machine.*

## Install and run on your host

Download a **prebuilt static Linux archive** for your architecture from the
[release guide](docs/releases.md), then verify, extract, and install it using
[Install](INSTALL.md). No Rust, Node.js, Docker, or database is required.

**Release status:** the published v1.0.0 predates `--version`, HTML export,
`kronika-report`, and `kronika-dump slice`. The current package is a
commit-qualified **CI review candidate**, not an updated public release.
The release guide identifies the workflow artifacts and tested Linux matrix.

Once the archive is extracted, check and install its programs:

```sh
sha256sum --check SHA256SUMS
for binary in kronika-collector kronika-web kronika-dump kronika-report kronika-demo; do
  "./$binary" --version
done
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report kronika-demo /usr/local/bin/
```

Start with **Linux only** on the machine you want to examine:

```sh
sudo install -d -m 0700 /var/lib/kronika
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

In another terminal, replace the password and open web over the same recording:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

Open **<http://127.0.0.1:8080/>** and sign in. Collection starts immediately;
web reads the active journal, so there is no need to wait for a finished segment.
`sudo` gives the collector access to process details and local logs; using the
same account for web preserves private storage permissions. `Ctrl+C` stops
either process and keeps the recording.

Next, [connect PostgreSQL](INSTALL.md#5-add-postgresql-when-needed) with a
monitoring role and `KRONIKA_PG_DSNS`; use web sources `3` for OS + PostgreSQL.
[Systemd](docs/services.md) keeps collection running with private configuration
files. [Source builds](docs/build.md) and the optional
[Docker demo](bins/kronika-demo/README.md) have separate guides.

## Investigate what happened

Start with **when**: choose a recorded hour and a point on its timeline.
Then find **where**: a pressured resource, busy process, waiting backend, or
expensive query. Open the selected row's history and follow related objects.
Kronika puts observations alongside each other; the operator decides which
ones belong to the same problem.

| Question | What to open |
| --- | --- |
| Which process was using the machine? | **Processes**: Tree, CPU, memory, disk I/O, and general lenses; process search, hourly activity, and recorded command/context. |
| Which resource was busy or stalled? | **Host**: CPU, memory, PSI, devices, mount capacity, storage topology, and network. In containers, the collector's own cgroup CPU, memory, I/O, and thread limits appear first. |
| What was PostgreSQL doing then? | **Overview**, **Activity**, and **Databases**: active sessions, waits, transaction ages, database traffic, WAL, maintenance, and recorded settings. |
| Who was blocking whom? | **Locks**: holder/waiter trees, exact blocking PIDs, lock targets, query text, and recorded backend context. |
| Which SQL consumed the interval? | **Statements**: execution, calls, buffers, rows, planning, and WAL lenses; full SQL and links to recorded plans. |
| How did that query run? | **Plans**: plan-by-plan metrics and recorded text from `pg_store_plans`, linked by Query ID to Statements. |
| Was maintenance involved? | **Vacuum**: episodes, phases, progress, and joined process cost. **Tables / Indexes**: activity, changes, maintenance, size, buffers, and grouping by database, schema, or tablespace. |
| What did the logs report? | **Events**: metric marks and grouped PostgreSQL/PgBouncer log events; inspect individual occurrences, statements, relations, and lock holders. |

The [operator guide](docs/operator-guide.md) turns these paths into worked
investigations using the public hour. The [view and control reference](docs/features.md)
explains every surface, lens, grouping, chart, threshold, and small control:
fixed hour vs cursor, Browser/UTC, refresh, Find, sort, heatmap scales,
Total/Other, row history, Inspector, keyboard use, and mobile layouts.

![Statements: an actual selected query with SQL text and interval activity](docs/images/statements.png)

*Statements connects a busy interval to the actual SQL. Its metrics come from
an installed supported `pg_stat_statements` extension.*

![Plans: actual recorded plan text beside its SQL and plan table](docs/images/plans.png)

*Plans uses a supported `pg_store_plans` installation. In this public recording,
Plans Activity fails because a recorded plan identity repeats at the same
timestamp. Its table and selected-plan Inspector work; the guide states this
limitation and uses those working views.*

## Record once, examine later

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.svg">
  <img alt="Linux and PostgreSQL feed the collector; web reads its recording for the browser and MCP clients" src="docs/images/architecture.svg">
</picture>

The collector runs on the monitored Linux host and writes local files. Web reads
those files for the browser and **MCP**, served by the same process at
`http://127.0.0.1:8080/mcp` with the same authentication. Open the **AI** panel
for client configuration, or use the [MCP guide](docs/mcp-clients.md).
Its tools retrieve recorded rankings, rows, history, definitions, and events;
they do not query the monitored PostgreSQL server or the current host.

The collector targets **under 25 MiB peak RSS on an ordinary host** and logs
its peak memory on every segment write. The default retention target is
**2 GiB**, including journals, compressed recordings, and indexes.
One measured workload with roughly 500 tables and 3,000 indexes extrapolated
to **184 MB/day** of finished compressed segments; it is not a general sizing
promise. [Storage configuration](bins/kronika-collector/README.md#storage)
explains the measurement and what counts toward retention.

## Share an interval

Click **Export** in web to download your chosen interval as one interactive
HTML file. A colleague opens it directly in a browser, without Kronika, a
server, or a network connection. Tables, heatmaps, search, and charts remain
interactive. The file includes the selected recorded data; share it with the
same care as the recording itself.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/report-export-dark.svg">
  <img alt="Export an interval from web or a saved recording into one interactive offline HTML file" src="docs/images/report-export.svg">
</picture>

For saved files and scripted work, [dump](bins/kronika-dump/README.md) inspects
recordings and slices an interval; [report](bins/kronika-report/README.md)
creates the HTML. Its embedded Rust/WebAssembly query engine runs on the
browser's **main thread**. Static reports have no MCP or live refresh.

## Documentation

- **Start:** [Install](INSTALL.md) · [Linux archives and tested systems](docs/releases.md)
  · [Systemd](docs/services.md) · [Build from source](docs/build.md)
- **Operate:** [Investigate an hour](docs/operator-guide.md) · [Views and controls](docs/features.md)
  · [MCP clients](docs/mcp-clients.md)
- **Configure:** [Collector](bins/kronika-collector/README.md) · [Web](bins/kronika-web/README.md)
- **Utilities:** [Demo](bins/kronika-demo/README.md) · [Dump](bins/kronika-dump/README.md)
  · [HTML reports](bins/kronika-report/README.md)
- **Recorded fields:** [Linux](docs/type-registry/os.md) · [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md)
  · [PostgreSQL events](docs/type-registry/postgresql.md) · [PgBouncer events](docs/type-registry/pgbouncer.md)
- **Design:** [Principles and contracts](DESIGN.md) · [Segment format](crates/kronika-format/README.md)

Kronika is open source under the [MIT License](LICENSE).
