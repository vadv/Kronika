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

test("process history reads in time order and keeps null distinct from zero", () => {
  const series = detail.processLensHistory([
    row(30, { pid: 77, starttime: 10, utime: null, stime: 4 }, "segment-a", "2"),
    row(10, { pid: 77, starttime: 10, utime: 0, stime: 2 }, "segment-a", "0"),
    row(20, { pid: 77, starttime: 10, utime: 3, stime: 3 }, "segment-a", "1"),
  ], "cpu")

  assert.deepEqual(series.map((item) => item.field), [
    "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "minflt", "majflt",
  ])
  assert.deepEqual(series[0].points.map((point) => point.value), [0, 3, null])
  assert.deepEqual(series[1].points.map((point) => point.value), [2, 3, 4])
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
