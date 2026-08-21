# Kronika interactive demo

[Русская версия](README.ru.md)

The demo starts PostgreSQL 15, PgBouncer, the Kronika collector, a bounded
synthetic workload, and the Kronika web UI in one Docker Compose service. It
uses the real collection and storage paths and needs no external database or
private environment.

## Start and inspect

Requirements: Docker with Compose v2 on an amd64 or arm64 Linux host.

```sh
make demo-up
```

The command builds the image, starts the service, waits for its health check,
and prints the URL. Open <http://127.0.0.1:8080/> and sign in with:

```text
Username: demo
Password: forensics
```

Processes is the default view. Host, Processes, PostgreSQL, and Events expose
the collected hour through the normal Kronika UI. PostgreSQL includes
Overview, Activity, Statements, Plans, Locks, Databases, Tables, Indexes, and
Settings when the corresponding samples are present. PgBouncer is represented
by its real log events under Events; Kronika does not currently have a separate
PgBouncer dashboard.

Move the timeline cursor to inspect a recorded moment. Select a table row to
open its Inspector detail, use the chart button for the Inspector chart, and
press Escape to close it. The visible search controls filter the current
surface; browser Back restores addressable navigation state.

The small `DEMO · synthetic data` label distinguishes this dataset from a real
host. The workload and its credentials are local to the Compose network. The
running UI uses no remote fonts, assets, or packages.

If port 8080 is occupied, choose another loopback port:

```sh
DEMO_PORT=18081 make demo-up
```

Inspect health and follow service logs with:

```sh
make demo-status
make demo-logs
```

## Stop and remove data

Stop the container while preserving collected Kronika history:

```sh
make demo-stop
```

Run `make demo-up` to start it again. PostgreSQL and PgBouncer use ephemeral
tmpfs filesystems and are recreated on every container start; the named volume
contains only Kronika history. Retention is capped at 512 MiB.

Remove the container, network, and named demo-data volume:

```sh
make demo-clean
```

The next `make demo-up` creates a clean demo. Image construction downloads the
pinned base images, locked Cargo dependencies, and the exact pg_store_plans
source revision. Normal runtime does not require network access beyond the
browser connecting to the published loopback port.

## `kronika-demo` binary

The binary runs `kronika-collector` for a bounded window and reports segment
size, journal size, peak RSS, and CPU time. The image uses it as the supervisor
for the collector and optional PostgreSQL workload.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_DEMO_DIR` | `demo-data` | Where the collector log, segments, and `report.json` are written. |
| `KRONIKA_DEMO_DURATION_S` | 60 | Run duration in seconds. `0` runs until `SIGTERM` or `SIGINT`. |
| `KRONIKA_DEMO_COLLECTOR_LOG` | `file` | `file` writes `collector.log`; `stderr` uses inherited stderr. The image uses `stderr` with bounded Docker log rotation. |
| `KRONIKA_COLLECTOR_BIN` | `kronika-collector` beside this binary | Collector binary to run. |

Other `KRONIKA_*` collector variables pass through unchanged. `kronika-demo`
sets only `KRONIKA_OUT_DIR` to the run's `segments` subdirectory.

### Optional PostgreSQL workload

`KRONIKA_DEMO_WORKLOAD_DSN` enables the workload. If it is unset,
`kronika-demo` keeps its original collector-only behavior.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_DEMO_WORKLOAD_DSN` | unset | Workload connection, normally through PgBouncer. |
| `KRONIKA_DEMO_WORKLOAD_SCHEMAS` | 4 | Schemas to create. |
| `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` | 40 | Tables per schema. |
| `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` | 4 | Concurrent setup connections. |
| `KRONIKA_DEMO_WORKLOAD_SESSIONS` | 4 | Long-lived DML sessions. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` | 2 | Total lock chains: one remains contended and the others run in rounds. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` | 3 | Transactions in each lock chain; must be at least two. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` | 1500 | Lock hold time per link in the cycling chains, milliseconds. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` | 30 | Pause between cycling lock rounds, seconds. |

Each session uses a fixed pseudo-random sequence. Four table shapes produce
varied statements across 160 tables without pathological setup load. Steady
sessions run bounded insert, update, select, and delete traffic, plus a small
fixed share of slow and failing statements for real log-derived events. Lock
chains use PostgreSQL row locks; they are not fabricated findings.

For a direct binary run:

```sh
KRONIKA_COLLECTOR_BIN=target/x86_64-unknown-linux-gnu/debug/kronika-collector \
    kronika-demo
```

`SIGTERM` and `SIGINT` stop the workload and collector, close the active
segment, and write the final report before exit.
