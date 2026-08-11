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
    contents: 'export { chartRuns } from "../src/series-chart.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

test("mini charts split paths around a recorded null", () => {
  const runs = [...helpers.chartRuns([
    { segmentId: "a", timestamp: 1, value: 2 },
    { segmentId: "a", timestamp: 2, value: null },
    { segmentId: "a", timestamp: 3, value: 0 },
  ]).values()]
  assert.deepEqual(runs.map((run) => run.map((point) => point.value)), [[2], [0]])
})
