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
| `KRONIKA_DEMO_WORKLOAD_SCHEMAS` | 1 | Commerce schemas to create. |
| `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` | 8 | Recognizable commerce tables to create. |
| `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` | 4 | Concurrent setup connections. |
| `KRONIKA_DEMO_WORKLOAD_SESSIONS` | 4 | Long-lived DML sessions. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` | 1 | Independent lock chains in each bounded round. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` | 4 | Transactions in each lock chain. Together with the hold time, this must let an earlier waiter acquire the row and a later waiter reach the fixed 10-second statement timeout. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` | 4000 | Lock hold time per link in a lock round, milliseconds. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` | 120 | Quiet pause after each lock round, seconds. |
| `KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S` | 180 | Quiet pause after one slow query, one bad statement, and one bad-database attempt. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROWS` | 300000 | Rows maintained in `shop.orders` for the plan-change story. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS` | 4 | Concurrent `checkout-api` sessions exercising the same query. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S` | 12 | Indexed baseline and recovery window, seconds. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S` | 30 | Window without the supporting checkout index, seconds. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S` | 120 | Quiet pause after a complete plan-change round, seconds. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS` | 100000 | Rows in the dedicated Vacuum showcase table. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S` | 180 | Quiet pause after each Vacuum episode, seconds. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S` | 30 | Finite timeout for each update and Vacuum statement, seconds. |

The default workload is one commerce application: `shop.orders`, `customers`,
`order_items`, `products`, `inventory`, `payments`, `event_log`, and `sessions`.
Named clients such as `checkout-api`, `catalog-api`, `payments-worker`, and
`vacuum-worker` make the recorded activity attributable. Steady sessions run bounded
insert, single-row update, select, and delete traffic.

The opening investigation reel runs the same checkout query against
`shop.orders` before, during, and after a supporting index is dropped and
restored. Kronika therefore records two plans under one query ID: a fast indexed
baseline and recovery around a slower sequential-scan interval. A finite row-lock
convoy begins after 65 seconds, Vacuum after 95 seconds, and explicit log/error
events after 140 seconds. Each incident has statement and transaction timeouts
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
