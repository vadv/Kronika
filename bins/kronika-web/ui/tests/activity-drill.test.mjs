import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const activity = await importModule(
  'export { cursorColumnOf, intervalInstant, planTextsByPlanId, rowPeakColumn, statementTextsByQueryId } from "../src/activity.tsx"; export { activityPreview } from "../src/activity-cuts.ts"',
  { plugins: [registryPlugin([])] },
)

const HOUR = 1_000_000_000_000
const HOUR_MICROS = 3_600_000_000

test("a cell click and the drill land on the identical microsecond of a column", () => {
  // The last moment of the column, exactly as ActivityStrip.pick computes it.
  assert.equal(activity.intervalInstant(HOUR, 0, 12), HOUR + 300_000_000 - 1)
  assert.equal(activity.intervalInstant(HOUR, 11, 12), HOUR + HOUR_MICROS - 1)
  assert.equal(activity.intervalInstant(HOUR, 59, 60), HOUR + HOUR_MICROS - 1)
})

test("the cursor's column mirrors the strip and is null outside the hour", () => {
  assert.equal(activity.cursorColumnOf(HOUR, HOUR, 12), 0)
  assert.equal(activity.cursorColumnOf(HOUR + 300_000_000, HOUR, 12), 1)
  assert.equal(activity.cursorColumnOf(HOUR + HOUR_MICROS - 1, HOUR, 12), 11)
  assert.equal(activity.cursorColumnOf(HOUR - 1, HOUR, 12), null)
  assert.equal(activity.cursorColumnOf(HOUR + HOUR_MICROS, HOUR, 12), null)
})

test("a row's peak is its first strictly positive maximum, and a silent row has none", () => {
  assert.equal(activity.rowPeakColumn([null, 2, 7, null, 7, 1]), 2)
  assert.equal(activity.rowPeakColumn([0, 0.5, 0.5]), 1)
  assert.equal(activity.rowPeakColumn([null, null]), null)
  assert.equal(activity.rowPeakColumn([0, 0, 0]), null)
  assert.equal(activity.rowPeakColumn([]), null)
})

test("a drill moves the cursor only when the drilled row is silent at it", async () => {
  const source = await readFile(new URL("../src/activity.tsx", import.meta.url), "utf8")
  const choose = /const choose = drill === undefined \? undefined : \(row[\s\S]*?\n  \}/.exec(source)?.[0] ?? ""
  // Silent at the cursor (or the cursor outside the hour) -> jump to the
  // row's own peak, by the shared instant. Alive at the cursor -> stay.
  assert.match(choose, /cursorColumn === null \|\| \(row\.cells\[cursorColumn\] \?\? null\) === null/)
  assert.match(choose, /onCursor\(intervalInstant\(hour, peak, columns\)\)/)
  assert.match(choose, /rowPeakColumn\(row\.cells\)/)
  // The strip's own click uses the same instant, so the two gestures agree.
  assert.match(source, /onCursor\(intervalInstant\(hour, column, columns\)\)/)
})

test("ranked statement and plan previews use the first nonempty loaded table text", async () => {
  const row = (logicalName, ordinal, values) => ({ logicalName, ordinal, segmentId: "s", timestamp: HOUR, typeId: "t", values })
  const statements = activity.statementTextsByQueryId([
    row("pg_stat_statements", "1", { queryid: "101", query: " \n\t" }),
    row("pg_stat_statements", "2", { queryid: "101", query: " select  \n  one " }),
    row("pg_stat_statements", "3", { queryid: "101", query: "ignored later text" }),
    row("pg_stat_statements", "4", { queryid: "102", query: null }),
  ])
  const plans = activity.planTextsByPlanId([
    row("pg_store_plans", "1", { planid: 201, plan: "  Seq Scan\t on orders  " }),
    row("pg_store_plans", "2", { planid: 201, plan: "ignored later plan" }),
  ])
  assert.deepEqual([...statements], [["101", " select  \n  one "]])
  assert.deepEqual([...plans], [["201", "  Seq Scan\t on orders  "]])
  assert.equal(activity.activityPreview(statements.get("101")), "select one")
  assert.equal(activity.activityPreview(plans.get("201")), "Seq Scan on orders")
  assert.equal(activity.activityPreview(`select ${"x".repeat(300)}`).length, 240)

  const [source, view] = await Promise.all([
    readFile(new URL("../src/activity.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
  ])
  assert.match(source, /t\("pg\.detail\.query", \{ id: queryId \?\? "—" \}\)/)
  assert.match(source, /t\("pg\.detail\.plan", \{ id: planId \?\? "—" \}\)/)
  assert.doesNotMatch(source, /labelText\(row, "(?:query|plan)"\)|loadRelatedStatementTextRow|first_match/)
  assert.match(view, /<StatementsActivity[^>]+rows=\{data\.sections\.pg_stat_statements \?\? NO_ROWS\}/)
  assert.match(view, /<PlansActivity[^>]+rows=\{data\.sections\.pg_store_plans \?\? NO_ROWS\}/)
})
