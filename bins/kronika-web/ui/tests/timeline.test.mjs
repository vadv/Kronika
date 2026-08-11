import assert from "node:assert/strict"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  external: ["kronika:registry"],
  format: "esm",
  platform: "node",
  stdin: {
    contents: 'export { findingShape, groupFindings, seriesYAt, timelineRuns } from "../src/timeline.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

function finding(kind, timestamp, ordinal) {
  return {
    category: null,
    fieldOrdinal: 0,
    kind,
    logicalName: "os_process",
    rowOrdinal: ordinal,
    segmentId: "segment-a",
    timestamp,
    typeId: "1100001",
  }
}

test("timeline markers cluster at the rendered scale and expand to every exact locator", () => {
  const input = [
    finding("spike", 150, "3"),
    finding("event", 100, "1"),
    finding("known_bad", 100, "2"),
    finding("event", 205, "4"),
    finding("event", 900, "5"),
  ]
  const grouped = helpers.groupFindings(input, 0, 1_000, 100, 10)

  assert.deepEqual(grouped.map(({ count, kinds, startTimestamp, endTimestamp }) => ({ count, kinds, startTimestamp, endTimestamp })), [
    { count: 3, kinds: ["event", "known_bad", "spike"], startTimestamp: 100, endTimestamp: 150 },
    { count: 1, kinds: ["event"], startTimestamp: 205, endTimestamp: 205 },
    { count: 1, kinds: ["event"], startTimestamp: 900, endTimestamp: 900 },
  ])
  const locator = (item) => `${item.segmentId}:${item.typeId}:${item.rowOrdinal}:${item.fieldOrdinal}:${item.timestamp}:${item.kind}`
  assert.deepEqual(
    grouped.flatMap((marker) => marker.findings).map(locator).sort(),
    input.map(locator).sort(),
  )
})

test("marker clustering is deterministic and separates locators when more pixels are available", () => {
  const input = [
    finding("spike", 150, "3"),
    finding("event", 100, "1"),
    finding("known_bad", 100, "2"),
    finding("event", 205, "4"),
  ]
  const snapshot = (groups) => groups.map((marker) => ({
    count: marker.count,
    locators: marker.findings.map((item) => `${item.timestamp}:${item.kind}:${item.rowOrdinal}`),
    timestamp: marker.timestamp,
  }))
  const compact = helpers.groupFindings(input, 0, 1_000, 100, 10)
  assert.deepEqual(snapshot(compact), snapshot(helpers.groupFindings(input.toReversed(), 0, 1_000, 100, 10)))
  for (let index = 1; index < compact.length; index += 1) {
    const previous = compact[index - 1]
    const current = compact[index]
    assert.ok((current.timestamp - previous.timestamp) / 1_000 * 100 > 10)
  }
  assert.deepEqual(
    helpers.groupFindings(input, 0, 1_000, 1_000, 10).map((marker) => marker.count),
    [2, 1, 1],
  )
})

test("finding kinds have non-color shape identities", () => {
  assert.equal(helpers.findingShape("event"), "circle")
  assert.equal(helpers.findingShape("known_bad"), "diamond")
  assert.equal(helpers.findingShape("spike"), "triangle")
})

test("timeline series break at null samples without dropping zero", () => {
  const runs = [...helpers.timelineRuns([
    { segmentId: "host-a", timestamp: 100, value: 10 },
    { segmentId: "host-a", timestamp: 200, value: null },
    { segmentId: "host-a", timestamp: 300, value: 0 },
  ]).values()]
  assert.deepEqual(runs.map((run) => run.map((point) => point.value)), [[10], [0]])
  // An absent sample breaks the line without moving the samples around it.
  assert.equal(
    helpers.seriesYAt([
      { segmentId: "host-a", timestamp: 100, value: 10 },
      { segmentId: "host-a", timestamp: 200, value: null },
      { segmentId: "host-a", timestamp: 300, value: 90 },
    ], "host-a", 100, 0),
    helpers.seriesYAt([
      { segmentId: "host-a", timestamp: 100, value: 10 },
      { segmentId: "host-a", timestamp: 300, value: 90 },
    ], "host-a", 100, 0),
  )
})

test("a finding from another source family sits on the nearest healthline segment", () => {
  const points = [
    { segmentId: "host-a", timestamp: 100, value: 20 },
    { segmentId: "host-a", timestamp: 200, value: 40 },
    { segmentId: "host-b", timestamp: 800, value: 90 },
  ]
  assert.equal(
    helpers.seriesYAt(points, "postgresql-a", 150, 0),
    helpers.seriesYAt(points, "host-a", 150, 0),
  )
})
