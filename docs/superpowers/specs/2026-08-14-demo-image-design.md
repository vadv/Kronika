# Demo image design

## What this is

DESIGN.md's "Demo" section says: "The repository demo runs the project against
live PostgreSQL and OS containers. The demo reports segment size, RSS, and CPU
use. It also supplies data for segment-size benchmarks." Today `kronika-demo`
runs the collector for a bounded local window with no database attached. This
change adds a single Docker image that runs PostgreSQL, PgBouncer, the
collector, and web together, with `kronika-demo` orchestrating a realistic
PostgreSQL workload so the demo shows populated dashboards, not an empty one.

## Scope

One image, one container, plain-bash `entrypoint.sh` as PID 1 supervising four
processes (PostgreSQL, PgBouncer, `kronika-web`, `kronika-demo`). No
supervisord, no s6-overlay, no docker-compose: a second orchestration layer
would be a second entity for a job a 60-line script already does, and the
project's own BDD image proves the pattern works.

## PostgreSQL and PgBouncer setup

Same pattern as `crates/kronika-bdd/src/services.rs`, run once at container
start instead of per-scenario:

- PostgreSQL: `initdb --auth=trust`, `logging_collector = on`,
  `log_destination = 'stderr'`, `log_checkpoints/log_lock_waits = on`,
  `log_min_duration_statement = 0`, started via `pg_ctl` as the `postgres`
  user. Data directory `/var/lib/kronika/pgdata`, reset on every container
  start (not persisted; only collected segments are).
- PgBouncer: `pool_mode = transaction`, `stats_users = postgres`,
  `logfile = /var/lib/kronika/pgbouncer/pgbouncer.log`, listening on 6432.
  `default_pool_size` and `max_client_conn` sized to comfortably hold the
  workload's steady sessions plus concurrent lock chains plus DDL fan-out
  (see below), with headroom.
- Entrypoint waits for each server's readiness (`psql ... show version`, as
  `services.rs` does) before starting anything downstream.

## Paths, env, and mounting prior data

- `KRONIKA_OUT_DIR=/var/lib/kronika/data`, declared as a Docker `VOLUME`.
  Mounting a host directory containing `YYYY/MM/DD/*.zms` (and optionally
  `.idx`) there makes it visible to web immediately: finished segments are
  self-contained and `.idx` files are derived and safely rebuilt, so no import
  step or new code is needed — only the mount point and a line of
  documentation. A foreign `active.wal` left in a mounted directory is
  recovered by the collector exactly as any interrupted local journal would
  be; this is existing startup-recovery behavior, not a demo special case.
- Collector: `KRONIKA_PG_DSNS` points at PostgreSQL directly (metrics and log
  discovery); `KRONIKA_PGBOUNCER_DSNS` points at PgBouncer's admin console
  only (`SHOW CONFIG` for its `logfile`, never for metrics — transaction
  pooling does not preserve session state). `KRONIKA_POSTGRES_EFFECTIVE_CPUS`
  is set from `nproc` in the entrypoint, since collector and monitored
  PostgreSQL share the same container/CPU quota.
- Web: same `KRONIKA_OUT_DIR`, `KRONIKA_WEB_LISTEN=0.0.0.0:8080` (published),
  `KRONIKA_WEB_USER`/`KRONIKA_WEB_PASSWORD` default to `demo`/`demo`,
  overridable via `docker run -e`. `KRONIKA_WEB_SOURCES=3` (os|postgresql).

## Workload generator (part of `kronika-demo`)

