import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const activity = await importModule(
  'export { cursorColumnOf, formatTopActivityValue, intervalInstant, rowPeakColumn } from "../src/activity.tsx"',
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

test("top activity values use the returned unit without local metadata", () => {
  assert.equal(activity.formatTopActivityValue(1024, "bytes", "en"), "1 KiB")
  assert.equal(activity.formatTopActivityValue(2, "count_per_second", "en", "/s"), "2/s")
  assert.equal(activity.formatTopActivityValue(2, "count", "en", "/s"), "2")
})

test("top activity formatting is independent of result load order", () => {
  const converted = { cell_unit: "bytes_per_second", total_unit: "bytes" }
  const raw = { cell_unit: "count_per_second", total_unit: "count" }
  const rendered = (definition) => ({
    cell: activity.formatTopActivityValue(2, definition.cell_unit, "en", "/s"),
    total: activity.formatTopActivityValue(2, definition.total_unit, "en", "/s"),
  })

  const convertedFirst = [rendered(converted), rendered(raw)]
  const rawFirst = [rendered(raw), rendered(converted)]
  assert.deepEqual(convertedFirst[0], rawFirst[1])
  assert.deepEqual(convertedFirst[1], rawFirst[0])
  assert.deepEqual(convertedFirst, [
    { cell: "2 B/s", total: "2 B" },
    { cell: "2/s", total: "2" },
  ])
})

test("the ledger reads units from the shared result instead of cursor-loaded metadata", async () => {
  const source = await readFile(new URL("../src/activity.tsx", import.meta.url), "utf8")
  assert.match(source, /view\.definition\.cell_unit/)
  assert.match(source, /view\.definition\.total_unit/)
  assert.doesNotMatch(source, /cutScale|ActivityScales|scales=/)
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
