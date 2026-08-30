import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const events = await importModule(
  'export { formatMetric } from "../src/events-format.ts"; export { findingMetric } from "../src/finding-presentation.ts"',
  { plugins: [registryPlugin([{
    typeId: "1100001", logicalName: "os_process", identity: ["pid"],
    columns: ["ts", "pid", "starttime", "read_bytes"],
  }, {
    typeId: "1005004", logicalName: "pg_stat_database", identity: ["datid"],
    // Ordinals are the contract: 16 deadlocks, 20/21 the wraparound ages,
    // 25 checksum_failures, 32/33 the session counters.
    columns: [...Array.from({ length: 16 }, (_, i) => `pad${i}`), "deadlocks",
      "pad17", "pad18", "pad19", "frozen_xid_age", "min_mxid_age",
      "pad22", "pad23", "pad24", "checksum_failures",
      ...Array.from({ length: 6 }, (_, i) => `pad${26 + i}`), "sessions_fatal", "sessions_killed"],
  }])] },
)
const t = (key) => ({ "unit.ms": " ms", "unit.per_call": "/call", "unit.per_second": "/s" })[key] ?? key

test("event metrics use semantic precision for percent, rate, duration, zero and null", () => {
  assert.equal(events.formatMetric(41.729068244136855, "percent", "en", t), "41.7%")
  assert.equal(events.formatMetric(41.729068244136855, "percent", "ru", t), "41,7 %")
  assert.equal(events.formatMetric(4e-7, "percent", "en", t), "<0.1%")
  assert.equal(events.formatMetric(850, "milliseconds", "en", t), "850 ms")
  assert.equal(events.formatMetric(850, "milliseconds", "ru", t), "850 мс")
  assert.equal(events.formatMetric(6_290, "milliseconds", "en", t), "6.29 s")
  assert.equal(events.formatMetric(6_290, "milliseconds", "ru", t), "6,29 с")
  assert.equal(events.formatMetric(90_500, "milliseconds", "en", t), "1.51 min")
  assert.equal(events.formatMetric(90_500, "milliseconds", "ru", t), "1,51 мин")
  assert.equal(events.formatMetric(0.004, "milliseconds_per_call", "en", t), "4 µs/call")
  assert.equal(events.formatMetric(0.49, "bytes_per_second", "en", t), "0.49 B/s")
  assert.equal(events.formatMetric(null, "count", "en", t), "—")
  const health = events.findingMetric({ fieldOrdinal: 1, kind: "known_bad", logicalName: "health", typeId: "0" }, t)
  assert.equal(health.unit, "percent")
  assert.equal(health.helpKey, "lane.health.overall_health.help")
  assert.equal(events.formatMetric(0.099, health.unit, "en", t), "<0.1%")
  const cpu = events.findingMetric({ fieldOrdinal: 0, kind: "known_bad", logicalName: "os_cpu", typeId: "1102001" }, t)
  assert.equal(cpu.helpKey, "system.metric.cpu_busy.help")
})

test("every known-bad boundary reaches Events with a named source and a stated boundary", () => {
  const known = (logicalName, typeId, fieldOrdinal) => events.findingMetric({ fieldOrdinal, kind: "known_bad", logicalName, typeId }, t)
  for (const [logicalName, typeId, fieldOrdinal, field, labelKey, helpKey, boundary] of [
    ["pg_stat_database", "1005004", 25, "checksum_failures", "pg.field.checksum_failures.label", "pg.field.checksum_failures.help", "events.boundary.increased"],
    ["pg_stat_database", "1005004", 32, "sessions_fatal", "pg.field.sessions_fatal.label", "pg.field.sessions_fatal.help", "events.boundary.increased"],
    ["pg_stat_database", "1005004", 33, "sessions_killed", "pg.field.sessions_killed.label", "pg.field.sessions_killed.help", "events.boundary.increased"],
    ["pg_stat_database", "1005004", 20, "frozen_xid_age", "pg.field.frozen_xid_age.label", "pg.field.frozen_xid_age.help", "events.boundary.wraparound"],
    ["pg_stat_database", "1005004", 21, "min_mxid_age", "pg.field.min_mxid_age.label", "pg.field.min_mxid_age.help", "events.boundary.wraparound"],
    ["pg_stat_archiver", "1008001", 4, "failed_count", "pg.field.failed_count.label", "pg.field.failed_count.help", "events.boundary.increased"],
    ["os_cgroup_memory", "1202002", 13, "oom_kill", null, null, "events.boundary.increased"],
    ["pg_locks", "1011002", 2, "blocked_by", "pg.field.blocked_by.label", "pg.field.blocked_by.help", "events.boundary.contention"],
    ["pg_log_errors", "2001001", 4, "category", "events.metric.data_corruption", "events.metric.data_corruption.help", "events.boundary.data_corruption"],
  ]) {
    const metric = known(logicalName, typeId, fieldOrdinal)
    assert.equal(metric.field, field, logicalName + " " + field)
    if (labelKey !== null) assert.equal(metric.labelKey, labelKey, logicalName + " label")
    if (helpKey !== null) assert.equal(metric.helpKey, helpKey, logicalName + " help")
    assert.equal(metric.boundary, boundary, logicalName + " boundary")
    assert.notEqual(metric.label, "events.metric.unavailable", logicalName + " label")
  }
})
