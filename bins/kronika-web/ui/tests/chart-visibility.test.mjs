import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { loadChartVisibility } = await importModule('export { loadChartVisibility } from "../src/chart-visibility.tsx"')

test("chart visibility is shown by default and only the hidden preference suppresses it", () => {
  const stored = (value) => ({ getItem: () => value })
  assert.equal(loadChartVisibility(stored(null)), true)
  assert.equal(loadChartVisibility(stored("1")), true)
  assert.equal(loadChartVisibility(stored("unexpected")), true)
  assert.equal(loadChartVisibility(stored("0")), false)
  assert.equal(loadChartVisibility({ getItem() { throw new Error("unavailable") } }), true)
})

test("each large chart container follows the shared visibility preference", async () => {
  const expected = {
    "app.tsx": 1,
    "detail.tsx": 1,
    "events-view.tsx": 2,
    "postgres-relations-view.tsx": 1,
    "postgres-view.tsx": 6,
    "process-table.tsx": 1,
    "system-view.tsx": 3,
  }
  for (const [file, count] of Object.entries(expected)) {
    const source = await readFile(new URL(`../src/${file}`, import.meta.url), "utf8")
    assert.equal(source.match(/<ChartOnly>/g)?.length ?? 0, count, file)
  }
  const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(app, /data-testid="charts-toggle"/)
  assert.match(app, /kronika\.charts/)
  assert.match(app, /charts-hidden/)
})
