import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const helpers = await importModule('export { chartDomain, chartRuns, numericChartPoints, readingAt } from "../src/series-chart.tsx"')

test("chart domains keep percentages exact and give counts and durations a zero-based nice ceiling", () => {
  assert.deepEqual(helpers.chartDomain([42], "percent"), { low: 0, high: 100 })
  assert.deepEqual(helpers.chartDomain([12], "count"), { low: 0, high: 20 })
  assert.deepEqual(helpers.chartDomain([0.42], "duration"), { low: 0, high: 0.5 })
})

test("a recorded null is the reading at its timestamp", () => {
  const points = [10, null, 12].map((value, index) => ({ segmentId: "a", timestamp: index + 1, value }))
  assert.equal(helpers.readingAt(points, 2), null)
  assert.equal(helpers.readingAt(points, 3), 12)
})

test("mini charts join storage segments but do not draw isolated samples around a recorded null", () => {
  const runs = [...helpers.chartRuns([
    { segmentId: "a", timestamp: 1, value: 2 },
    { segmentId: "a", timestamp: 2, value: null },
    { segmentId: "a", timestamp: 3, value: 0 },
    { segmentId: "b", timestamp: 4, value: 3 },
  ]).values()]
  assert.deepEqual(runs.map((run) => run.map((point) => point.value)), [[0, 3]])
  assert.deepEqual(runs[0].map((point) => point.segmentId), ["a", "b"])
})

test("an unavailable middle sample produces no fake line stubs", () => {
  const points = [10, null, 12].map((value, index) => ({ segmentId: "a", timestamp: index + 1, value }))
  assert.equal(helpers.chartRuns(points).size, 0)
})

test("an all-null chart has no drawable samples while zero remains data", () => {
  assert.equal(helpers.numericChartPoints([
    { segmentId: "a", timestamp: 1, value: null },
  ]).length, 0)
  assert.equal(helpers.numericChartPoints([
    { segmentId: "a", timestamp: 1, value: 0 },
  ]).length, 1)
})
