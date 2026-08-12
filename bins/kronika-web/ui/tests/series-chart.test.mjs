import assert from "node:assert/strict"
import test from "node:test"

import { importFile, importModule } from "./import-module.mjs"

const helpers = await importModule('export { chartDomain, chartRuns, numericChartPoints, readingAt, sampleAtOrBefore, uncollectedStart } from "../src/series-chart.tsx"')
const { buildMetricSamples, numericRuns } = await importFile("../src/chart.ts")

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

test("the shared sample builder omits absent fields but keeps explicit null and joins segments", () => {
  const rows = [
    { segmentId: "a", timestamp: 1, values: { metric: 4 } },
    { segmentId: "a", timestamp: 2, values: { other: 9 } },
    { segmentId: "a", timestamp: 3, values: { metric: null } },
    { segmentId: "a", timestamp: 4, values: { metric: 0 } },
    { segmentId: "b", timestamp: 5, values: { metric: 7 } },
  ]
  const points = buildMetricSamples(rows, (row) => Object.hasOwn(row.values, "metric") ? row.values.metric : undefined)
  assert.deepEqual(points.map(({ segmentId, timestamp, value }) => [segmentId, timestamp, value]), [
    ["a", 1, 4], ["a", 3, null], ["a", 4, 0], ["b", 5, 7],
  ])
  assert.deepEqual([...numericRuns(points, (left, right) => left.localeCompare(right)).values()].map((run) => run.map((point) => point.value)), [[0, 7]])
})

test("the selected point is the stored sample at or before the cursor", () => {
  const points = [
    { segmentId: "a", timestamp: 10, value: 2 },
    { segmentId: "a", timestamp: 20, value: null },
    { segmentId: "b", timestamp: 30, value: 8 },
  ]
  assert.equal(helpers.sampleAtOrBefore(points, 19).timestamp, 10)
  assert.equal(helpers.sampleAtOrBefore(points, 20).value, null)
  assert.equal(helpers.sampleAtOrBefore(points, 9), null)
})

test("only a current hour marks its uncollected remainder", () => {
  const hour = 3_600_000_000
  const points = [{ segmentId: "a", timestamp: hour + 42, value: 1 }]
  assert.equal(helpers.uncollectedStart(points, hour, hour + 1_000), hour + 42)
  assert.equal(helpers.uncollectedStart(points, hour, hour + 3_600_000_000), null)
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
