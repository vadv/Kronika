import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const locks = await importModule('export { decorateLockGraph } from "../src/postgres-locks.ts"')

const HOUR = 1_780_000_000_000_000

function row(pid, parent, depth, order, extra = [], prepared = false) {
  return {
    logicalName: "pg_locks", ordinal: String(pid), segmentId: "a",
    timestamp: HOUR, typeId: "1011002",
    values: {
      pid,
      blocked_by: parent === null ? [] : [parent],
      lock_tree_parent_pid: parent,
      lock_tree_depth: depth,
      lock_tree_order: order,
      lock_tree_extra_blockers: extra,
      lock_tree_waits_on_prepared: prepared,
    },
  }
}

test("lock decoration preserves canonical rows and adds chain connectors only", () => {
  const rows = [row(70, null, 1, 0), row(76, 70, 2, 1), row(77, 76, 3, 2)]
  const decorated = locks.decorateLockGraph(rows)

  assert.deepEqual(decorated.map((entry) => entry.values.pid), [70, 76, 77])
  assert.deepEqual(decorated.map((entry) => entry.values.lock_tree_prefix), ["", "└─ ", "   └─ "])
  for (const [index, entry] of decorated.entries()) {
    assert.deepEqual(
      { ...entry.values, lock_tree_prefix: undefined },
      { ...rows[index].values, lock_tree_prefix: undefined },
    )
  }
})

test("lock decoration draws sibling and continuing-branch connectors from canonical parents", () => {
  const decorated = locks.decorateLockGraph([
    row(1, null, 1, 0),
    row(2, 1, 2, 1),
    row(4, 2, 3, 2, [9], true),
    row(3, 1, 2, 3),
  ])

  assert.deepEqual(decorated.map((entry) => entry.values.lock_tree_prefix), ["", "├─ ", "│  └─ ", "└─ "])
  assert.deepEqual(decorated[2].values.lock_tree_extra_blockers, [9])
  assert.equal(decorated[2].values.lock_tree_waits_on_prepared, true)
  assert.equal(decorated[2].values.lock_tree_order, 2)
})

test("lock presentation contains no second graph builder or graph filter", async () => {
  const source = await readFile(new URL("../src/postgres-locks.ts", import.meta.url), "utf8")
  assert.doesNotMatch(source, /blocked_by|buildLockForest|filterLockForest|parseSearch|rowMatchesSearch/)
})
