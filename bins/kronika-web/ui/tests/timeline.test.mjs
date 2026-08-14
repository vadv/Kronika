import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

import { importModule, registryPlugin } from "./import-module.mjs"

const helpers = await importModule(
  'export { FindingMarker, MARKER_CLUSTER_PX, exactValue, findingShape, findingTrack, groupFindings, healthEvaluationAtOrBefore, healthThreshold, healthTimelineSeries, sampleWindow, selectedTimelineTimes, timelineDecorations, timelineRecordedTimes } from "../src/timeline.tsx"',
  { plugins: [registryPlugin([{ typeId: "1104001", logicalName: "os_meminfo", columns: ["ts", "mem_total", "mem_free", "mem_available"] }])] },
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

test("the shared empty timeline uses the hour-aware status", async () => {
  const source = await readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8")
  assert.match(source, /t\(emptyHourStatusKey\(hour\)\)/)
})

test("default arrows and pointer selection are owned by the selected recorded lane", async () => {
  const [app, keyboard, timeline] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/keyboard.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(app, /moveCursor/)
  assert.doesNotMatch(keyboard, /60_000_000|MINUTE/)
  assert.match(timeline, /selectedTimelineTimes\(lanes, selectedLane\)/)
  assert.match(timeline, /<UPlotChart/)
  assert.match(timeline, /window\.addEventListener\("keydown", move\)/)
})

test("one timeline mark stays an unlabeled shape", () => {
  const [marker] = helpers.groupFindings([finding("event", 100, "1")], 0, 1_000, 100, 10)
  assert.equal(marker.findings.length, 1)
  assert.deepEqual(marker.composition, [{ count: 1, kind: "event" }])
  assert.deepEqual(marker.findings, [finding("event", 100, "1")])
  const markup = renderToStaticMarkup(createElement(helpers.FindingMarker, { marker, onActivate() {}, share: 0.1, t: (key) => key }))
  assert.match(markup, /data-marker-shape="circle"[^>]*aria-hidden="true"|aria-hidden="true"[^>]*data-marker-shape="circle"/)
  assert.equal(markup.replace(/<[^>]+>/g, ""), "")
})

test("dense coincident marks become one compact count", () => {
  const input = [finding("event", 100, "3"), finding("event", 100, "1"), finding("event", 100, "2")]
  const [marker] = helpers.groupFindings(input, 0, 1_000, 100, 10)
  assert.equal(marker.findings.length, 3)
  assert.deepEqual(marker.composition, [{ count: 3, kind: "event" }])
  assert.deepEqual(marker.findings.map(({ rowOrdinal }) => rowOrdinal), ["1", "2", "3"])
})

test("timeline markers cluster at rendered density with exact ordered locators", () => {
  const input = [
    finding("spike", 150, "3"),
    finding("event", 100, "1"),
    finding("known_bad", 100, "2"),
    finding("event", 205, "4"),
    finding("event", 900, "5"),
  ]
  const grouped = helpers.groupFindings(input, 0, 1_000, 100, 10)

  assert.deepEqual(grouped.map(({ composition, findings }) => ({
    composition,
    count: findings.length,
    endTimestamp: findings.at(-1)?.timestamp,
    kinds: composition.map(({ kind }) => kind),
    startTimestamp: findings[0]?.timestamp,
  })), [
    { composition: [{ count: 1, kind: "event" }, { count: 1, kind: "known_bad" }, { count: 1, kind: "spike" }], count: 3, kinds: ["event", "known_bad", "spike"], startTimestamp: 100, endTimestamp: 150 },
    { composition: [{ count: 1, kind: "event" }], count: 1, kinds: ["event"], startTimestamp: 205, endTimestamp: 205 },
    { composition: [{ count: 1, kind: "event" }], count: 1, kinds: ["event"], startTimestamp: 900, endTimestamp: 900 },
  ])
  const locator = (item) => `${item.segmentId}:${item.typeId}:${item.rowOrdinal}:${item.fieldOrdinal}:${item.timestamp}:${item.kind}`
  assert.deepEqual(
    grouped.flatMap((marker) => marker.findings).map(locator),
    [input[1], input[2], input[0], input[3], input[4]].map(locator),
  )
})

test("duplicate locators keep their exact multiplicity and order", () => {
  const duplicate = finding("event", 100, "1")
  const [marker] = helpers.groupFindings([duplicate, finding("spike", 100, "2"), duplicate], 0, 1_000, 100, 10)
  assert.deepEqual(marker.findings, [duplicate, duplicate, finding("spike", 100, "2")])
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
  assert.equal(compact.length, 2)
  assert.deepEqual(
    helpers.groupFindings(input, 0, 1_000, 1_000, 10).map((marker) => marker.findings.length),
    [2, 1, 1],
  )
})

test("mixed clusters show numeric composition without rail labels or a band", async () => {
  const findings = [
    ...Array.from({ length: 12 }, (_, index) => finding("event", 100, String(index))),
    ...Array.from({ length: 11 }, (_, index) => finding("known_bad", 100, String(index + 20))),
    ...Array.from({ length: 10 }, (_, index) => finding("spike", 100, String(index + 40))),
  ]
  const [marker] = helpers.groupFindings(findings, 0, 1_000, 100, 10)
  const markup = renderToStaticMarkup(createElement(helpers.FindingMarker, {
    marker,
    onActivate() {},
    share: 0.1,
    t: (key) => ({ "locator.event": "Event", "locator.known_bad": "Known bad", "locator.spike": "Spike", "events.source.process": "Process" })[key] ?? key,
  }))
  assert.equal((markup.match(/data-marker-shape=/g) ?? []).length, 3)
  assert.match(markup, /data-marker-composition="event:12 known_bad:11 spike:10"/)
  assert.match(markup, />12<.*>11<.*>10<.*>33</)
  assert.match(markup, new RegExp(`clamp\\(${helpers.MARKER_CLUSTER_PX / 2}px`))
  assert.doesNotMatch(markup.replace(/aria-label="[^"]*"|title="[^"]*"/g, ""), /Event|Known bad|Spike|Process/)
  const [source, styles] = await Promise.all([
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(source, /marker-cluster-summary|className="finding-rail"/)
  assert.doesNotMatch(styles, /\.marker-cluster-summary|\.finding-rail|\.neutral-rail/)
  assert.match(styles, new RegExp(`width: ${helpers.MARKER_CLUSTER_PX - 8}px`))
  assert.deepEqual(helpers.groupFindings([], 0, 1_000, 100), [])
})

test("finding kinds have non-color shape identities", () => {
  assert.equal(helpers.findingShape("event"), "circle")
  assert.equal(helpers.findingShape("known_bad"), "diamond")
  assert.equal(helpers.findingShape("spike"), "triangle")
  const render = (kind) => {
    const [marker] = helpers.groupFindings([finding(kind, 100, "1")], 0, 1_000, 100, 10)
    return renderToStaticMarkup(createElement(helpers.FindingMarker, { marker, onActivate() {}, share: 0.1, t: (key) => key }))
  }
  assert.match(render("event"), /fill="var\(--event\)"/)
  assert.match(render("known_bad"), /fill="var\(--bad\)"/)
  assert.match(render("spike"), /stroke="var\(--warn\)"/)
})

test("health metrics share stored evaluation timestamps and a strict nonfuture cursor", () => {
  const rows = [
    { logicalName: "health", ordinal: "0", segmentId: "a", timestamp: 103, typeId: "0", values: { os_health: 90, overall_health: 62, postgres_health: 72 } },
    { logicalName: "health", ordinal: "1", segmentId: "a", timestamp: 109, typeId: "0", values: { os_health: 90, overall_health: 45, postgres_health: 55 } },
  ]
  const health = helpers.healthTimelineSeries(rows)
  assert.deepEqual(health.series.map(({ field, points }) => [field, points.map(({ timestamp, value }) => [timestamp, value])]), [
    ["overall_health", [[103, 62], [109, 45]]],
    ["os_health", [[103, 90], [109, 90]]],
    ["postgres_health", [[103, 72], [109, 55]]],
  ])
  assert.equal(health.threshold, 50)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 102), null)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 105), 103)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 106), 103)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 109), 109)

  const osOnly = helpers.healthTimelineSeries([
    { logicalName: "health", ordinal: "3", segmentId: "c", timestamp: 300, typeId: "0", values: { os_health: 73, overall_health: 73 } },
  ])
  assert.deepEqual(osOnly.series.map(({ field }) => field), ["overall_health", "os_health"])
  assert.equal(osOnly.series.some(({ field }) => field === "postgres_health"), false)
})

