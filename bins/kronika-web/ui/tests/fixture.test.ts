import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { gunzipSync } from "node:zlib"
import test from "node:test"

import { bundledFixtureHour, bundledFixtureRange } from "../src/fixture.ts"

const encoded = readFileSync(new URL("../fixtures/real-hour.json.gz", import.meta.url))
const fixture = JSON.parse(gunzipSync(encoded).toString("utf8"))

test("the approved real hour converts without losing entity relationships", () => {
  Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: fixture })
  const range = bundledFixtureRange()
  assert.notEqual(range, null)
  const hour = bundledFixtureHour(Number(range?.from))
  assert.notEqual(hour, null)
  assert.equal(hour?.processes.length, 111_673)
  assert.equal(hour?.activities.length, 2_888)
  assert.equal(hour?.findings.length, 2_884)
  assert.equal(hour?.sourceFamilies.find((source) => source.name === "postgresql")?.present, true)
  const processByTime = new Map<number, Set<unknown>>()
  for (const row of hour?.processes ?? []) {
    const pids = processByTime.get(row.timestamp) ?? new Set()
    pids.add(row.values.pid)
    processByTime.set(row.timestamp, pids)
  }
  const processTimes = [...processByTime.keys()]
  const nearestProcesses = new Map<number, Set<unknown>>()
  for (const activity of hour?.activities ?? []) {
    if (!nearestProcesses.has(activity.timestamp)) {
      const timestamp = processTimes.reduce((nearest, candidate) =>
        Math.abs(candidate - activity.timestamp) < Math.abs(nearest - activity.timestamp) ? candidate : nearest,
      )
      nearestProcesses.set(activity.timestamp, processByTime.get(timestamp) ?? new Set())
    }
  }
  const joined = hour?.activities.filter((row) => nearestProcesses.get(row.timestamp)?.has(row.values.pid)).length
  assert.equal(joined, 2_755)
})

test("the production path has no fixture when no approved bundle is injected", () => {
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  assert.equal(bundledFixtureRange(), null)
  assert.equal(bundledFixtureHour(0), null)
})
