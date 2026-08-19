import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { semanticValueTone } from "../src/value-tone.ts"

test("null and workload volume remain neutral while zero rates are inactive", () => {
  assert.equal(semanticValueTone("calls_per_second", null, true), null)
  assert.equal(semanticValueTone("calls_per_second", 0, true), "inactive")
  assert.equal(semanticValueTone("calls_per_second", 1_000_000, true), null)
  assert.equal(semanticValueTone("calls", 0), null)
})

test("statement duration marks only the inclusive five-second boundary", () => {
  assert.equal(semanticValueTone("mean_exec_ms_per_call", 4_999.999), null)
  assert.equal(semanticValueTone("mean_exec_ms_per_call", 5_000), "critical")
  assert.equal(semanticValueTone("mean_exec_time_ms", 4_999.99), null)
  assert.equal(semanticValueTone("mean_exec_time_ms", 5_000), "critical")
  assert.equal(semanticValueTone("query_duration_ms", 999.99), null)
  assert.equal(semanticValueTone("query_duration_ms", 1_000), "warning")
  assert.equal(semanticValueTone("query_duration_ms", 4_999.99), "warning")
  assert.equal(semanticValueTone("query_duration_ms", 5_000), "critical")
})

test("Activity marks long transactions and visible wait states", () => {
  assert.equal(semanticValueTone("transaction_duration_ms", 4_999), null)
  assert.equal(semanticValueTone("transaction_duration_ms", 5_000), "warning")
  assert.equal(semanticValueTone("transaction_duration_ms", 60_000), "critical")
  assert.equal(semanticValueTone("state", "idle"), null)
  assert.equal(semanticValueTone("state", "idle in transaction"), "warning")
  assert.equal(semanticValueTone("state", "idle in transaction (aborted)"), "critical")
  assert.equal(semanticValueTone("wait_event_type", null), null)
  assert.equal(semanticValueTone("wait_event_type", "Lock"), "warning")
})

test("Activity duration tones belong only to active client backends", () => {
  const activity = (backend_type: string, state = "active") => ({
    logicalName: "pg_stat_activity",
    ordinal: "0",
    segmentId: "a",
    timestamp: 10_000_000,
    typeId: "1001004",
    values: { backend_type, state },
  })
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("client backend")), "critical")
  assert.equal(semanticValueTone("transaction_duration_ms", 60_000, false, activity("client backend")), "critical")
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("walsender")), null)
  assert.equal(semanticValueTone("transaction_duration_ms", 60_000, false, activity("walsender")), null)
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("checkpointer")), null)
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("client backend", "idle in transaction")), null)
})

test("cache hit tones distinguish no accesses from a real zero-percent hit rate", () => {
  assert.equal(semanticValueTone("hit_pct", null), null)
  assert.equal(semanticValueTone("hit_pct", 0), "critical")
  assert.equal(semanticValueTone("hit_pct", 89.999), "critical")
  assert.equal(semanticValueTone("hit_pct", 90), "warning")
  assert.equal(semanticValueTone("hit_pct", 98.999), "warning")
  assert.equal(semanticValueTone("hit_pct", 99), "good")
})

test("statement stability and planning use exact inclusive boundaries", () => {
  assert.equal(semanticValueTone("cv", 0.999), "good")
  assert.equal(semanticValueTone("cv", 1), "warning")
  assert.equal(semanticValueTone("cv", 3), "critical")
  assert.equal(semanticValueTone("plan_time_pct", 49.999), "good")
  assert.equal(semanticValueTone("plan_time_pct", 50), "warning")
  assert.equal(semanticValueTone("plan_time_pct", 80), "critical")
})

test("workload volume and identifiers stay neutral", () => {
  assert.equal(semanticValueTone("execution_ms_per_second", 10_000), null)
  assert.equal(semanticValueTone("rows_per_second", 86_400_000), null)
  assert.equal(semanticValueTone("planid", 4), null)
})

test("semantic tones coexist with exact locator classes", async () => {
  const table = await readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8")
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
  assert.match(table, /data-value-tone=/)
  assert.match(table, /data-locator-cell=/)
  assert.match(table, /value-tone-\$\{tone\}.*locator-cell/)
  assert.match(table, /aria-label=\{toneText === null \|\| field === undefined \? undefined : `\$\{toneText\}: \$\{cellAriaValue\(stored, field, locale, t\)\}`\}/)
  assert.doesNotMatch(table, /\$\{toneText\}: \$\{rawText\(stored\)/)
  assert.match(table, /critical: "\[&_\.entity-value\]:font-\[700\] \[&_\.entity-value\]:text-bad"/)
  assert.match(table, /border-l-2 border-current/)
  assert.doesNotMatch(table, /value-tone-critical[^"]*box-shadow/)
  assert.match(table, /known_bad: "[^"]*\[&_\.entity-value\]:text-fg-hi"/)
})
