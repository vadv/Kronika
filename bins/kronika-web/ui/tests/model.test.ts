import assert from "node:assert/strict"
import test from "node:test"

import type { Cell, DataRow } from "../src/api.ts"
import { activityFor, formatUtc, identifier, measure, nearestTime, payloadMeta, processCommand, rawText, selectedHour, systemSnapshots } from "../src/model.ts"

function row(timestamp: number): DataRow {
  return { segmentId: "7", typeId: "1100001", ordinal: "0", timestamp, values: {} }
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
})

test("text payloads retain their stored value and truncation metadata", () => {
  const payload: Cell = { representation: "text", stored_text: "SELECT 1", full_len: "8", truncated: true, sha256: "abc" }
  assert.equal(rawText(payload), "SELECT 1")
  assert.equal(payloadMeta(payload), "truncated · full_len=8 · sha256=abc")
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

test("system snapshot rows preserve exact timestamps, nulls and zero", () => {
  const health = [{ ...row(100), values: { os_health: 0 } }]
  const load = [{ ...row(200), values: { load1: 1.5, load5: null, load15: 0 } }]
  const snapshots = systemSnapshots(health, load, [], [])
  assert.deepEqual(snapshots.map((snapshot) => snapshot.timestamp), [100, 200])
  assert.equal(snapshots[0]?.health, 0)
  assert.equal(snapshots[0]?.load1, null)
  assert.equal(snapshots[1]?.load15, 0)
})
