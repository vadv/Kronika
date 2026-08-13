import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { availableUseChartKeys, reading } = await importModule('export { availableUseChartKeys, reading } from "../src/use-table.tsx"')

test("a resource reading carries the unit of what it measures", () => {
  assert.equal(reading(61.06, "en", "share", "/s"), "61.1%")
  assert.equal(reading(3_355_443, "en", "bytes", "/s"), "3.2 MiB/s")
  assert.equal(reading(0.02, "en", "count", "/s"), "0.02")
  assert.equal(reading(1_400, "en", "rate", "/s"), "1.4K/s")
  assert.equal(reading(21_471, "ru", "rate", "/с"), "21,5 тыс./с")
})

test("the resource table offers exact numeric histories and combines compatible network lanes", async () => {
  const lane = (name, timestamp, value) => ({ lane: name, logicalName: "x", segmentId: "a", timestamp, typeId: "1", value })
  const points = [
    lane("cpu_busy", 1, 0),
    lane("cpu_busy", 2, null),
    lane("net_rx", 1, 10),
    lane("net_tx", 1, 20),
    lane("net_drop", 1, null),
  ]
  assert.deepEqual(availableUseChartKeys(points), ["cpu-utilisation", "network-utilisation"])
  const source = await readFile(new URL("../src/use-table.tsx", import.meta.url), "utf8")
  assert.equal(source.match(/<SeriesChart/g)?.length, 1)
  assert.match(source, /second=\{selected\.second\.length/)
  assert.match(source, /aria-pressed=\{selected\?\.key === choice\.key\}/)
})