test("the selected lane owns exact heterogeneous timestamps including null observations", () => {
  const health = [{ points: [
    { timestamp: 100, value: 90 },
    { timestamp: 105, value: null },
    { timestamp: 111, value: 88 },
  ] }, { points: [
    { timestamp: 105, value: 91 },
    { timestamp: 118, value: 89 },
  ] }]
  const cpu = [{ points: [
    { timestamp: 102, value: 20 },
    { timestamp: 107, value: 21 },
  ] }]
  const lanes = [{ key: "health", series: health }, { key: "cpu_busy", series: cpu }]
  assert.deepEqual(helpers.selectedTimelineTimes(lanes, "health"), [100, 105, 111, 118])
  assert.deepEqual(helpers.selectedTimelineTimes(lanes, "cpu_busy"), [102, 107])
  assert.deepEqual(helpers.selectedTimelineTimes(lanes, "missing"), [100, 105, 111, 118])
})

test("timeline distinguishes unavailable edges from the uncollected current-hour tail", () => {
  const hour = Date.UTC(2026, 7, 13, 10) * 1_000
  const end = hour + 3_600_000_000
  const point = (offset, value) => ({ segmentId: "a", timestamp: hour + offset, value })
  const selected = [{ points: [point(600_000_000, 10), point(1_800_000_000, null), point(2_100_000_000, 12)] }]
  const lanes = [
    { series: selected },
    { series: [{ points: [point(300_000_000, 7), point(2_400_000_000, 9)] }] },
  ]
  assert.deepEqual(helpers.sampleWindow(lanes), { start: hour + 300_000_000, end: hour + 2_400_000_000 })
  assert.deepEqual(helpers.timelineDecorations(lanes, selected, hour, end, hour + 3_000_000_000), [
    { from: hour, to: hour + 300_000_000, tone: "unavailable" },
    { from: hour + 2_400_000_000, to: end, tone: "unavailable" },
    { from: hour + 2_100_000_000, to: end, tone: "future" },
  ])
  assert.deepEqual(helpers.timelineDecorations(lanes, selected, hour, end, end + 1), [
    { from: hour, to: hour + 300_000_000, tone: "unavailable" },
    { from: hour + 2_400_000_000, to: end, tone: "unavailable" },
  ])
})

