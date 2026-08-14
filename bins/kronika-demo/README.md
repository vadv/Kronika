# kronika-demo

[Русская версия](README.ru.md)

`kronika-demo` runs `kronika-collector` for a bounded window and reports what
the run cost: segment size, journal size, peak RSS, and CPU time. It is the
data source for the segment-size benchmarks, and it is also the process the
demo Docker image runs to drive the collector against a live PostgreSQL and
PgBouncer.

## Configuration

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_DEMO_DIR` | `demo-data` | Where the run's collector log, segments, and `report.json` are written. |
| `KRONIKA_DEMO_DURATION_S` | 60 | How long to run the collector, seconds. `0` runs until `SIGTERM` or `SIGINT` instead of an already-elapsed deadline. |
| `KRONIKA_COLLECTOR_BIN` | `kronika-collector` next to this binary | Which collector binary to run. |

Any other `KRONIKA_*` variable the collector reads (`KRONIKA_PG_DSNS`,
`KRONIKA_PGBOUNCER_DSNS`, and so on) passes through unchanged: `kronika-demo`
only sets `KRONIKA_OUT_DIR` for the collector, to the run's `segments`
subdirectory.

## The optional workload

Setting `KRONIKA_DEMO_WORKLOAD_DSN` also drives a PostgreSQL workload
alongside the collector, so a fresh run has populated dashboards instead of
an empty one. Unset, `kronika-demo` behaves exactly as it does without this
feature.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_DEMO_WORKLOAD_DSN` | unset | Where the workload connects, normally through PgBouncer. Unset disables the workload entirely. |
| `KRONIKA_DEMO_WORKLOAD_SCHEMAS` | 5 | How many schemas to create. |
| `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` | 400 | How many tables to create in each schema. |
| `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` | 16 | How many connections run `CREATE TABLE` concurrently during setup. |
| `KRONIKA_DEMO_WORKLOAD_SESSIONS` | 8 | How many long-lived sessions run steady-state DML. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` | 4 | How many independent lock chains run in each round. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` | 6 | How many transactions make up one lock chain. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` | 3000 | How long each link in a chain holds its lock before committing. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` | 45 | Pause between lock-chain rounds. |

Table shapes rotate through a fixed set (orders-like, events-like,
jsonb-profile-like, wide-numeric) so `pg_stat_statements` and the system
tables view see genuinely different shapes, not one shape copied thousands
of times. Steady sessions mostly run ordinary insert/update/select/delete,
with a small, fixed share of deliberately slow queries (crossing the 5s
known-bad boundary) and deliberately bad statements (a syntax error, a
connection to a nonexistent database through PgBouncer), so the log-derived
findings have more than a happy path to show. Lock chains are not simulated:
every link in a chain issues the same `UPDATE` against the same row inside
its own transaction, and PostgreSQL's own row-lock queue is what makes the
second link wait for the first.

## Run it

```sh
KRONIKA_COLLECTOR_BIN=target/x86_64-unknown-linux-gnu/debug/kronika-collector \
    kronika-demo
```

`SIGTERM` and `SIGINT` stop the run cleanly: the workload (if any) is told to
stop, the collector is sent `SIGTERM` so it writes its open segment, and the
final report is written before exit.
