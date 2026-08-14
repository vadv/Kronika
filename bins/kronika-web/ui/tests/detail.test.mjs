import assert from "node:assert/strict"
import test from "node:test"

import { importFile, registryPlugin } from "./import-module.mjs"

const detail = await importFile("../src/detail.tsx", { plugins: [registryPlugin([])] })

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
    "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "minflt", "majflt", "nice", "prio", "rtprio",
  ])
  assert.deepEqual(series[1].points.map((point) => point.value), [null, 4, 16])
})

test("an explicit null counter resets the next rate", () => {
  const series = detail.processLensHistory([
    row(1_000_000, { pid: 77, starttime: 10, stime: 10 }, "segment-a", "0"),
    row(2_000_000, { pid: 77, starttime: 10, stime: null }, "segment-a", "1"),
    row(3_000_000, { pid: 77, starttime: 10, stime: 30 }, "segment-b", "2"),
    row(4_000_000, { pid: 77, starttime: 10, stime: 34 }, "segment-b", "3"),
  ], "cpu")

  assert.deepEqual(series[1].points.map((point) => point.value), [null, null, null, 4])
})

test("a row that does not own the counter neither creates a null nor resets the rate", () => {
  const series = detail.processLensHistory([
    row(1_000_000, { pid: 77, starttime: 10, stime: 10 }, "segment-a", "0"),
    row(2_000_000, { pid: 77, starttime: 10, utime: 99 }, "segment-a", "1"),
    row(3_000_000, { pid: 77, starttime: 10, stime: 30 }, "segment-b", "2"),
  ], "cpu")

  assert.deepEqual(series[1].points.map((point) => [point.timestamp, point.value]), [[1_000_000, null], [3_000_000, 10]])
})

test("null values split rendered history runs while later zero stays numeric", () => {
  const [series] = detail.processLensHistory([
    row(10, { pid: 77, starttime: 10, rmem_kb: 1 }),
    row(20, { pid: 77, starttime: 10, rmem_kb: null }),
    row(30, { pid: 77, starttime: 10, rmem_kb: 0 }),
  ], "memory")

  assert.equal(series.points[0].segmentId, series.points[1].segmentId)
  assert.equal(series.points[1].segmentId, series.points[2].segmentId)
  assert.equal(series.points[2].value, 0)
})

test("chart presentation normalizes process counters to the unit shown on its axis", () => {
  const source = { field: "utime", key: "col.utime", kind: "cores", counter: true, points: [
    { segmentId: "segment-a", timestamp: 10, value: null },
    { segmentId: "segment-a", timestamp: 20, value: 250 },
    { segmentId: "segment-a", timestamp: 30, value: 0 },
  ] }

  assert.deepEqual(detail.processChartPoints(source, 100), [
    { segmentId: "segment-a", timestamp: 10, value: null },
    { segmentId: "segment-a", timestamp: 20, value: 2.5 },
    { segmentId: "segment-a", timestamp: 30, value: 0 },
  ])
  assert.equal(detail.processChartUnit("cores", () => " cores", 100), "cores")
  assert.equal(detail.processChartUnit("bytes", () => "/s", 100), "B/s")
  assert.equal(detail.processChartUnit("rate", () => "/s", 100), "#/s")
})

test("process detail mounts one selected chart and exposes metric actions", async () => {
  const source = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"))

  assert.match(source, /className="process-history-selector" role="group"/)
  assert.match(source, /aria-pressed=\{series\.field === selectedHistory\?\.field\}/)
  assert.match(source, /data-testid=\{`process-history-metric-\$\{series\.field\}`\}/)
  assert.doesNotMatch(source, /<TimeTicks/)
  assert.equal(source.match(/<SeriesChart/g)?.length, 1)
})

test("all meaningful CPU numeric fields have history and nice uses a signed scale", () => {
  for (const field of ["nice", "prio", "rtprio"]) assert.ok(detail.PROCESS_HISTORY_FIELDS.includes(field), field)
  const rows = [
    row(1, { nice: -5, prio: 15, rtprio: 0 }),
    row(2, { nice: 0, prio: 20, rtprio: 1 }),
  ]
  const byField = new Map(detail.processLensHistory(rows, "cpu").map((series) => [series.field, series]))
  assert.deepEqual(byField.get("nice").points.map(({ value }) => value), [-5, 0])
  assert.equal(byField.get("nice").scale, "signed")
  assert.equal(byField.get("prio").unit, "priority")
  assert.equal(byField.get("rtprio").unit, "priority")
})

test("process history requests project PID without process start time", async () => {
  assert.equal(detail.PROCESS_HISTORY_FIELDS[0], "pid")
  assert.equal(detail.PROCESS_HISTORY_FIELDS.includes("starttime"), false)
  const source = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/app.tsx", import.meta.url), "utf8"))
  assert.match(source, /loadSeries\(hour, "os_process", \{ pid: selectedPid \}, PROCESS_HISTORY_FIELDS/)
  assert.doesNotMatch(source, /selectedStart|starttime: selectedStart/)
})

test("linked Activity detail shows elapsed values instead of repeatable absolute starts", async () => {
  const source = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"))
  const fields = /const ACTIVITY_FIELDS = \[([\s\S]*?)\] as const/.exec(source)?.[1] ?? ""
  for (const field of ["backend_start", "xact_start", "query_start", "state_change"]) assert.doesNotMatch(fields, new RegExp(field))
  for (const field of ["backend_age_ms", "transaction_duration_ms", "query_duration_ms", "state_duration_ms"]) assert.match(source, new RegExp(`\\["${field}"`))
  assert.match(source, /value=\{humanDuration\(elapsed, locale\)\}/)
})

test("Escape closes the shared detail dock unless a child already handled it", async () => {
  const source = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"))
  assert.match(source, /if \(event\.key !== "Escape" \|\| event\.defaultPrevented\) return/)
  assert.match(source, /queueMicrotask\(\(\) => \{\s+if \(event\.defaultPrevented\) return/)
  assert.match(source, /event\.preventDefault\(\)\s+onClose\(\)/)
  assert.match(source, /window\.addEventListener\("keydown", escape\)/)
  assert.match(source, /return \(\) => window\.removeEventListener\("keydown", escape\)/)
})
