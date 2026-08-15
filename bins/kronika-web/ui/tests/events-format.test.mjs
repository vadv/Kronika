import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const events = await importModule(
  'export { eventValue, formatMetric } from "../src/events-view.tsx"; export { findingHistoryRequest, findingMetric } from "../src/finding-presentation.ts"',
  { plugins: [registryPlugin([{
    typeId: "1100001", logicalName: "os_process", identity: ["pid"],
    columns: ["ts", "pid", "starttime", "read_bytes"],
  }])] },
)
const t = (key) => ({ "unit.ms": " ms", "unit.per_call": "/call", "unit.per_second": "/s" })[key] ?? key
const finding = { logicalName: "pg_log_slow_queries" }

test("event metrics use semantic precision for percent, rate, duration, zero and null", () => {
  assert.equal(events.formatMetric(41.729068244136855, "percent", "en", t), "41.7%")
  assert.equal(events.formatMetric(41.729068244136855, "percent", "ru", t), "41,7 %")
  assert.equal(events.formatMetric(4e-7, "percent", "en", t), "<0.1%")
  assert.equal(events.formatMetric(0, "milliseconds", "en", t), "0 ms")
  assert.equal(events.formatMetric(0.004, "milliseconds_per_call", "en", t), "0.004 ms/call")
  assert.equal(events.formatMetric(0.49, "bytes_per_second", "en", t), "0.49 B/s")
  assert.equal(events.formatMetric(null, "count", "en", t), "—")
  const health = events.findingMetric({ fieldOrdinal: 1, kind: "known_bad", logicalName: "health", typeId: "0" }, t)
  assert.equal(health.unit, "percent")
  assert.equal(events.formatMetric(0.099, health.unit, "en", t), "<0.1%")
})

test("event identity fields remain exact while ordinary readings stay bounded", () => {
  assert.equal(events.eventValue(finding, "queryid", "9007199254740993", "en", t), "9007199254740993")
  assert.equal(events.eventValue(finding, "pid", "001234", "en", t), "001234")
  assert.equal(events.eventValue(finding, "latency_ms", 41.729068244136855, "en", t), "41.7 ms")
  assert.equal(events.eventValue(finding, "tiny", 4e-7, "en", t), "4E-7")
})

test("process finding history uses PID without the recorded start timestamp", () => {
  const request = events.findingHistoryRequest({
    category: null, fieldOrdinal: 3, kind: "spike", logicalName: "os_process",
    rowOrdinal: "7", segmentId: "segment-a", timestamp: 100, typeId: "1100001",
  }, {
    logicalName: "os_process", ordinal: "7", segmentId: "segment-a", timestamp: 100,
    typeId: "1100001", values: { pid: 41, starttime: "9007199254740997", read_bytes: 12 },
  })
  assert.deepEqual(request, { fields: ["pid", "read_bytes"], where: { pid: "41" } })
})