`kronika-demo`'s job stays "run the collector, report what it cost"; the
workload generator is a new concern it drives alongside the collector when
`KRONIKA_DEMO_WORKLOAD_DSN` is set (unset: today's behavior, unchanged). This
also requires `KRONIKA_DEMO_DURATION_S=0` to mean "run until `SIGTERM`"
instead of an already-elapsed deadline — a small change to the existing wait
loop in `main.rs`.

New modules under `bins/kronika-demo/src/workload/`:

- `schema.rs` — creates `KRONIKA_DEMO_WORKLOAD_SCHEMAS` schemas (default 5) ×
  `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` tables (default 400; ~2,000
  tables total), using `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` (default 16)
  parallel connections. Tables rotate through a handful of distinct column
  shapes (orders-like, events-like, jsonb-profile-like, wide-numeric) so
  `pg_stat_statements` and the system tables view see genuinely different
  shapes, not one shape copied 2,000 times. Logs progress every ~500 tables.
- `dml.rs` — `KRONIKA_DEMO_WORKLOAD_SESSIONS` (default 8) long-lived sessions
  through PgBouncer running randomized insert/update/select/delete with
  jitter; periodic deliberately slow query (crosses the 5s known-bad
  boundary) and deliberately bad statements (constraint violation, syntax
  error, connect to a nonexistent database through PgBouncer) so several
  `pg_log_errors` categories and `pgbouncer_events` are populated, not just
  the happy path.
- `locks.rs` — `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` (default 4) concurrent
  independent chains of `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` (default 6)
  transactions, each issuing the same `UPDATE ... WHERE id = <chain key>` in
  sequence. PostgreSQL's FIFO row-lock queue makes this a real, not
  simulated, wait chain: each transaction holds its lock for
  `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` (default 3000) before committing, so a
  depth-6 chain runs for ~18s — comfortably inside the 30s PostgreSQL
  collection interval, so `pg_locks`/`pg_stat_activity` reliably catch live
  waiters and `pg_log_lock_waits` records real events. A new round starts
  every `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` (default 45).

Shutdown: the same `SIGTERM` handling in `main.rs` that stops the collector
also flips a shared cancellation flag the workload tasks observe; each
finishes its current operation and returns. No retry or restart logic
anywhere in the workload — a failed statement is logged and the loop moves
on, matching the project's "be dumber" stance for a demo-only component.

## Dockerfile.demo, entrypoint, build tooling

- `Dockerfile.demo`: two-stage, mirrors `Dockerfile.bdd`. `deps` stage installs
  `postgresql pgbouncer` and builds the dependency graph from manifests only
  (cached on the lockfile). Final stage builds
  `-p kronika-collector -p kronika-web -p kronika-demo --release`, copies
  `scripts/demo-entrypoint.sh`, declares `VOLUME /var/lib/kronika/data`,
  `EXPOSE 8080`, `ENTRYPOINT ["/work/scripts/demo-entrypoint.sh"]`.
- `scripts/demo-entrypoint.sh` (`set -euo pipefail`, PID 1): start PostgreSQL,
  start PgBouncer, wait for both, export the env above, start `kronika-web` in
  the background, run `kronika-demo` (which owns the collector subprocess and
  the workload). `trap` on `SIGTERM`/`SIGINT` forwards to `kronika-demo` (which
  already stops the collector and writes the final `report.json`), then stops
  PostgreSQL. `wait -n` on the supervised processes: if PostgreSQL, PgBouncer,
  web, or `kronika-demo` dies, log which one and exit non-zero — no restarts.
- `scripts/demo-image.sh`: `build|run|deps-key`, mirrors `bdd-image.sh`, adds
  `-p 8080:8080` and an optional `-v <host>:/var/lib/kronika/data`.
- `Makefile`: `demo-image` (build) and `demo-image-run` (build and run with
  the port and optional volume) targets, alongside the existing `test-bdd`
  pattern.

## Verifying the plan before implementation

Not product test code — a manual check that the design holds together before
writing BDD/unit tests as part of implementation: build the image, wait for
readiness, `curl /api/catalog` with basic auth and confirm `postgresql`
sections and `pgbouncer_events` appear; separately, mount a directory
containing `.zms` files from a prior run and confirm `/api/catalog` shows them
immediately with no restart or import step.
