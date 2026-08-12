import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const layouts = [
  layout("1100001", "os_process", ["pid", "starttime"], ["ts", "pid", "starttime", "read_bytes"]),
  layout("1108001", "os_diskstats", ["major", "minor"], ["ts", "major", "minor", "device"]),
  layout("1001003", "pg_stat_activity", [], ["ts", "pid", "backend_start", "query"]),
  layout("1002001", "pg_stat_statements", ["queryid", "userid", "dbid"], ["ts", "queryid", "userid", "dbid", "query", "total_time"]),
  layout("1002003", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], ["ts", "queryid", "userid", "dbid", "toplevel", "query", "total_exec_time"]),
  layout("1003001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], ["ts", "userid", "dbid", "queryid", "planid", "plan", "total_time"]),
  layout("1005001", "pg_stat_database", ["datid"], ["ts", "datid", "datname", "deadlocks"]),
]
const helpers = await importModule('export * from "../src/entity-context.ts"', { plugins: [registryPlugin(layouts)] })

function layout(typeId, logicalName, identity, columns) { return { typeId, logicalName, identity, columns } }
function row(logicalName, typeId, ordinal, values) {
  return { segmentId: "segment", logicalName, typeId, ordinal, timestamp: 100, values }
}
function finding(stored, fieldOrdinal = 1) {
  return { category: null, fieldOrdinal, kind: "known_bad", logicalName: stored.logicalName, rowOrdinal: stored.ordinal, segmentId: stored.segmentId, timestamp: stored.timestamp, typeId: stored.typeId }
}

test("finding routes choose the shared entity lens", () => {
  assert.equal(helpers.findingRoute(finding(row("os_process", "1100001", "1", {}), 3)), "processes")
  assert.equal(helpers.findingRoute(finding(row("pg_stat_statements", "1002003", "1", {}))), "statements")
  assert.equal(helpers.findingRoute(finding(row("pg_store_plans", "1003001", "1", {}))), "plans")
  assert.equal(helpers.findingRoute(finding(row("pg_stat_database", "1005001", "1", {}))), "databases")
  assert.equal(helpers.findingRoute(finding(row("os_diskstats", "1108001", "1", {}))), "system")
})

test("context keeps the complete physical identity and type", () => {
  const process = row("os_process", "1100001", "1", { pid: 41, starttime: "9007199254740997", read_bytes: 3 })
  const oldStatement = row("pg_stat_statements", "1002001", "2", { queryid: "9", userid: "10", dbid: "11", query: "select 1" })
  const newStatement = row("pg_stat_statements", "1002003", "3", { queryid: "9", userid: "10", dbid: "11", toplevel: false, query: "select 1" })
  const plan = row("pg_store_plans", "1003001", "4", { userid: "10", dbid: "11", queryid: "9", planid: "12", plan: "Scan t" })
  const database = row("pg_stat_database", "1005001", "5", { datid: "16384", datname: "app" })
  const device = row("os_diskstats", "1108001", "6", { major: 8, minor: 1, device: "sda1" })
  const activity = row("pg_stat_activity", "1001003", "7", { pid: 42, backend_start: "9007199254740999", query: "select 1" })

  const processContext = helpers.entityContext(finding(process), process)
  assert.equal(processContext.label, "PID 41")
  assert.deepEqual(processContext.identity, [["pid", "41"], ["starttime", "9007199254740997"]])
  assert.deepEqual(helpers.entityContext(finding(oldStatement), oldStatement).identity, [["queryid", "9"], ["userid", "10"], ["dbid", "11"]])
  const statementContext = helpers.entityContext(finding(newStatement), newStatement)
  assert.equal(statementContext.typeId, "1002003")
  assert.deepEqual(statementContext.identity, [["queryid", "9"], ["userid", "10"], ["dbid", "11"], ["toplevel", "false"]])
  assert.deepEqual(helpers.entityContext(finding(plan), plan).identity, [["userid", "10"], ["dbid", "11"], ["queryid", "9"], ["planid", "12"]])
  assert.deepEqual(helpers.entityContext(finding(database), database).identity, [["datid", "16384"]])
  assert.deepEqual(helpers.entityContext(finding(device), device).identity, [["major", "8"], ["minor", "1"]])
  const activityContext = helpers.entityContext(finding(activity), activity)
  assert.deepEqual(activityContext.identity, [["pid", "42"], ["backend_start", "9007199254740999"]])
  assert.equal(helpers.contextMatches({ ...activity, values: { ...activity.values, backend_start: "9007199254741000" } }, activityContext), false)
  assert.equal(helpers.contextMatches({ ...activity, timestamp: 200 }, activityContext), true)
})

test("context filtering injects an omitted exact row without changing page order", () => {
  const exact = row("os_process", "1100001", "9", { pid: 41, starttime: "100", read_bytes: 3 })
  const context = helpers.entityContext(finding(exact), exact)
  const page = [
    row("os_process", "1100001", "1", { pid: 8, starttime: "80" }),
    row("os_process", "1100001", "2", { pid: 9, starttime: "90" }),
  ]
  assert.deepEqual(helpers.contextualRows(page, context, exact).map(({ ordinal }) => ordinal), ["9"])
  assert.deepEqual(page.map(({ ordinal }) => ordinal), ["1", "2"])
  assert.equal(helpers.contextualRows(page, null, exact), page)
  const present = [page[0], exact, page[1]]
  assert.deepEqual(helpers.contextualRows(present, context, exact).map(({ ordinal }) => ordinal), ["9"])
})

test("exact injection is limited to the finding cursor and never duplicates a current entity", () => {
  const exact = row("os_process", "1100001", "9", { pid: 41, starttime: "100" })
  const context = helpers.entityContext(finding(exact), exact)
  const current = { ...exact, ordinal: "10", timestamp: 200 }
  assert.notEqual(finding(exact).timestamp, 200)
  assert.equal(finding(exact).timestamp, 100)
  assert.deepEqual(helpers.contextualRows([current], context, exact), [current])
  assert.deepEqual(helpers.contextualRows([], context, null), [])
})

test("a dense context exposes its exact server filters", () => {
  const exact = row("pg_stat_statements", "1002001", "2", { queryid: "9", userid: "10", dbid: "11", query: "select 1" })
  const context = helpers.entityContext(finding(exact), exact)
  assert.deepEqual(Object.fromEntries(context.identity), { queryid: "9", userid: "10", dbid: "11" })
})
