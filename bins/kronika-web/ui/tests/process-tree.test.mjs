import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const tree = await importModule('export { buildProcessForest } from "../src/process-tree.ts"')

const HOUR = 1_780_000_000_000_000

let ordinal = 0
function row(pid, ppid, extra = {}) {
  ordinal += 1
  return {
    logicalName: "os_process", ordinal: String(ordinal), segmentId: "a",
    timestamp: HOUR, typeId: "1100001",
    values: { pid, ppid, comm: "worker", ...extra },
  }
}

function pids(forest) { return forest.map((r) => r.values.pid) }

test("a simple parent/child chain walks the parent first, each child directly under it", () => {
  const rows = [row(3, 2), row(1, 0), row(2, 1)]
  const forest = tree.buildProcessForest(rows, HOUR, 100, 2_048_000)
  assert.deepEqual(pids(forest), [1, 2, 3])
  assert.equal(forest[0].values.process_tree_prefix, "")
  assert.equal(forest[1].values.process_tree_prefix, "└─ ")
  assert.equal(forest[2].values.process_tree_prefix, "   └─ ")
})

test("two children of the same parent are siblings, only the last gets the closing connector", () => {
  const rows = [row(1, 0), row(2, 1), row(3, 1)]
  const forest = tree.buildProcessForest(rows, HOUR, 100, 2_048_000)
  assert.deepEqual(pids(forest), [1, 2, 3])
  assert.equal(forest[1].values.process_tree_prefix, "├─ ")
  assert.equal(forest[2].values.process_tree_prefix, "└─ ")
})

test("a continuing ancestor branch draws a vertical bar, not blank indentation", () => {
  const rows = [row(1, 0), row(2, 1), row(3, 1), row(4, 2)]
  const forest = tree.buildProcessForest(rows, HOUR, 100, 2_048_000)
  assert.deepEqual(pids(forest), [1, 2, 4, 3])
  assert.equal(forest[2].values.process_tree_prefix, "│  └─ ")
})

test("two unrelated process trees (different sessions/services) stay separate and ordered by root pid", () => {
  const rows = [row(50, 10), row(10, 0), row(1, 0), row(2, 1)]
  const forest = tree.buildProcessForest(rows, HOUR, 100, 2_048_000)
  assert.deepEqual(pids(forest), [1, 2, 10, 50])
})

test("a pid whose recorded parent is not in this snapshot renders as its own root, not dropped", () => {
  const rows = [row(2, 1)] // pid 1 never arrived in this page
  const forest = tree.buildProcessForest(rows, HOUR, 100, 2_048_000)
  assert.deepEqual(pids(forest), [2])
  assert.equal(forest[0].values.process_tree_prefix, "")
})

test("a two-process cycle still renders both processes, rooted at the lower pid, without looping", () => {
  const rows = [row(20, 10), row(10, 20)]
  const forest = tree.buildProcessForest(rows, HOUR, 100, 2_048_000)
  assert.deepEqual(pids(forest).slice().sort((left, right) => left - right), [10, 20])
  assert.equal(forest.length, 2)
})

test("%CPU is scheduled time over wall time since starttime, %MEM is resident over host total, TIME is scheduled seconds", () => {
  const CURSOR = HOUR + 60_000_000 // one minute after starttime
  const rows = [row(1, 0, { utime: 300, stime: 300, starttime: HOUR, rmem_kb: 512_000 })]
  const [annotated] = tree.buildProcessForest(rows, CURSOR, 100, 2_048_000)
  assert.equal(annotated.values.cpu_time_seconds, 6) // (300+300)/100 ticks-per-second
  assert.equal(annotated.values.cpu_percent, 10) // 6s scheduled / 60s elapsed * 100
  assert.equal(annotated.values.mem_percent, 25) // 512000/2048000 * 100
})

test("a missing input yields a null metric instead of a wrong number", () => {
  const rows = [row(1, 0, { utime: null, stime: 300, starttime: HOUR, rmem_kb: 512_000 })]
  const [withoutUtime] = tree.buildProcessForest(rows, HOUR + 1_000_000, 100, 2_048_000)
  assert.equal(withoutUtime.values.cpu_time_seconds, null)
  assert.equal(withoutUtime.values.cpu_percent, null)

  const [withoutTicks] = tree.buildProcessForest(rows, HOUR + 1_000_000, null, 2_048_000)
  assert.equal(withoutTicks.values.cpu_percent, null)

  const [withoutMemTotal] = tree.buildProcessForest(rows, HOUR + 1_000_000, 100, null)
  assert.equal(withoutMemTotal.values.mem_percent, null)
})
