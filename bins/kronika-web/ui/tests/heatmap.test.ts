import assert from "node:assert/strict"
import test from "node:test"

import { HEATMAP_STEPS, collapseHeatmapView, heatmap, heatmapIntensity, heatmapIntervals, heatmapEntityKey } from "../src/heatmap.ts"

const HOUR = 7_200_000_000_000
const MINUTE = 60_000_000

function counterSamples(entity: string, points: readonly (readonly [number, number | null])[]) {
  return points.map(([minute, value]) => ({ entity, timestamp: HOUR + minute * MINUTE, value }))
}

test("intervals carry exact boundaries and cover the hour", () => {
  const intervals = heatmapIntervals(HOUR, 12)
  assert.equal(intervals.length, 12)
  assert.equal(intervals[0]?.start, HOUR)
  assert.equal(intervals[0]?.end, HOUR + 300 * 1_000_000 - 1)
  assert.equal(intervals[11]?.end, HOUR + 3_600_000_000 - 1)
})

test("a counter cell is the interval delta over the observed elapsed time", () => {
  const result = heatmap(counterSamples("a", [[0, 100], [30, 400]]), true, HOUR, 1, 5)
  assert.equal(result.rows[0]?.cells[0], (400 - 100) / (30 * 60))
})

test("counter null rules: one sample, zero duration, negative delta; zero delta is 0", () => {
  const one = heatmap(counterSamples("a", [[0, 100]]), true, HOUR, 1, 5)
  assert.equal(one.rows[0]?.cells[0], null)
  const torn = heatmap([
    { entity: "a", timestamp: HOUR, value: 1 },
    { entity: "a", timestamp: HOUR, value: 2 },
  ], true, HOUR, 1, 5)
  assert.equal(torn.rows[0]?.cells[0], null)
  const reset = heatmap(counterSamples("a", [[0, 500], [30, 100]]), true, HOUR, 1, 5)
  assert.equal(reset.rows[0]?.cells[0], null)
  const idle = heatmap(counterSamples("a", [[0, 500], [30, 500]]), true, HOUR, 1, 5)
  assert.equal(idle.rows[0]?.cells[0], 0)
})

test("a gauge cell is the last sample in the interval and null samples are skipped", () => {
  const result = heatmap(counterSamples("a", [[1, 10], [20, 30], [25, null], [40, 7]]), false, HOUR, 2, 5)
  assert.deepEqual(result.rows[0]?.cells, [30, 7])
})

test("ranking does not change with the number of columns", () => {
  const samples = [
    ...counterSamples("small", [[0, 0], [59, 600]]),
    ...counterSamples("big", [[0, 0], [59, 6000]]),
  ]
  const coarse = heatmap(samples, true, HOUR, 6, 5)
  const fine = heatmap(samples, true, HOUR, 60, 5)
  assert.deepEqual(coarse.rows.map((row) => [row.entity, row.total]), fine.rows.map((row) => [row.entity, row.total]))
  assert.equal(coarse.rows[0]?.entity, "big")
  assert.equal(coarse.rows[0]?.total, 6000)
})

test("gauges rank by the window maximum", () => {
  const samples = [
    ...counterSamples("spiky", [[0, 5], [30, 90], [59, 5]]),
    ...counterSamples("flat", [[0, 40], [59, 40]]),
  ]
  const result = heatmap(samples, false, HOUR, 4, 5)
  assert.deepEqual(result.rows.map((row) => [row.entity, row.total]), [["spiky", 90], ["flat", 40]])
})

test("top slicing feeds the others band and totals cover every entity", () => {
  const samples = [
    ...counterSamples("a", [[0, 0], [30, 3600]]),
    ...counterSamples("b", [[0, 0], [30, 1800]]),
    ...counterSamples("c", [[0, 0], [30, 900]]),
  ]
  const result = heatmap(samples, true, HOUR, 1, 2)
  assert.deepEqual(result.rows.map((row) => row.entity), ["a", "b"])
  assert.equal(result.othersCount, 1)
  assert.equal(result.entityCount, 3)
  assert.equal(result.others[0], 0.5)
  assert.equal(result.totals[0], 3.5)
  assert.equal(result.othersTotal, 900)
  assert.equal(result.totalsTotal, 6300)
})

test("the shared scale covers ranked rows and others but not totals", () => {
  const samples = [
    ...counterSamples("a", [[0, 0], [30, 3600]]),
    ...counterSamples("b", [[0, 0], [30, 3600]]),
  ]
  const result = heatmap(samples, true, HOUR, 1, 1)
  assert.equal(result.max, 2)
  assert.equal(result.totals[0], 4)
})

test("samples outside the hour are ignored and unsorted input works", () => {
  const result = heatmap([
    { entity: "a", timestamp: HOUR - 1, value: 0 },
    { entity: "a", timestamp: HOUR + 30 * MINUTE, value: 300 },
    { entity: "a", timestamp: HOUR + 10 * MINUTE, value: 100 },
    { entity: "a", timestamp: HOUR + 3_600_000_000, value: 9999 },
  ], true, HOUR, 1, 5)
  assert.equal(result.rows[0]?.total, 200)
})

test("entities with a null ranking value sort after real totals", () => {
  const samples = [
    ...counterSamples("resettled", [[0, 900], [30, 100]]),
    ...counterSamples("steady", [[0, 0], [30, 60]]),
  ]
  const result = heatmap(samples, true, HOUR, 2, 5)
  assert.deepEqual(result.rows.map((row) => [row.entity, row.total]), [["steady", 60], ["resettled", null]])
})

test("intensity steps: zero at 0, sqrt keeps quarter-of-max three steps up, max at the top", () => {
  assert.equal(heatmapIntensity(0, 100), 0)
  assert.equal(heatmapIntensity(100, 100), HEATMAP_STEPS)
  assert.equal(heatmapIntensity(25, 100), HEATMAP_STEPS / 2)
  assert.equal(heatmapIntensity(0.0001, 100), 1)
})

test("entity keys survive null identity parts", () => {
  assert.notEqual(heatmapEntityKey(["1", null]), heatmapEntityKey(["1", "null"]))
})

test("collapsing a view folds the tail rows into the others band", () => {
  const view = {
    cumulative: true,
    intervals: [{ start: HOUR, end: HOUR + 3_600_000_000 - 1 }],
    rows: [
      { typeId: "1", identity: ["a"], labels: [], total: 100, cells: [2] },
      { typeId: "1", identity: ["b"], labels: [], total: 50, cells: [1] },
      { typeId: "1", identity: ["c"], labels: [], total: 25, cells: [null] },
    ],
    totals: { total: 200, cells: [4] },
    others: { total: 25, cells: [1] },
    othersCount: 1,
    entityCount: 4,
  }
  const collapsed = collapseHeatmapView(view, 1)
  assert.equal(collapsed.rows.length, 1)
  assert.equal(collapsed.othersCount, 3)
  assert.equal(collapsed.others.total, 100)
  assert.deepEqual(collapsed.others.cells, [2])
  assert.equal(collapseHeatmapView(view, 3), view)
})
