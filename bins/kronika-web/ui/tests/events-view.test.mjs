import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const helpers = await importModule('export { locatorText } from "../src/events-view.tsx"', {
  plugins: [registryPlugin([{
    typeId: "2001001", logicalName: "pg_log_errors", identity: [],
    columns: ["ts", "severity", "category", "message"],
  }])],
})

function finding(rowOrdinal, fieldOrdinal) {
  return {
    category: null,
    fieldOrdinal,
    kind: "event",
    logicalName: "pg_log_errors",
    rowOrdinal,
    segmentId: "1786376240918529",
    timestamp: 1786376245000000,
    typeId: "2001001",
  }
}

test("expanded marker rows expose every exact locator coordinate", () => {
  const translate = (_key, slots) => `segment ${slots.segment} · type ${slots.type} · row ${slots.row} · field ${slots.field}`
  const locators = [finding("10", 0), finding("20", 0), finding("20", 1)].map((item) => helpers.locatorText(item, translate))
  assert.deepEqual(locators, [
    "segment 1786376240918529 · type 2001001 · row 10 · field 0",
    "segment 1786376240918529 · type 2001001 · row 20 · field 0",
    "segment 1786376240918529 · type 2001001 · row 20 · field 1",
  ])
  assert.equal(new Set(locators).size, locators.length)
})
