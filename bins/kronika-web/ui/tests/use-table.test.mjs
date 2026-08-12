import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { reading } = await importModule('export { reading } from "../src/use-table.tsx"')

test("a resource reading carries the unit of what it measures", () => {
  assert.equal(reading(61.06, "en", "share", "/s"), "61.06%")
  assert.equal(reading(3_355_443, "en", "bytes", "/s"), "3.2 MiB/s")
  assert.equal(reading(0.02, "en", "count", "/s"), "0.02")
  assert.equal(reading(1_400, "en", "rate", "/s"), "1,400/s")
  assert.equal(reading(21_471, "ru", "rate", "/с"), "21,5k/с")
})
