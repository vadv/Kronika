# Demo Showcase Workload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace permanent and noisy demo anomalies with real, bounded, repeating scenarios that populate every current Kronika surface.

**Architecture:** Keep ordinary DML as the steady baseline. Run locks, slow/error events, and Vacuum in independent stop-aware loops with server-side duration bounds; the first episode of each loop starts immediately and later episodes are separated by quiet intervals.

**Tech Stack:** Rust 2024, Tokio, tokio-postgres, PostgreSQL 15, PgBouncer, Docker Compose, React UI verification.

**Spec:** `docs/superpowers/specs/2026-08-22-demo-showcase-workload-design.md`

## Global Constraints

- Generate only real PostgreSQL/PgBouncer/OS activity; never write synthetic Kronika rows.
- No transaction or statement disables its timeout.
- PostgreSQL collection cadence is five seconds; a lock or Vacuum episode must remain observable longer than one cadence.
- The demo container remains within two CPUs, 1 GiB memory, 512 MiB PostgreSQL tmpfs, and 512 MiB Kronika retention.
- English logs and comments; English and Russian user documentation stay equivalent.

---

### Task 1: Periodic lock episodes

**Files:**
- Modify: `bins/kronika-demo/src/workload/locks.rs`
- Create: `bins/kronika-demo/src/workload/locks/tests.rs`
- Modify: `bins/kronika-demo/src/workload/mod.rs`
- Modify: `bins/kronika-demo/src/workload/tests.rs`

**Interfaces:**
- Produces: `periodic_chain_keys(chains: u32) -> Range<u32>` used by every lock round.
- Produces: defaults `chains=1`, `depth=4`, `hold_ms=4000`, `round_interval_s=45`; the final waiter reaches a finite 10-second statement timeout.

- [ ] Add `all_chains_run_in_periodic_rounds`, asserting keys `[0]` and `[0, 1]`.
- [ ] Run `cargo test -p kronika-demo workload::locks::tests::all_chains_run_in_periodic_rounds` and confirm RED because `periodic_chain_keys` is absent.
- [ ] Implement `periodic_chain_keys`, use it in `run_one_round`, and remove `hold_continuous_chain` plus all timeout-zero SQL.
- [ ] Change lock defaults and the test fixture to `1`, `4`, `4000`, `45`, with a 10-second timeout on each lock update.
- [ ] Run `cargo test -p kronika-demo workload::locks::tests::all_chains_run_in_periodic_rounds` and the full `cargo test -p kronika-demo`.

### Task 2: Separate rare events from steady DML

**Files:**
- Modify: `bins/kronika-demo/src/workload/dml.rs`
- Modify: `bins/kronika-demo/src/workload/dml/tests.rs`
- Create: `bins/kronika-demo/src/workload/events.rs`
- Create: `bins/kronika-demo/src/workload/events/tests.rs`
- Modify: `bins/kronika-demo/src/workload/mod.rs`
- Modify: `bins/kronika-demo/src/workload/tests.rs`

**Interfaces:**
- Produces: `dml::perform(client, config, table, action, id)` callable by the event loop.
- Produces: `events::episode_actions() -> [Action; 3]` in the order SlowQuery, BadStatement, BadDatabase.
- Consumes: `WorkloadConfig.event_round_interval_s`, default 60 seconds.

- [ ] Change the DML test to assert `next_action` returns only Insert, Update, Select, or Delete for every roll `0..100`; run it and confirm RED on roll 96.
- [ ] Add an events test asserting the exact three-action episode; run it and confirm RED because the module/API is absent.
- [ ] Restrict `next_action` to ordinary DML and expose `perform` to its sibling module.
- [ ] Implement `events::run_rounds`: connect once, execute the three actions against a known table, then wait `event_round_interval_s` while observing stop.
- [ ] Add, parse, validate, debug-print, and test `KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S=60`; spawn the event loop alongside DML and locks.
- [ ] Run both focused tests and `cargo test -p kronika-demo`.

### Task 3: Add bounded Vacuum episodes

**Files:**
- Create: `bins/kronika-demo/src/workload/vacuum.rs`
- Create: `bins/kronika-demo/src/workload/vacuum/tests.rs`
- Modify: `bins/kronika-demo/src/workload/mod.rs`
- Modify: `bins/kronika-demo/src/workload/tests.rs`

**Interfaces:**
- Produces: `vacuum::setup_sql(rows: u32) -> String` for `tenant_0.vacuum_showcase`.
- Produces: `vacuum::run_sql(timeout_s: u64) -> Vec<String>` with finite statement timeout, `vacuum_cost_delay=8ms`, `vacuum_cost_limit=200`, update, `VACUUM (ANALYZE)`, and a cleanup statement.
- Consumes: `vacuum_rows=100000`, `vacuum_round_interval_s=180`, `vacuum_statement_timeout_s=30`.

- [ ] Add SQL-boundary tests: setup uses the configured literal row bound and a 256-character payload; run SQL contains `statement_timeout = '30s'`, never contains timeout zero, updates the showcase table, and runs `VACUUM (ANALYZE)`.
- [ ] Run focused tests and confirm RED because `setup_sql` and `run_sql` are absent.
- [ ] Implement table setup and the first immediate Vacuum episode, then repeat after a stop-aware 180-second pause.
- [ ] Add, parse, validate, debug-print, and test the three Vacuum configuration values; spawn the loop independently so failure does not stop other scenarios.
- [ ] Run focused tests and `cargo test -p kronika-demo`.

### Task 4: Readiness and bilingual documentation

**Files:**
- Modify: `scripts/demo-healthcheck.sh`
- Modify: `bins/kronika-demo/README.md`
- Modify: `bins/kronika-demo/README.ru.md`

**Interfaces:**
- Consumes: historical catalog sections from the collector.
- Produces: health requires both `pg_locks` and `pg_stat_progress_vacuum` to have appeared at least once.

- [ ] Add `pg_stat_progress_vacuum` to the healthcheck section contract.
- [ ] Document the steady baseline, bounded scenario cadence, maximum transaction age, and all new environment variables in English and Russian.
- [ ] Run `shellcheck scripts/demo-healthcheck.sh` when available and `git diff --check`.

### Task 5: Full verification in Docker and browser

**Files:**
- Verify: `compose.demo.yml`, the built `kronika-demo:local` image, and browser UI.

**Interfaces:**
- Produces: a running healthy demo from this branch at a free loopback port.

- [ ] Run `cargo fmt --all --check`.
- [ ] Run `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- [ ] Run `cargo test --workspace --all-targets --all-features`.
- [ ] Run `make demo-up` and wait for health; inspect PostgreSQL live state to prove locks appear and later return to zero, with no transaction older than the scenario bound.
- [ ] Verify `pg_stat_progress_vacuum`, `pg_locks`, `pg_log_lock_waits`, `pg_stat_statements`, `pg_store_plans`, OS and PgBouncer sections in `/api/catalog`.
- [ ] In the browser verify Overview has no hour-long current transaction, Locks has filled and empty cursor moments, Vacuum has a real episode, and Events is episodic rather than continuously saturated.
- [ ] Record fresh command output and the final branch diff before reporting completion.
