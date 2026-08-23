import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const tree = await importModule('export { buildProcessForest, filterProcessForest, scheduledTicks } from "../src/process-tree.ts"')

const HOUR = 1_780_000_000_000_000
const SHAPE_ONLY = { intervalSeconds: null, memTotalKb: null, previousTicks: new Map(), ticksPerSecond: 100 }

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
  const forest = tree.buildProcessForest(rows, SHAPE_ONLY)
  assert.deepEqual(pids(forest), [1, 2, 3])
  assert.equal(forest[0].values.process_tree_prefix, "")
  assert.equal(forest[0].values.process_tree_depth, 0)
  assert.equal(forest[0].values.process_tree_parent_pid, null)
  assert.equal(forest[1].values.process_tree_prefix, "└─ ")
  assert.equal(forest[1].values.process_tree_parent_pid, 1)
  assert.equal(forest[2].values.process_tree_prefix, "   └─ ")
  assert.equal(forest[2].values.process_tree_depth, 2)
})

test("two children of the same parent are siblings, only the last gets the closing connector", () => {
  const rows = [row(1, 0), row(2, 1), row(3, 1)]
  const forest = tree.buildProcessForest(rows, SHAPE_ONLY)
  assert.deepEqual(pids(forest), [1, 2, 3])
  assert.equal(forest[1].values.process_tree_prefix, "├─ ")
  assert.equal(forest[2].values.process_tree_prefix, "└─ ")
})

test("a continuing ancestor branch draws a vertical bar, not blank indentation", () => {
  const rows = [row(1, 0), row(2, 1), row(3, 1), row(4, 2)]
  const forest = tree.buildProcessForest(rows, SHAPE_ONLY)
  assert.deepEqual(pids(forest), [1, 2, 4, 3])
  assert.equal(forest[2].values.process_tree_prefix, "│  └─ ")
})

test("two unrelated process trees (different sessions/services) stay separate and ordered by root pid", () => {
  const rows = [row(50, 10), row(10, 0), row(1, 0), row(2, 1)]
  const forest = tree.buildProcessForest(rows, SHAPE_ONLY)
  assert.deepEqual(pids(forest), [1, 2, 10, 50])
})

test("a pid whose recorded parent is not in this snapshot renders as its own root, not dropped", () => {
  const rows = [row(2, 1)]
  const forest = tree.buildProcessForest(rows, SHAPE_ONLY)
  assert.deepEqual(pids(forest), [2])
  assert.equal(forest[0].values.process_tree_prefix, "")
})

test("a two-process cycle still renders both processes, rooted at the lower pid, without looping", () => {
  const rows = [row(20, 10), row(10, 20)]
  const forest = tree.buildProcessForest(rows, SHAPE_ONLY)
  assert.deepEqual(pids(forest).slice().sort((left, right) => left - right), [10, 20])
  assert.equal(forest.length, 2)
})

test("%CPU is the share of one core burned since the previous snapshot, the way top counts it", () => {
  // 600 ticks / 100 Hz / 12 seconds = 50%.
  const rows = [row(1, 0, { utime: 900, stime: 300, rmem_kb: 512_000 })]
  const previousTicks = new Map([[1, 600]])
  const [busy] = tree.buildProcessForest(rows, { intervalSeconds: 12, memTotalKb: 2_048_000, previousTicks, ticksPerSecond: 100 })
  assert.equal(busy.values.cpu_percent, 50)
  assert.equal(busy.values.cpu_time_seconds, 12)
  assert.equal(busy.values.mem_percent, 25)

  const [idle] = tree.buildProcessForest(rows, { intervalSeconds: 12, memTotalKb: null, previousTicks: new Map([[1, 1_200]]), ticksPerSecond: 100 })
  assert.equal(idle.values.cpu_percent, 0)
})

test("without a usable preceding sample %CPU is missing rather than guessed", () => {
  const rows = [row(1, 0, { utime: 900, stime: 300, rmem_kb: 512_000 })]
  const inputs = (extra) => ({ intervalSeconds: 12, memTotalKb: 2_048_000, previousTicks: new Map([[1, 600]]), ticksPerSecond: 100, ...extra })
  assert.equal(tree.buildProcessForest(rows, inputs({ previousTicks: new Map() }))[0].values.cpu_percent, null)
  assert.equal(tree.buildProcessForest(rows, inputs({ intervalSeconds: null }))[0].values.cpu_percent, null)
  assert.equal(tree.buildProcessForest(rows, inputs({ intervalSeconds: 0 }))[0].values.cpu_percent, null)
  assert.equal(tree.buildProcessForest(rows, inputs({ ticksPerSecond: null }))[0].values.cpu_percent, null)
  assert.equal(tree.buildProcessForest(rows, inputs({ previousTicks: new Map([[1, 9_999]]) }))[0].values.cpu_percent, null)
  assert.equal(tree.buildProcessForest(rows, inputs({ intervalSeconds: null }))[0].values.mem_percent, 25)
})

test("scheduled ticks are the sum the delta is taken on, and missing either half is unusable", () => {
  const at = (values) => tree.scheduledTicks(row(1, 0, values))
  assert.equal(at({ utime: 900, stime: 300 }), 1_200)
  assert.equal(at({ utime: null, stime: 300 }), null)
  assert.equal(at({ utime: 900 }), null)
})

test("Tree search keeps matched rows with their parent chain", () => {
  const forest = tree.buildProcessForest([
    row(1, 0, { cmdline: "/sbin/init", rmem_kb: 512, user: "root" }),
    row(2, 1, { cmdline: "postgres", rmem_kb: 4_096, user: "postgres" }),
    row(3, 2, { cmdline: "autovacuum worker", rmem_kb: 1_024, user: "postgres" }),
    row(4, 1, { cmdline: "nginx", rmem_kb: 768, user: "www-data" }),
  ], SHAPE_ONLY)
  assert.deepEqual(pids(tree.filterProcessForest(forest, "autovacuum", 100)), [1, 2, 3])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "user:postgres", 100)), [1, 2, 3])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "rss>2MiB", 100)), [1, 2])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "pid:4 OR rss>2MiB", 100)), [1, 2, 4])
})

test("Tree numeric search uses API rate units", () => {
  const forest = tree.buildProcessForest([
    row(1, 0, { blkdelay_ticks: 25, read_bytes: 2_000_000, rundelay_ns: 40_000_000, stime: 10, utime: 20 }),
    row(2, 1, { blkdelay_ticks: 1, read_bytes: 100, rundelay_ns: 1_000, stime: 1, utime: 1 }),
  ], SHAPE_ONLY)
  assert.deepEqual(pids(tree.filterProcessForest(forest, "cpu_cores>0.2 AND disk_read_rate>1MB/s", 100)), [1])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "run_delay>30ms/s", 100)), [1])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "block_io_delay>200ms/s", 100)), [1])
})
