import assert from "node:assert/strict"
import test from "node:test"
import { Buffer } from "node:buffer"
import { fileURLToPath } from "node:url"

import { build } from "esbuild"

const entry = fileURLToPath(new URL("../src/detail.tsx", import.meta.url))
const result = await build({
  bundle: true,
  entryPoints: [entry],
  format: "esm",
  jsx: "automatic",
  logLevel: "silent",
  platform: "node",
  write: false,
})
const source = result.outputFiles[0]
assert.ok(source)
const detail = await import(`data:text/javascript;base64,${Buffer.from(source.text).toString("base64")}`)

function row(timestamp, values, segmentId = "segment-a", ordinal = "0") {
  return { segmentId, typeId: "1100001", ordinal, timestamp, values }
}

test("a counter is drawn as the rate between two readings", () => {
  const series = detail.processLensHistory([
    row(3_000_000, { pid: 77, starttime: 10, stime: 30 }, "segment-b", "2"),
    row(1_000_000, { pid: 77, starttime: 10, stime: 10 }, "segment-a", "0"),
    row(2_000_000, { pid: 77, starttime: 10, stime: 14 }, "segment-a", "1"),
  ], "cpu")

  assert.deepEqual(series.map((item) => item.field), [
    "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "minflt", "majflt",
  ])
  assert.deepEqual(series[1].points.map((point) => point.value), [null, 4, 16])
})

test("a missing counter reading resets the next rate", () => {
  const series = detail.processLensHistory([
    row(1_000_000, { pid: 77, starttime: 10, stime: 10 }, "segment-a", "0"),
    row(2_000_000, { pid: 77, starttime: 10, stime: null }, "segment-a", "1"),
    row(3_000_000, { pid: 77, starttime: 10, stime: 30 }, "segment-b", "2"),
    row(4_000_000, { pid: 77, starttime: 10, stime: 34 }, "segment-b", "3"),
  ], "cpu")

  assert.deepEqual(series[1].points.map((point) => point.value), [null, null, null, 4])
})

test("null values split rendered history runs while later zero stays numeric", () => {
  const [series] = detail.processLensHistory([
    row(10, { pid: 77, starttime: 10, rmem_kb: 1 }),
    row(20, { pid: 77, starttime: 10, rmem_kb: null }),
    row(30, { pid: 77, starttime: 10, rmem_kb: 0 }),
  ], "memory")

  assert.equal(series.points[0].segmentId, series.points[1].segmentId)
  assert.notEqual(series.points[1].segmentId, series.points[2].segmentId)
  assert.equal(series.points[2].value, 0)
})
