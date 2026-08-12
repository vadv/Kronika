import assert from "node:assert/strict"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  format: "esm",
  platform: "node",
  stdin: {
    contents: 'export { chartDomain, chartRuns, readingAt } from "../src/series-chart.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

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

test("mini charts split paths around a recorded null", () => {
  const runs = [...helpers.chartRuns([
    { segmentId: "a", timestamp: 1, value: 2 },
    { segmentId: "a", timestamp: 2, value: null },
    { segmentId: "a", timestamp: 3, value: 0 },
    { segmentId: "b", timestamp: 4, value: 3 },
  ]).values()]
  assert.deepEqual(runs.map((run) => run.map((point) => point.value)), [[2], [0, 3]])
})
