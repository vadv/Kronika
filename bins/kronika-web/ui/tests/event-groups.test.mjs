import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const events = await importModule('export { groupEvents, MINUTE_COLUMNS } from "../src/events-groups.ts"')

const HOUR = 1_780_000_000_000_000

function row(section, typeId, ordinal, minute, values) {
  return {
    segmentId: "7",
    logicalName: section,
    typeId,
    ordinal: String(ordinal),
    timestamp: HOUR + minute * 60_000_000,
    values,
  }
}

const error = (ordinal, minute, values) => row("pg_log_errors", "2001001", ordinal, minute, values)
const lockWait = (ordinal, minute, values) => row("pg_log_lock_waits", "2005002", ordinal, minute, values)
const checkpoint = (ordinal, minute, values) => row("pg_log_checkpoints", "2002001", ordinal, minute, values)

test("error windows with one pattern merge into one entry summing counts", () => {
  const entries = events.groupEvents({
    pg_log_errors: [
      error(0, 2, { severity: 0, category: 1, sqlstate: "23505", pattern: "duplicate key", count: 3, sample: "duplicate key value", database: "shop" }),
      error(1, 30, { severity: 0, category: 1, sqlstate: "23505", pattern: "duplicate key", count: 5, sample: "duplicate key value", database: "shop" }),
      error(2, 30, { severity: 1, category: 6, sqlstate: null, pattern: "terminating connection", count: 1, sample: "terminating connection" }),
    ],
  }, HOUR)

  assert.equal(entries.length, 2)
  const [fatal, duplicate] = entries
  assert.equal(fatal.tier, "critical")
  assert.equal(duplicate.tier, "notable")
  assert.equal(duplicate.count, 8)
  assert.equal(duplicate.text, "duplicate key")
  assert.equal(duplicate.stat.sqlstate, "23505")
  assert.equal(duplicate.stat.database, "shop")
  assert.equal(duplicate.minutes[2], 3)
  assert.equal(duplicate.minutes[30], 5)
  assert.equal(duplicate.firstTs, HOUR + 2 * 60_000_000)
  assert.equal(duplicate.rows.length, 2)
})

test("critical entries sort before notable ones regardless of count", () => {
  const entries = events.groupEvents({
    pg_log_errors: [
      error(0, 1, { severity: 0, category: 9, pattern: "syntax error", count: 500, sample: "syntax error" }),
      error(1, 2, { severity: 2, category: 5, pattern: "invalid page", count: 1, sample: "invalid page" }),
    ],
  }, HOUR)

  assert.equal(entries[0].text, "invalid page")
  assert.equal(entries[0].tier, "critical")
})

test("lock waits group into one episode per recorded holder list", () => {
  const entries = events.groupEvents({
    pg_log_lock_waits: [
      lockWait(0, 10, { kind: 0, pid: 2078, lock_mode: "ShareLock", lock_target: "transaction 987", duration_ms: 1000.1, holding_pids: "583", statement: "update orders" }),
      lockWait(1, 10, { kind: 0, pid: 456, lock_mode: "ShareLock", lock_target: "transaction 987", duration_ms: 1000.2, holding_pids: "583", statement: "delete from orders" }),
      lockWait(2, 11, { kind: 1, pid: 2078, lock_mode: "ShareLock", lock_target: "transaction 987", duration_ms: 40_000, holding_pids: "583" }),
      lockWait(3, 40, { kind: 0, pid: 900, lock_mode: "AccessExclusiveLock", lock_target: "relation 16384 of database 5", duration_ms: 1000.0, holding_pids: "77, 78" }),
    ],
  }, HOUR)

  assert.equal(entries.length, 2)
  const episode = entries.find((entry) => entry.stat.holders === "583")
  assert.equal(episode.stat.waiters, 2)
  assert.equal(episode.stat.maxMs, 40_000)
  assert.deepEqual(episode.stat.targets, ["transaction 987"])
  assert.equal(episode.count, 2)
  const composite = entries.find((entry) => entry.stat.holders === "77, 78")
  assert.equal(composite.stat.waiters, 1)
})

test("checkpoints collapse into one entry with the trigger mix and a separate too-frequent warning", () => {
  const entries = events.groupEvents({
    pg_log_checkpoints: [
      checkpoint(0, 0, { phase: 0, reason: "time" }),
      checkpoint(1, 1, { phase: 1, buffers_written: 100, sync_ms: 11.0 }),
      checkpoint(2, 20, { phase: 0, reason: "wal" }),
      checkpoint(3, 21, { phase: 1, buffers_written: 900, sync_ms: 2_100.0 }),
      checkpoint(4, 21, { phase: 2, seconds_apart: 18 }),
    ],
  }, HOUR)

  assert.equal(entries.length, 2)
  const [warning, summary] = entries
  assert.equal(warning.tier, "notable")
  assert.equal(warning.stat.kind, "pg.checkpoint_warning")
  assert.equal(warning.stat.secondsApart, 18)
  assert.equal(summary.tier, "routine")
  assert.equal(summary.stat.timed, 1)
  assert.equal(summary.stat.requested, 1)
  assert.equal(summary.stat.completes, 2)
  assert.equal(summary.stat.buffers, 1000)
  assert.equal(summary.stat.maxSyncMs, 2100)
})

test("a lifecycle record never groups and a crash is critical", () => {
  const entries = events.groupEvents({
    pg_log_lifecycle: [
      row("pg_log_lifecycle", "2006001", 0, 5, { kind: 0, pid: 4242, signal: 9, message: "server process (PID 4242) was terminated by signal 9" }),
      row("pg_log_lifecycle", "2006001", 1, 6, { kind: 2, message: "database system is ready to accept connections" }),
      row("pg_log_lifecycle", "2006001", 2, 7, { kind: 2, message: "database system is ready to accept connections" }),
    ],
  }, HOUR)

  assert.equal(entries.length, 3)
  assert.equal(entries[0].tier, "critical")
  assert.equal(entries[0].stat.signal, 9)
})

test("pgbouncer events group by level and message and slow queries keep the slowest sample", () => {
  const entries = events.groupEvents({
    pgbouncer_events: [
      row("pgbouncer_events", "2100001", 0, 1, { level: 2, text: "server login failed", database: "app" }),
      row("pgbouncer_events", "2100001", 1, 2, { level: 2, text: "server login failed", database: "app" }),
    ],
    pg_log_slow_queries: [
      row("pg_log_slow_queries", "2004001", 0, 3, { pattern: "select ?", sample: "select 1", count: 2, max_duration_ms: 100, total_duration_ms: 150 }),
      row("pg_log_slow_queries", "2004001", 1, 9, { pattern: "select ?", sample: "select 2", count: 1, max_duration_ms: 900, total_duration_ms: 900 }),
    ],
  }, HOUR)

  const pool = entries.find((entry) => entry.section === "pgbouncer_events")
  assert.equal(pool.count, 2)
  assert.equal(pool.stat.database, "app")
  const slow = entries.find((entry) => entry.section === "pg_log_slow_queries")
  assert.equal(slow.count, 3)
  assert.equal(slow.text, "select 2")
  assert.equal(slow.stat.maxMs, 900)
  assert.equal(slow.stat.totalMs, 1050)
})

test("the minute strip spans the hour", () => {
  const entries = events.groupEvents({
    pg_log_errors: [error(0, 59, { severity: 4, category: 10, pattern: "x", count: 2, sample: "x" })],
  }, HOUR)
  assert.equal(entries[0].minutes.length, events.MINUTE_COLUMNS)
  assert.equal(entries[0].minutes[59], 2)
})
