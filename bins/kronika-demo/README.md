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
Overview, Activity, Vacuum, Locks, Statements, Plans, Databases, Tables, and
Indexes when the corresponding samples are present. PgBouncer is represented
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
| `KRONIKA_DEMO_DIR` | `demo-data` | Where the collector log and `report.json` are written. |
| `KRONIKA_STORAGE_DIR` | `$KRONIKA_DEMO_DIR/segments` | Collector storage directory. |
| `KRONIKA_DEMO_DURATION_S` | 60 | Run duration in seconds. `0` runs until `SIGTERM` or `SIGINT`. |
| `KRONIKA_DEMO_COLLECTOR_LOG` | `file` | `file` writes `collector.log`; `stderr` uses inherited stderr. The image uses `stderr` with bounded Docker log rotation. |
| `KRONIKA_COLLECTOR_BIN` | `kronika-collector` beside this binary | Collector binary to run. |

Other `KRONIKA_*` collector variables pass through unchanged.

### Optional PostgreSQL workload

`KRONIKA_DEMO_WORKLOAD_DSN` enables the workload. If it is unset,
`kronika-demo` keeps its original collector-only behavior.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_DEMO_WORKLOAD_DSN` | unset | Workload connection, normally through PgBouncer. |
| `KRONIKA_DEMO_WORKLOAD_DIRECT_DSN` | required with workload | Direct PostgreSQL connection for the plan story and session-scoped Vacuum settings. It must not point at transaction-pooled PgBouncer. The image sets this to its embedded PostgreSQL. |
| `KRONIKA_DEMO_WORKLOAD_SCHEMAS` | 1 | Commerce schemas to create, from 1 through 8. |
| `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` | 8 | Tables per schema, from the 8 commerce tables through 64 total tables. |
| `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` | 4 | Concurrent setup connections, from 1 through 16. |
| `KRONIKA_DEMO_WORKLOAD_SESSIONS` | 4 | Long-lived OLTP clients, from 1 through 16. |
| `KRONIKA_DEMO_WORKLOAD_TPS` | 20 | Maximum aggregate OLTP transactions per second, from 1 through 64. |
| `KRONIKA_DEMO_WORKLOAD_MAX_ORDERS` | 10000 | Reusable live OLTP order slots, from the client count through 50000. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` | 1 | Independent lock chains in each bounded round, from 1 through 4. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` | 4 | Transactions in each lock chain, from 2 through 8. Together with the hold time, this must let an earlier waiter acquire the row and a later waiter reach the fixed 10-second statement timeout. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` | 4000 | Lock hold time per link in a lock round, milliseconds. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` | 120 | Quiet pause after each lock round, seconds. |
| `KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S` | 180 | Quiet pause after one slow query, one bad statement, and one bad-database attempt. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROWS` | 300000 | Rows maintained in `shop.orders` for the plan-change story, from 1 through 500000. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS` | 4 | Concurrent `checkout-api` sessions exercising the same query, from 1 through 8. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S` | 12 | Indexed baseline and recovery window, seconds. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S` | 30 | Window without the supporting checkout index, seconds. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S` | 120 | Quiet pause after a complete plan-change round, seconds. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS` | 100000 | Rows in the dedicated Vacuum showcase table, from 1 through 250000. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S` | 180 | Quiet pause after each Vacuum episode, seconds. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S` | 30 | Finite timeout for each update and Vacuum statement, seconds. |

The default steady workload uses four long-lived `shop-oltp-*` clients through
the PgBouncer connection and runs at most 20 short transactions per second in
total. Each transaction reads one customer and product through their keys,
locks and changes one inventory row, and writes a related order, item, payment,
event, and application session. A client reuses only its own order slots. The
order, item, payment, and linked-event tables therefore retain at most 10000
live OLTP rows each while continuing to produce WAL, buffer, table, and index
activity. A slow transaction reduces the achieved rate; the client does not
replay missed work in a burst.

The commerce relations have primary keys, foreign keys, checks, and the indexes
used by the workload. Reference rows cover 20000 customers and 2048 products.
The plan and Vacuum fixtures use lower IDs; the OLTP slot ring is placed above
both ranges. `checkout-api`, `catalog-api`, `vacuum-worker`, and the other
story-specific names remain visible alongside the steady clients.

The opening investigation reel runs the same checkout query against
`shop.orders` before, during, and after a supporting index is dropped and
restored. Kronika therefore records two plans under one query ID: a fast indexed
baseline and recovery around a slower sequential-scan interval. A finite row-lock
convoy begins after 65 seconds, Vacuum after 95 seconds, and explicit log/error
events after 140 seconds. Each story has statement and transaction timeouts
and a long quiet period, so the historical screens show both the problem and
the recovery while the live database remains usable. The image samples
PostgreSQL every 5 seconds so each bounded episode crosses at least one
collection tick. No scenario disables `statement_timeout` or
`idle_in_transaction_session_timeout`.

For a direct binary run:

```sh
KRONIKA_COLLECTOR_BIN=target/x86_64-unknown-linux-gnu/debug/kronika-collector \
KRONIKA_DEMO_WORKLOAD_DSN='host=127.0.0.1 port=6432 user=kronika_demo dbname=kronika_demo' \
KRONIKA_DEMO_WORKLOAD_DIRECT_DSN='host=127.0.0.1 port=5432 user=kronika_demo dbname=kronika_demo' \
    kronika-demo
```

`SIGTERM` and `SIGINT` stop the workload and collector, close the active
segment, and write the final report before exit.
