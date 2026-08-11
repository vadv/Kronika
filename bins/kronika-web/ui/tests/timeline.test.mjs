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

test("timeline markers aggregate only identical kind and timestamp", () => {
  const first = finding("event", 200, "7")
  const grouped = helpers.groupFindings([
    finding("spike", 100, "1"),
    first,
    finding("event", 200, "8"),
    finding("known_bad", 200, "9"),
  ])

  assert.deepEqual(grouped.map(({ count, kind, timestamp }) => ({ count, kind, timestamp })), [
    { count: 1, kind: "spike", timestamp: 100 },
    { count: 2, kind: "event", timestamp: 200 },
    { count: 1, kind: "known_bad", timestamp: 200 },
  ])
  assert.equal(grouped[1].finding, first)
  assert.deepEqual(grouped[1].findings.map((finding) => finding.rowOrdinal), ["7", "8"])
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
  assert.equal(
    helpers.seriesYAt([
      { segmentId: "host-a", timestamp: 100, value: 10 },
      { segmentId: "host-a", timestamp: 200, value: null },
      { segmentId: "host-a", timestamp: 300, value: 90 },
    ], "host-a", 200, 0),
    helpers.seriesYAt([{ segmentId: "host-a", timestamp: 100, value: 10 }], "host-a", 100, 0),
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
