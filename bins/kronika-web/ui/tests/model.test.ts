import assert from "node:assert/strict"
import test from "node:test"

import type { Cell, DataRow } from "../src/api.ts"
import { fittedWidth } from "../src/column-size.ts"
import { activityFor, compact, formatUtc, identifier, measure, nearestTime, processCommand, processLens, rawText, shownMoment, selectedHour, stateText } from "../src/model.ts"

function row(timestamp: number): DataRow {
  return { segmentId: "7", logicalName: "os_process", typeId: "1100001", ordinal: "0", timestamp, values: {} }
}

test("UTC hour selection and timestamp presentation are exact", () => {
  const hour = selectedHour("2026-02-03", 4)
  assert.equal(hour, Date.UTC(2026, 1, 3, 4) * 1_000)
  const timestamp = Date.UTC(2026, 1, 3, 14, 44, 5, 678) * 1_000 + 901
  assert.equal(formatUtc(timestamp), "2026-02-03 14:44:05.678 UTC")
})

test("nearest stored time chooses the earlier value on a tie", () => {
  assert.equal(nearestTime([row(100), row(300)], 200), 100)
})

test("null is a dash while zero remains data and identifiers are ungrouped", () => {
  assert.equal(measure(null, "en"), "—")
  assert.equal(measure(0, "en"), "0")
  assert.equal(identifier("1234567"), "1234567")
  assert.equal(stateText(82), "R")
})

test("text payloads retain their exact text value", () => {
  const payload: Cell = { representation: "text", stored_text: "SELECT 1", full_len: "8", truncated: true, sha256: "abc" }
  assert.equal(rawText(payload), "SELECT 1")
})

test("integer lists retain exact ungrouped identifiers", () => {
  assert.equal(rawText([1234, 5678]), "[1234,5678]")
})

test("activity linking uses the cursor-nearest PG snapshot and exact PID", () => {
  const process = { ...row(100), values: { pid: 77, comm: "worker", cmdline: null } }
  const activityAt100 = { ...row(100), values: { pid: 77, backend_type: "old" } }
  const activityAt300 = { ...row(300), values: { pid: 77, backend_type: "nearest" } }
  const activityOtherPid = { ...row(300), ordinal: "1", values: { pid: 78, backend_type: "other" } }
  const linked = activityFor(process, [activityAt100, activityAt300, activityOtherPid], 290)
  assert.equal(linked.snapshotTime, 300)
  assert.equal(linked.row?.values.backend_type, "nearest")
  assert.equal(processCommand(process), "worker")
})

test("finding fields select the matching process lens", () => {
  assert.equal(processLens("read_bytes"), "disk")
  assert.equal(processLens("rmem_kb"), "memory")
  assert.equal(processLens("rundelay_ns"), "cpu")
  assert.equal(processLens("state"), "generic")
})

test("counters that arrive as rates are read in units a person thinks in", async () => {
  const model = await import("../src/model.ts")
  assert.equal(model.humanBytes(1_572_864, "en"), "1.5 MiB")
  assert.equal(model.humanBytes(512, "en"), "512 B")
  assert.equal(model.humanBytes(null, "en"), "—")
  assert.equal(model.cores(161.01, "en", 100), "1.61")
  assert.equal(model.cores(161.01, "en", null), "—")
  assert.equal(model.cores(161.01, "en", 0), "—")
  assert.equal(model.millisecondsPerSecond(111_622_111.53, "en"), "111.6")
})

test("the shown moment is the last sample at or before the cursor", () => {
  const row = (timestamp: number) => ({ segmentId: "s", logicalName: "os_cpu", typeId: "1", ordinal: "0", timestamp, values: {} })
  const sections = { os_cpu: [row(10), row(30)], pg_stat_activity: [row(20), row(90)] }

  assert.equal(shownMoment(sections, 50), 30)
  assert.equal(shownMoment(sections, 90), 90)
  assert.equal(shownMoment(sections, 5), null)
  assert.equal(shownMoment({}, 50), null)
})

test("a fitted column takes the widest cell within bounds", () => {
  assert.equal(fittedWidth(200), 212)
  assert.equal(fittedWidth(0), 64)
  assert.equal(fittedWidth(4000), 720)
  assert.equal(fittedWidth(120.2), 133)
})

test("a large count is scaled instead of spelled out digit by digit", () => {
  assert.equal(compact(1407.48, "en"), "1,407.48")
  assert.equal(compact(9999, "en"), "9,999")
  assert.equal(compact(21_471, "en"), "21.5k")
  assert.equal(compact(3_052_945.27, "en"), "3.1M")
  assert.equal(compact(452_000_000, "en"), "452M")
  assert.equal(compact(-21_471, "en"), "-21.5k")
  assert.equal(compact(4.5e12, "en"), "4.5T")
})
