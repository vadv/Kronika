import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const activity = await importModule(
  'export { cursorColumnOf, intervalInstant, rowPeakColumn } from "../src/activity.tsx"',
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

test("ranked statement and plan rows use compact IDs instead of stored text labels", async () => {
  const source = await readFile(new URL("../src/activity.tsx", import.meta.url), "utf8")
  assert.match(source, /text: `Query ID \$\{queryId \?\? "—"\}`/)
  assert.match(source, /text: `Plan ID \$\{planId \?\? "—"\}`/)
  assert.doesNotMatch(source, /labelText\(row, "(?:query|plan)"\)|activityPreview|statementTextsByQueryId|planTextsByPlanId/)
})
