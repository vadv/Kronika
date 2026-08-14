import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const chart = await importModule('export { alignRecordedSeries, axisTimeLabel, chartSummary, exactReadings, isolatedSampleIndices, nearestRecordedTimestamp, sampleText, scalePartitions, scaleRange } from "../src/uplot-chart.tsx"; export { createDisplayTimeFormatter } from "../src/display-time.ts"; export { compact, humanPercent } from "../src/model.ts"')

const format = (value) => String(value)
const line = (id, unit, scale, points) => ({ color: "cyan", id, label: id, points, scale, unit, value: format })

test("aligned data distinguishes missing rows, explicit nulls, zero and storage boundaries", () => {
  const frame = chart.alignRecordedSeries([
    line("one", "%", "percent", [
      { segmentId: "a", timestamp: 1, value: 4 },
      { segmentId: "a", timestamp: 3, value: null },
      { segmentId: "a", timestamp: 4, value: 0 },
      { segmentId: "b", timestamp: 5, value: 7 },
    ]),
    line("two", "%", "percent", [{ segmentId: "b", timestamp: 2, value: 9 }]),
  ])
  assert.deepEqual(frame.timestamps, [1, 2, 3, 4, 5])
  assert.deepEqual(frame.data[1], [4, undefined, null, 0, 7])
  assert.deepEqual(frame.data[2], [undefined, 9, undefined, undefined, undefined])
  assert.deepEqual(frame.isolated.get(1), [0])
  assert.deepEqual(frame.isolated.get(2), [1])
})

test("aligned mode joins identical boundary samples and rejects conflicting values at one timestamp", () => {
  const joined = chart.alignRecordedSeries([
    line("one", "%", "percent", [
      { segmentId: "a", timestamp: 1, value: 4 },
      { segmentId: "b", timestamp: 1, value: 4 },
    ]),
  ])
  assert.deepEqual(joined.data[1], [4])
  assert.throws(() => chart.alignRecordedSeries([
    line("one", "%", "percent", [
      { segmentId: "a", timestamp: 1, value: 4 },
      { segmentId: "b", timestamp: 1, value: 5 },
    ]),
  ]), /conflicting chart sample one@1/)
  assert.throws(() => chart.alignRecordedSeries([
    line("one", "%", "percent", [{ segmentId: "a", timestamp: Number.MAX_SAFE_INTEGER + 1, value: 4 }]),
  ]), /invalid chart timestamp/)
})

test("isolated samples are points and not fake line stubs", () => {
  assert.deepEqual(chart.isolatedSampleIndices([10, null, 12]), [0, 2])
  assert.deepEqual(chart.isolatedSampleIndices([null, 0, undefined, 3, null]), [])
})

test("semantic scales remain explicit", () => {
  assert.deepEqual(chart.scaleRange("percent", [-2, 42, 101]), [0, 100])
  assert.deepEqual(chart.scaleRange("nonnegative", [0, 12]), [0, 20])
  assert.deepEqual(chart.scaleRange("signed", [-4, 12]), [-5, 20])
  assert.deepEqual(chart.scaleRange("signed", [-12, -4]), [-20, 0])
})

test("incompatible units and semantics receive distinct labelled scales", () => {
  const partitions = chart.scalePartitions([
    line("health", "%", "percent", []),
    line("cpu", "%", "percent", []),
    line("bytes", "B/s", "nonnegative", []),
    line("signed", "%", "signed", []),
  ])
  assert.deepEqual(partitions.map(({ label, scale, seriesIds, unit }) => [label, unit, scale, seriesIds]), [
    ["health / cpu", "%", "percent", ["health", "cpu"]],
    ["bytes", "B/s", "nonnegative", ["bytes"]],
    ["signed", "%", "signed", ["signed"]],
  ])
})

test("tooltip readings use only the exact timestamp without carrying a neighbor", () => {
  const time = chart.createDisplayTimeFormatter("en", "utc", "UTC")
  const series = [
    line("one", "%", "percent", [{ segmentId: "a", timestamp: 1, value: 4 }]),
    line("two", "B/s", "nonnegative", [{ segmentId: "a", timestamp: 2, value: 0 }]),
  ]
  const frame = chart.alignRecordedSeries(series)
  const reading = chart.exactReadings(frame, series, 2, "en", time)
  assert.deepEqual(reading.values.map(({ output }) => output), ["—", "0"])
  assert.equal(chart.nearestRecordedTimestamp([100, 180], 140), 100)
  assert.equal(chart.nearestRecordedTimestamp([100, 180], 141), 180)
  assert.equal(chart.nearestRecordedTimestamp([100, 180, 500, 900], 340), 180)
  assert.equal(chart.nearestRecordedTimestamp([100, 180, 500, 900], 340.1), 500)
  assert.match(chart.sampleText(series, frame, 2, "en", time), /two \(B\/s\): 0/)
})

test("chart time has one active zone and plain seconds precision", () => {
  const timestamp = Date.UTC(2026, 10, 1, 6, 30) * 1_000
  const eastern = chart.createDisplayTimeFormatter("en", "browser", "America/New_York")
  const utc = chart.createDisplayTimeFormatter("en", "utc", "America/New_York")
  assert.equal(eastern.clock(timestamp), "01:30:00")
  assert.equal(utc.clock(timestamp), "06:30:00")
  assert.equal(chart.axisTimeLabel(timestamp, eastern), "01:30")
  assert.doesNotMatch(eastern.clock(timestamp), /\.000|UTC|GMT/)
})

test("tooltip, summary, and keyboard text share bounded semantic precision", () => {
  const timestamp = Date.UTC(2026, 7, 14, 5, 30, 45) * 1_000
  const time = chart.createDisplayTimeFormatter("en", "browser", "Europe/Moscow")
  const series = [{
    ...line("memory", "%", "percent", [{ segmentId: "a", timestamp, value: 41.729068244136855 }]),
    value: chart.humanPercent,
  }]
  const frame = chart.alignRecordedSeries(series)
  assert.equal(chart.exactReadings(frame, series, timestamp, "en", time).values[0].output, "41.7%")
  assert.match(chart.chartSummary(series, frame, timestamp, timestamp + 3_600_000_000, "en", time), /41\.7%…41\.7%/)
  assert.match(chart.sampleText(series, frame, timestamp, "en", time), /08:30:45; memory \(%\): 41\.7%/)
  assert.doesNotMatch(chart.sampleText(series, frame, timestamp, "en", time), /41\.729068|\.000/)
  const ru = chart.createDisplayTimeFormatter("ru", "browser", "Europe/Moscow")
  assert.match(chart.sampleText(series, frame, timestamp, "ru", ru), /41,7 %/)
})