test("timeline readings use only an exact observation", () => {
  const points = [10, null, 12].map((value, index) => ({ segmentId: "a", timestamp: index + 1, value }))
  assert.equal(helpers.exactValue(points, 2), null)
  assert.equal(helpers.exactValue(points, 3), 12)
  assert.equal(helpers.exactValue(points, 4), null)
})

test("only overall health owns the below-50 band and exact findings map to tracks", () => {
  assert.equal(helpers.healthThreshold("overall_health"), 50)
  assert.equal(helpers.healthThreshold("os_health"), null)
  assert.equal(helpers.healthThreshold("postgres_health"), null)
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "health", typeId: "0", fieldOrdinal: 1 }), "health")
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "health", typeId: "0", fieldOrdinal: 0 }), null)
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "os_meminfo", typeId: "1104001", fieldOrdinal: 3 }), "memory")
  assert.equal(helpers.groupFindings([finding("event", 100, "1")], 0, 1_000, 100)[0].findings[0]?.kind, "event")
  assert.equal(helpers.groupFindings([finding("spike", 100, "1")], 0, 1_000, 100)[0].findings[0]?.kind, "spike")
})

test("the renderer is exclusively the shared uPlot adapter", async () => {
  const source = await readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8")
  assert.match(source, /<UPlotChart/)
  assert.doesNotMatch(source, /SeriesLine|svgPath|timelineRuns|preserveAspectRatio/)
  assert.ok(source.indexOf("if (selected === undefined)") > source.indexOf("const threshold = useMemo"))
})

test("timeline controls stay above a full-width plot without a redundant time title", async () => {
  const [source, styles, chart] = await Promise.all([
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8"),
  ])
  assert.match(styles, /\.timeline-shell \{[^}]*flex-direction: column;[^}]*overflow: hidden; \}/)
  assert.match(styles, /\.timeline-labels \{[^}]*display: flex;[^}]*overflow-x: auto; \}/)
  assert.match(styles, /\.uplot-figure\.timeline-chart \{[^}]*min-height: 216px;[^}]*padding:/)
  assert.ok(source.indexOf('className="timeline-labels"') < source.indexOf('className="timeline-chart"'))
  assert.doesNotMatch(chart, /Time, browser local|Время, местное в браузере/)
})
