import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const statementIdentity = ["queryid", "userid", "dbid"]
const statementFields = ["ts", ...statementIdentity, "query", "calls", "rows", "total_exec_time", "blk_read_time"]
const planIdentity = ["userid", "dbid", "queryid", "planid"]
const registry = [
  layout("1100001", "os_process", ["pid"], ["ts", "pid", "read_bytes"]),
  layout("1002001", "pg_stat_statements", statementIdentity, ["ts", ...statementIdentity, "query", "calls", "rows", "total_time", "blk_read_time"]),
  layout("1002002", "pg_stat_statements", statementIdentity, statementFields),
  layout("1002003", "pg_stat_statements", [...statementIdentity, "toplevel"], [...statementFields, "toplevel"]),
  layout("1002004", "pg_stat_statements", [...statementIdentity, "toplevel"], [...statementFields, "toplevel", "temp_blk_read_time"]),
  layout("1002005", "pg_stat_statements", [...statementIdentity, "toplevel"], [...statementFields.filter((field) => field !== "blk_read_time"), "toplevel", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time"]),
  layout("1002006", "pg_stat_statements", [...statementIdentity, "toplevel"], [...statementFields.filter((field) => field !== "blk_read_time"), "toplevel", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time", "wal_buffers_full"]),
  layout("1003001", "pg_store_plans", planIdentity, ["ts", ...planIdentity, "plan", "calls", "total_time", "shared_blk_read_time"]),
  layout("1004001", "pg_store_plans", planIdentity, ["ts", ...planIdentity, "plan", "calls", "total_time", "blk_read_time", "total_plan_time", "queryid_stat_statements"]),
  layout("1018001", "pg_store_plans", planIdentity, ["ts", ...planIdentity, "plan", "calls", "total_time", "shared_blk_read_time", "relids", "cmd_type"]),
  layout("1016001", "pg_store_plans_info", [], ["ts", "dealloc", "stats_reset"]),
]
const helpers = await importModule(
  'export { locatorMatchesColumn, nextServerOrder } from "../src/entity-table.tsx"; export { rowMatchesLocator } from "../src/locator.ts"; export { PLAN_COLUMNS, STATEMENT_COLUMNS } from "../src/postgres-view.tsx"',
  { plugins: [registryPlugin(registry)] },
)

function layout(typeId, logicalName, identity, fields) {
  return { typeId, logicalName, identity, columns: [...new Set(fields)] }
}

const row = { segmentId: "segment-a", logicalName: "os_process", typeId: "1100001", ordinal: "7", timestamp: 100, values: { pid: 9, read_bytes: 12 } }
const finding = { segmentId: "segment-a", logicalName: "os_process", typeId: "1100001", rowOrdinal: "7", timestamp: 100, fieldOrdinal: 2, kind: "spike", category: null }

test("physical locators match the exact loaded row and mapped cell", () => {
  assert.equal(helpers.rowMatchesLocator(row, finding), true)
  assert.equal(helpers.rowMatchesLocator({ ...row, timestamp: 101 }, finding), false)
  assert.equal(helpers.rowMatchesLocator({ ...row, ordinal: "8" }, finding), false)
  assert.equal(helpers.locatorMatchesColumn({ field: "read_rate", label: "Read", physicalField: { "1100001": "read_bytes" } }, row.typeId, "read_bytes"), true)
  assert.equal(helpers.locatorMatchesColumn({ field: "write_bytes", label: "Write" }, row.typeId, "read_bytes"), false)
})

test("statement execution findings select the interval mean cell", () => {
  const mean = helpers.STATEMENT_COLUMNS.find((column) => column.field === "mean_exec_ms_per_call")
  const demand = helpers.STATEMENT_COLUMNS.find((column) => column.field === "execution_ms_per_second")
  assert.notEqual(mean, undefined)
  assert.notEqual(demand, undefined)
  assert.equal(helpers.locatorMatchesColumn(mean, "1002001", "total_time"), true)
  for (const typeId of ["1002002", "1002003", "1002004", "1002005", "1002006"]) {
    assert.equal(helpers.locatorMatchesColumn(mean, typeId, "total_exec_time"), true)
  }
  assert.equal(helpers.locatorMatchesColumn(demand, "1002002", "total_exec_time"), false)

  const planMean = helpers.PLAN_COLUMNS.find((column) => column.field === "mean_exec_ms_per_call")
  assert.notEqual(planMean, undefined)
  for (const typeId of ["1003001", "1004001", "1018001"]) {
    assert.equal(helpers.locatorMatchesColumn(planMean, typeId, "total_time"), true)
  }
})

test("locator classes, scrolling, and selection state are independent", async () => {
  const entity = await readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8")
  const process = await readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8")
  assert.match(entity, /aria-selected=/)
  assert.match(entity, /locator-row/)
  assert.match(entity, /locator-cell/)
  assert.match(entity, /scrollToIndex\(locatedIndex/)
  assert.match(process, /<EntityTable/)
  assert.doesNotMatch(process, /useReactTable|useVirtualizer|locator-row/)
})

test("server-ranked tables offer only descending order or no order", () => {
  assert.deepEqual(helpers.nextServerOrder(undefined, "calls_per_second"), {
    column: "calls_per_second",
    descending: true,
  })
  assert.equal(helpers.nextServerOrder({ column: "calls_per_second", descending: true }, "calls_per_second"), null)
  assert.deepEqual(helpers.nextServerOrder({ column: "calls_per_second", descending: false }, "calls_per_second"), {
    column: "calls_per_second",
    descending: true,
  })
  assert.deepEqual(helpers.nextServerOrder({ column: "calls_per_second", descending: true }, "rows_per_second"), {
    column: "rows_per_second",
    descending: true,
  })
})
