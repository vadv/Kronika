import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const tree = await importModule('export { decorateProcessTree } from "../src/process-tree.ts"')

const HOUR = 1_780_000_000_000_000
const CLOCK_HZ = 100

let ordinal = 0
function row(pid, parent, depth, order, extra = {}) {
  ordinal += 1
  return {
    logicalName: "os_process", ordinal: String(ordinal), segmentId: "a",
    timestamp: HOUR, typeId: "1100001",
    values: {
      pid, comm: "worker", process_tree_depth: depth,
      process_tree_order: order, process_tree_parent_pid: parent, ...extra,
    },
  }
}

test("tree decoration preserves the product row order and canonical structure", () => {
  const rows = [
    row(1, null, 0, 0),
    row(2, 1, 1, 1),
    row(4, 2, 2, 2),
    row(3, 1, 1, 3),
  ]
  const decorated = tree.decorateProcessTree(rows, null, CLOCK_HZ)
  assert.deepEqual(decorated.map((entry) => entry.values.pid), [1, 2, 4, 3])
  assert.deepEqual(decorated.map((entry) => entry.values.process_tree_order), [0, 1, 2, 3])
  assert.deepEqual(decorated.map((entry) => entry.values.process_tree_parent_pid), [null, 1, 2, 1])
  assert.deepEqual(decorated.map((entry) => entry.values.process_tree_depth), [0, 1, 2, 1])
  assert.deepEqual(decorated.map((entry) => entry.values.process_tree_prefix), ["", " \\_ ", " |   \\_ ", " \\_ "])
})

test("a last ancestor branch uses blank ps indentation", () => {
  const decorated = tree.decorateProcessTree([
    row(1, null, 0, 0),
    row(2, 1, 1, 1),
    row(3, 2, 2, 2),
  ], null, CLOCK_HZ)
  assert.equal(decorated[2].values.process_tree_prefix, "     \\_ ")
})

test("%CPU and %MEM retain their ps presentation semantics", () => {
  const [busy] = tree.decorateProcessTree([
    row(1, null, 0, 0, { utime: 40, stime: 10, rmem_kb: 512_000 }),
  ], 2_048_000, CLOCK_HZ)
  assert.equal(busy.values.cpu_percent, 50)
  assert.equal(busy.values.mem_percent, 25)
})

test("%CPU is missing rather than guessed when a half or scale is missing", () => {
  const at = (values, ticksPerSecond = CLOCK_HZ) => tree.decorateProcessTree([
    row(1, null, 0, 0, values),
  ], null, ticksPerSecond)[0].values.cpu_percent
  assert.equal(at({ utime: 0, stime: 0 }), 0)
  assert.equal(at({ utime: 40 }), null)
  assert.equal(at({ utime: null, stime: 10 }), null)
  assert.equal(at({ utime: 40, stime: 10 }, null), null)
  assert.equal(at({ utime: 40, stime: 10 }, 0), null)
})

test("STAT carries the state letter and the flags the record can tell", () => {
  const stat = (values) => tree.decorateProcessTree([row(1, null, 0, 0, values)], null, CLOCK_HZ)[0].values.process_stat
  const sleeping = "S".charCodeAt(0)
  assert.equal(stat({ state: sleeping, nice: 0, num_threads: 1 }), "S")
  assert.equal(stat({ state: sleeping, nice: -5, num_threads: 1 }), "S<")
  assert.equal(stat({ state: sleeping, nice: 10, num_threads: 1 }), "SN")
  assert.equal(stat({ state: sleeping, nice: 0, num_threads: 20 }), "Sl")
  assert.equal(stat({ state: sleeping, nice: -5, num_threads: 20 }), "S<l")
  assert.equal(stat({ state: "R".charCodeAt(0) }), "R")
  assert.equal(stat({ nice: -5 }), null)
})

test("TIME is the recorded lifetime CPU counter against the clock rate", () => {
  const seconds = (values, ticksPerSecond = CLOCK_HZ) => tree.decorateProcessTree([
    row(1, null, 0, 0, values),
  ], null, ticksPerSecond)[0].values.cpu_time_seconds
  assert.equal(seconds({ cpu_time_ticks: 27_618 }), 276.18)
  assert.equal(seconds({ cpu_time_ticks: 0 }), 0)
  assert.equal(seconds({ utime: 40, stime: 10 }), null)
  assert.equal(seconds({ cpu_time_ticks: 27_618 }, null), null)
  assert.equal(seconds({ cpu_time_ticks: 27_618 }, 0), null)
})

test("the UI has no private tree construction or search reducer", async () => {
  const source = await readFile(new URL("../src/process-tree.ts", import.meta.url), "utf8")
  assert.doesNotMatch(source, /buildProcessForest|filterProcessForest|rowMatchesSearch|parseSearch/)
})
