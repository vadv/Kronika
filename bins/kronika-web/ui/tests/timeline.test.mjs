import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const helpers = await importModule(
  'export { findingShape, findingTrack, groupFindings, healthThreshold, healthTimelineSeries, laneRange, overviewLaneCount, sampleWindow, seriesYAt, timelineRuns, valueAt } from "../src/timeline.tsx"',
  { plugins: [registryPlugin([{ typeId: "1104001", logicalName: "os_meminfo", columns: ["ts", "mem_total", "mem_available"] }])] },
)

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

  assert.deepEqual(grouped.map(({ count, kinds, placement, startTimestamp, endTimestamp }) => ({ count, kinds, placement, startTimestamp, endTimestamp })), [
    { count: 1, kinds: ["event"], placement: "event", startTimestamp: 100, endTimestamp: 100 },
    { count: 2, kinds: ["known_bad", "spike"], placement: "neutral", startTimestamp: 100, endTimestamp: 150 },
    { count: 1, kinds: ["event"], startTimestamp: 205, endTimestamp: 205 },
    { count: 1, kinds: ["event"], startTimestamp: 900, endTimestamp: 900 },
  ].map((item) => ({ placement: "event", ...item })))
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
    if (current.placement === previous.placement && current.track === previous.track) {
      assert.ok((current.timestamp - previous.timestamp) / 1_000 * 100 > 10)
    }
  }
  assert.deepEqual(
    helpers.groupFindings(input, 0, 1_000, 1_000, 10).map((marker) => marker.count),
    [1, 1, 1, 1],
  )
})

test("finding kinds have non-color shape identities", () => {
  assert.equal(helpers.findingShape("event"), "circle")
  assert.equal(helpers.findingShape("known_bad"), "diamond")
  assert.equal(helpers.findingShape("spike"), "triangle")
})

test("timeline series cross segment boundaries and break only at stored nulls", () => {
  const runs = [...helpers.timelineRuns([
    { segmentId: "host-a", timestamp: 100, value: 10 },
    { segmentId: "host-a", timestamp: 200, value: null },
    { segmentId: "host-a", timestamp: 300, value: 0 },
    { segmentId: "host-b", timestamp: 400, value: 12 },
  ]).values()]
  assert.deepEqual(runs.map((run) => run.map((point) => point.value)), [[0, 12]])
  assert.deepEqual(runs[0].map((point) => point.segmentId), ["host-a", "host-b"])
})

test("health metrics remain three exact series", () => {
  const rows = [
    { logicalName: "health", ordinal: "0", segmentId: "a", timestamp: 100, typeId: "0", values: { os_health: 81, overall_health: 62 } },
    { logicalName: "health", ordinal: "1", segmentId: "a", timestamp: 150, typeId: "0", values: { postgres_health: 77 } },
    { logicalName: "health", ordinal: "2", segmentId: "b", timestamp: 200, typeId: "0", values: { os_health: 83, overall_health: null } },
  ]
  const health = helpers.healthTimelineSeries(rows)
  assert.deepEqual(health.series.map(({ field, points }) => [field, points.map(({ value }) => value)]), [
    ["overall_health", [62, null]],
    ["os_health", [81, 83]],
    ["postgres_health", [77]],
  ])
  assert.equal(health.threshold, 50)
})

test("the displayed sample window ends at the last stored number", () => {
  const points = [
    { segmentId: "a", timestamp: 100, value: null },
    { segmentId: "a", timestamp: 200, value: 8 },
    { segmentId: "b", timestamp: 300, value: 9 },
    { segmentId: "b", timestamp: 400, value: null },
  ]
  assert.deepEqual(helpers.sampleWindow([{ series: [{ points }] }]), { start: 200, end: 300 })
  assert.equal(helpers.sampleWindow([{ series: [{ points: points.map((point) => ({ ...point, value: null })) }] }]), null)
})

test("a finding attaches only to an exact sample in the same segment", () => {
  const points = [
    { segmentId: "host-a", timestamp: 100, value: 20 },
    { segmentId: "host-a", timestamp: 200, value: 40 },
  ]
  assert.equal(typeof helpers.seriesYAt(points, "host-a", 100, 0), "number")
  assert.equal(helpers.seriesYAt(points, "host-a", 150, 0), null)
  assert.equal(helpers.seriesYAt(points, "postgresql-a", 100, 0), null)
})

test("a stored null wins over an older number at the cursor", () => {
  const points = [10, null, 12].map((value, index) => ({ segmentId: "a", timestamp: index + 1, value }))
  assert.equal(helpers.valueAt(points, 2), null)
  assert.equal(helpers.valueAt(points, 3), 12)
})

test("only overall health owns the below-50 band and exact findings map to tracks", () => {
  assert.equal(helpers.healthThreshold("overall_health"), 50)
  assert.equal(helpers.healthThreshold("os_health"), null)
  assert.equal(helpers.healthThreshold("postgres_health"), null)
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "health", typeId: "0", fieldOrdinal: 1 }), "health")
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "health", typeId: "0", fieldOrdinal: 0 }), null)
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "os_meminfo", typeId: "1104001", fieldOrdinal: 2 }), "memory")
  assert.equal(helpers.groupFindings([finding("event", 100, "1")], 0, 1_000, 100)[0].placement, "event")
  assert.equal(helpers.groupFindings([finding("spike", 100, "1")], 0, 1_000, 100)[0].placement, "neutral")
})

test("timeline domains and overview density are explicit", () => {
  assert.deepEqual(helpers.laneRange({ domain: [0, 100], series: [{ points: [{ segmentId: "a", timestamp: 1, value: 3 }] }] }), { low: 0, span: 100 })
  assert.deepEqual(helpers.laneRange({ series: [{ points: [{ segmentId: "a", timestamp: 1, value: 12 }] }] }), { low: 0, span: 20 })
  assert.deepEqual(helpers.laneRange({ minimumSpan: 5, series: [{ points: [{ segmentId: "a", timestamp: 1, value: 1 }] }] }), { low: 0, span: 5 })
  assert.deepEqual(helpers.laneRange({ minimumSpan: 5, series: [{ points: [{ segmentId: "a", timestamp: 1, value: 12 }] }] }), { low: 0, span: 20 })
  assert.deepEqual([helpers.overviewLaneCount(1_200), helpers.overviewLaneCount(900), helpers.overviewLaneCount(600)], [4, 3, 2])
})
