import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const events = await importModule(
  'export { eventValue, formatMetric } from "../src/events-format.ts"; export { findingMetric } from "../src/finding-presentation.ts"',
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
const finding = { logicalName: "pg_log_slow_queries" }

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
  assert.equal(events.formatMetric(0.099, health.unit, "en", t), "<0.1%")
})

test("event identity fields remain exact while ordinary readings stay bounded", () => {
  assert.equal(events.eventValue(finding, "queryid", "9007199254740993", "en", t), "9007199254740993")
  assert.equal(events.eventValue(finding, "pid", "001234", "en", t), "001234")
  assert.equal(events.eventValue(finding, "write_ms", 850, "en", t), "850 ms")
  assert.equal(events.eventValue(finding, "write_ms", 850, "ru", t), "850 мс")
  assert.equal(events.eventValue(finding, "max_duration_ms", 6_290, "en", t), "6.29 s")
  assert.equal(events.eventValue(finding, "max_duration_ms", 6_290, "ru", t), "6,29 с")
  assert.equal(events.eventValue(finding, "elapsed_ms", 90_500, "en", t), "1.51 min")
  assert.equal(events.eventValue(finding, "elapsed_ms", 90_500, "ru", t), "1,51 мин")
  assert.equal(events.eventValue(finding, "tiny", 4e-7, "en", t), "4E-7")
})

test("every known-bad boundary reaches Events with a named source and a stated boundary", () => {
  const known = (logicalName, typeId, fieldOrdinal) => events.findingMetric({ fieldOrdinal, kind: "known_bad", logicalName, typeId }, t)
  for (const [logicalName, typeId, fieldOrdinal, field, boundary] of [
    ["pg_stat_database", "1005004", 25, "checksum_failures", "events.boundary.increased"],
    ["pg_stat_database", "1005004", 32, "sessions_fatal", "events.boundary.increased"],
    ["pg_stat_database", "1005004", 33, "sessions_killed", "events.boundary.increased"],
    ["pg_stat_database", "1005004", 20, "frozen_xid_age", "events.boundary.wraparound"],
    ["pg_stat_database", "1005004", 21, "min_mxid_age", "events.boundary.wraparound"],
    ["pg_stat_archiver", "1008001", 4, "failed_count", "events.boundary.increased"],
    ["os_cgroup_memory", "1202002", 13, "oom_kill", "events.boundary.increased"],
    ["pg_locks", "1011002", 2, "blocked_by", "events.boundary.contention"],
    ["pg_log_errors", "2001001", 4, "category", "events.boundary.data_corruption"],
  ]) {
    const metric = known(logicalName, typeId, fieldOrdinal)
    assert.equal(metric.field, field, logicalName + " " + field)
    assert.equal(metric.boundary, boundary, logicalName + " boundary")
    assert.notEqual(metric.label, "events.metric.unavailable", logicalName + " label")
  }
})
