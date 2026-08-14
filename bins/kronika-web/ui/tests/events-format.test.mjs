import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const events = await importModule(
  'export { eventValue, formatMetric } from "../src/events-view.tsx"',
  { plugins: [registryPlugin([])] },
)
const t = (key) => ({ "unit.ms": " ms", "unit.per_call": "/call", "unit.per_second": "/s" })[key] ?? key
const finding = { logicalName: "pg_log_slow_queries" }

test("event metrics use semantic precision for percent, rate, duration, zero and null", () => {
  assert.equal(events.formatMetric(41.729068244136855, "percent", "en", t), "41.7%")
  assert.equal(events.formatMetric(41.729068244136855, "percent", "ru", t), "41,7 %")
  assert.equal(events.formatMetric(4e-7, "percent", "en", t), "4E-7%")
  assert.equal(events.formatMetric(0, "milliseconds", "en", t), "0 ms")
  assert.equal(events.formatMetric(0.004, "milliseconds_per_call", "en", t), "0.004 ms/call")
  assert.equal(events.formatMetric(0.49, "bytes_per_second", "en", t), "0.49 B/s")
  assert.equal(events.formatMetric(null, "count", "en", t), "—")
})

test("event identity fields remain exact while ordinary readings stay bounded", () => {
  assert.equal(events.eventValue(finding, "queryid", "9007199254740993", "en", t), "9007199254740993")
  assert.equal(events.eventValue(finding, "pid", "001234", "en", t), "001234")
  assert.equal(events.eventValue(finding, "latency_ms", 41.729068244136855, "en", t), "41.7 ms")
  assert.equal(events.eventValue(finding, "tiny", 4e-7, "en", t), "4E-7")
})
