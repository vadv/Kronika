# Demo Investigation Reel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the noisy synthetic workload with a bounded commerce investigation reel that visibly records lock contention, a plan regression, recovery, and Vacuum.

**Architecture:** Keep `kronika-demo` as the workload supervisor. Give schema setup, baseline traffic, locks, plans, events, and Vacuum focused modules; all connect through a shared helper that sets a truthful PostgreSQL `application_name`. The existing collector and web UI remain the recorded-data path.

**Tech Stack:** Rust 2024, Tokio, tokio-postgres, PostgreSQL 15, PgBouncer, Docker Compose, Kronika web UI.

**Spec:** `docs/superpowers/specs/2026-08-22-demo-investigation-reel-design.md`

## Global Constraints

- Every blocking, slow, DDL, and Vacuum statement has a finite server-side timeout.
- Every episode restores its owned database state before its quiet interval.
- Do not add automatic diagnosis, correlation, incident, or cause claims.
- Preserve collector-only behavior when `KRONIKA_DEMO_WORKLOAD_DSN` is unset.
- Keep the image inside its existing 1 GiB memory and 512 MiB PostgreSQL tmpfs limits.

---

### Task 1: Truthful commerce identities

**Files:**
- Modify: `bins/kronika-demo/src/workload/naming.rs`
- Modify: `bins/kronika-demo/src/workload/naming/tests.rs`
- Modify: `bins/kronika-demo/src/workload/schema.rs`
- Modify: `bins/kronika-demo/src/workload/schema/tests.rs`
- Modify: `bins/kronika-demo/src/workload/mod.rs`

**Interfaces:**
- Produces: stable `shop` relation names and `connect_as(dsn, application_name)`.

- [ ] Write failing tests for the `shop` schema, named commerce tables, and connection application names.
- [ ] Run `cargo test -p kronika-demo workload::naming -- --nocapture` and confirm the old tenant names fail.
- [ ] Implement named schema/table DDL and the named connection helper.
- [ ] Run the focused schema/naming tests and all `kronika-demo` tests.

### Task 2: Bounded plan-regression episode

**Files:**
- Create: `bins/kronika-demo/src/workload/plans.rs`
- Create: `bins/kronika-demo/src/workload/plans/tests.rs`
- Modify: `bins/kronika-demo/src/workload/mod.rs`
- Modify: `bins/kronika-demo/src/workload/tests.rs`

**Interfaces:**
- Consumes: `connect_as` and `shop.orders`.
- Produces: setup SQL, stable checkout query SQL, index removal/restoration SQL, and `run_rounds`.

- [ ] Write failing tests that require one stable query text, finite timeouts, index removal, index restoration, and a positive bounded episode duration.
- [ ] Run the focused test and confirm the plans module/config fields are absent.
- [ ] Implement idempotent seeded orders, concurrent slow-plan workers, schema restoration, and quiet cadence.
- [ ] Run the focused test and the whole `kronika-demo` package.

### Task 3: Named bounded lock and maintenance episodes

**Files:**
- Modify: `bins/kronika-demo/src/workload/locks.rs`
- Modify: `bins/kronika-demo/src/workload/locks/tests.rs`
- Modify: `bins/kronika-demo/src/workload/dml.rs`
- Modify: `bins/kronika-demo/src/workload/vacuum.rs`
- Modify: `bins/kronika-demo/src/workload/events.rs`

**Interfaces:**
- Consumes: `connect_as`, `shop.orders`, `shop.event_log`.
- Produces: user-visible application identities on every workload connection.

- [ ] Write failing tests for named root/waiter roles and named relations.
- [ ] Run the focused tests and confirm the old generic identities fail.
- [ ] Route each connection through its scenario-specific application name and preserve finite rollback/commit paths.
- [ ] Run all workload tests.

### Task 4: Demo defaults and documentation

**Files:**
- Modify: `bins/kronika-demo/README.md`
- Modify: `bins/kronika-demo/README.ru.md`
- Modify: `scripts/demo-entrypoint.sh`

**Interfaces:**
- Produces: a five-minute default cadence and documented investigation path.

- [ ] Update defaults to a small named schema and bounded episode cadence.
- [ ] Document the lock and plan before/during/after paths in both languages.
- [ ] Run `cargo fmt --all --check`, shell syntax checks, and `git diff --check`.

### Task 5: Build and inspect the real demo

**Files:**
- Verify only.

**Interfaces:**
- Consumes: the built demo image and existing Kronika APIs/UI.
- Produces: browser-visible data for every acceptance criterion.

- [ ] Run the full affected Rust test target and clippy.
- [ ] Build and start a clean Compose demo on the chosen loopback port.
- [ ] Query PostgreSQL to confirm bounded sessions and at least two plan IDs for the checkout query.
- [ ] Use the in-app browser to inspect Host, Processes, Activity, Locks, Statements, Plans, Tables, Vacuum, and Events.
- [ ] Record any missing acceptance result as a defect, fix it test-first, rebuild, and repeat.
