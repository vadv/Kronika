import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const BLOCK_COUNTERS = [
  "shared_blks_hit", "shared_blks_read", "shared_blks_dirtied", "shared_blks_written",
  "local_blks_hit", "local_blks_read", "local_blks_dirtied", "local_blks_written",
  "temp_blks_read", "temp_blks_written",
]
const STATEMENT_BASE = ["ts", "queryid", "userid", "dbid", "datname", "usename", "query", "calls", "rows", ...BLOCK_COUNTERS]
const STATEMENT_V2 = [...STATEMENT_BASE, "plans", "total_exec_time", "total_plan_time", "blk_read_time", "blk_write_time", "wal_records", "wal_fpi", "wal_bytes"]
const PLAN_BASE = ["ts", "userid", "dbid", "queryid", "planid", "datname", "usename", "plan", "calls", "total_time", "rows", ...BLOCK_COUNTERS]
const TEST_REGISTRY = [
  layout("1002001", "pg_stat_statements", ["queryid", "userid", "dbid"], [...STATEMENT_BASE, "total_time", "blk_read_time", "blk_write_time"]),
  layout("1002002", "pg_stat_statements", ["queryid", "userid", "dbid"], STATEMENT_V2),
  layout("1002003", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], [...STATEMENT_V2, "toplevel"]),
  layout("1002004", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], [...STATEMENT_V2, "toplevel", "temp_blk_read_time", "temp_blk_write_time"]),
  layout("1002005", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], statementV5()),
  layout("1002006", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], [...statementV5(), "wal_buffers_full"]),
  layout("1003001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], [...PLAN_BASE, "shared_blk_read_time", "shared_blk_write_time", "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time"]),
  layout("1004001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], [...PLAN_BASE, "queryid_stat_statements", "slow_log_calls", "blk_read_time", "blk_write_time", "total_plan_time"]),
  layout("1018001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], [...PLAN_BASE, "relids", "cmd_type", "shared_blk_read_time", "shared_blk_write_time", "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time"]),
  layout("1016001", "pg_store_plans_info", [], ["ts", "dealloc", "stats_reset"]),
]
const compiled = await build({
  bundle: true,
  format: "esm",
  platform: "node",
  plugins: [{
    name: "registry",
    setup(context) {
      context.onResolve({ filter: /^kronika:registry$/ }, () => ({ namespace: "registry", path: "registry" }))
      context.onLoad({ filter: /.*/, namespace: "registry" }, () => ({ contents: `export const registry=${JSON.stringify(TEST_REGISTRY)}` }))
    },
  }],
  stdin: {
    contents: 'export { ACTIVITY_COLUMNS, columnsFor, isTimestampField, overviewValue, PLAN_COLUMNS, postgresDatabaseCount, sameEntity, selectedEntity, STATEMENT_COLUMNS } from "../src/postgres-view.tsx"; export { decoratePostgresIntervalRow, findingSemanticField, physicalField, postgresIdentity, postgresProjection } from "../src/postgres-metrics.ts"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

function layout(typeId, logicalName, identity, fields) {
  return { typeId, logicalName, identity, columns: [...new Set(fields)].map((name) => ({ name })) }
}

function statementV5() {
  return [
    ...STATEMENT_V2.filter((field) => field !== "blk_read_time" && field !== "blk_write_time"),
    "toplevel", "shared_blk_read_time", "shared_blk_write_time", "local_blk_read_time", "local_blk_write_time",
    "temp_blk_read_time", "temp_blk_write_time", "stats_since",
  ]
}

function row(typeId, values, logicalName = "pg_stat_statements") {
  return { logicalName, ordinal: "0", segmentId: "a", timestamp: 1, typeId, values }
}

test("PostgreSQL durations are not formatted as Unix timestamps", () => {
  assert.equal(helpers.isTimestampField("write_time"), false)
  assert.equal(helpers.isTimestampField("stats_reset"), true)
  assert.equal(helpers.isTimestampField("last_archived_time"), true)
  assert.equal(helpers.overviewValue(123.4, "write_time", "en"), "123.4 ms")
  assert.equal(helpers.overviewValue(123.4, "max_age_us", "en"), "123.4 μs")
  assert.equal(helpers.overviewValue(123.4, "wal_bytes", "en"), "123.4 B")
  assert.equal(helpers.overviewValue(true, "datallowconn", "ru"), "да")
  assert.equal(helpers.columnsFor([row("1", { write_time: 123.4 })])[0].kind, "milliseconds")
  assert.equal(helpers.columnsFor([row("1", { max_age_us: 123.4 })])[0].kind, "microseconds")
})

test("activity keeps an explicit compact sticky PID header", () => {
  assert.deepEqual(
    helpers.ACTIVITY_COLUMNS[0],
    { field: "pid", help: "pg.field.pid.help", kind: "id", label: "pg.field.pid.label", sticky: true, width: 78 },
  )
})

test("all statement layouts use their registered identity and timing names", () => {
  const cases = [
    ["1002001", ["queryid", "userid", "dbid"], "total_time", "blk_read_time", null, null],
    ["1002002", ["queryid", "userid", "dbid"], "total_exec_time", "blk_read_time", null, null],
    ["1002003", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "blk_read_time", null, null],
    ["1002004", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "blk_read_time", null, "temp_blk_read_time"],
    ["1002005", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time"],
    ["1002006", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time"],
  ]
  for (const [typeId, identity, execution, sharedRead, localRead, tempRead] of cases) {
    assert.deepEqual(helpers.postgresIdentity(typeId), identity)
    assert.equal(helpers.physicalField(typeId, "execution_ms_per_second"), execution)
    assert.equal(helpers.physicalField(typeId, "shared_blk_read_ms_per_second"), sharedRead)
    assert.equal(helpers.physicalField(typeId, "local_blk_read_ms_per_second"), localRead)
    assert.equal(helpers.physicalField(typeId, "temp_blk_read_ms_per_second"), tempRead)
    const projection = helpers.postgresProjection(typeId)
    assert.deepEqual(projection.slice(0, identity.length), identity)
    assert.equal(projection.includes(execution), true)
    const decorated = helpers.decoratePostgresIntervalRow(row(typeId, { calls: 4, rows: 12, [execution]: 20, [sharedRead]: 8 }))
    assert.equal(decorated.values.calls_per_second, 4)
    assert.equal(decorated.values.execution_ms_per_second, 20)
    assert.equal(decorated.values.mean_exec_ms_per_call, 5)
    assert.equal(decorated.values.rows_per_second, 12)
    assert.equal(decorated.values.shared_blk_read_ms_per_second, 8)
  }

  for (const typeId of ["1002001", "1002002"]) {
    assert.equal(helpers.postgresProjection(typeId).includes("toplevel"), false)
    const base = row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: true })
    assert.equal(helpers.sameEntity(base, row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: false }), "pg_stat_statements"), true)
  }
  for (const typeId of ["1002003", "1002004", "1002005", "1002006"]) {
    const base = row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: true })
    assert.equal(helpers.sameEntity(base, row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: false }), "pg_stat_statements"), false)
  }
  assert.equal(helpers.sameEntity(row("1002001", { queryid: 7, userid: 8, dbid: 9 }), row("1002002", { queryid: 7, userid: 8, dbid: 9 }), "pg_stat_statements"), false)
})

test("plan layouts expose plan identity and only their available semantics", () => {
  const identity = ["userid", "dbid", "queryid", "planid"]
  const cases = [
    ["1003001", "shared_blk_read_time", null, false],
    ["1004001", "blk_read_time", "total_plan_time", true],
    ["1018001", "shared_blk_read_time", null, false],
  ]
  for (const [typeId, sharedRead, planning, lastAttribution] of cases) {
    assert.deepEqual(helpers.postgresIdentity(typeId), identity)
    assert.equal(helpers.physicalField(typeId, "execution_ms_per_second"), "total_time")
    assert.equal(helpers.physicalField(typeId, "shared_blk_read_ms_per_second"), sharedRead)
    assert.equal(helpers.physicalField(typeId, "planning_ms_per_second"), planning)
    assert.equal(helpers.postgresProjection(typeId).includes("queryid_stat_statements"), lastAttribution)
    const decorated = helpers.decoratePostgresIntervalRow(row(typeId, { calls: 2, total_time: 18 }, "pg_store_plans"))
    assert.equal(decorated.values.execution_ms_per_second, 18)
    assert.equal(decorated.values.mean_exec_ms_per_call, 9)
  }
  assert.deepEqual(helpers.postgresProjection("1016001"), ["dealloc", "stats_reset"])
  assert.equal(helpers.PLAN_COLUMNS.at(-1)?.field, "queryid_stat_statements")
})

test("dense PostgreSQL columns and the Plans tab stay available by section", async () => {
  const statementFields = helpers.STATEMENT_COLUMNS.map((column) => column.field)
  const planFields = helpers.PLAN_COLUMNS.map((column) => column.field)
  for (const field of ["calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second", "shared_blks_read", "wal_bytes", "planning_ms_per_second"]) {
    assert.equal(statementFields.includes(field), true)
  }
  for (const field of ["planid", "queryid", "plan", "calls_per_second", "execution_ms_per_second", "rows_per_second", "queryid_stat_statements"]) {
    assert.equal(planFields.includes(field), true)
  }
  const source = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /id: "plans"[\s\S]*sections: \["pg_store_plans", "pg_store_plans_info"\]/)
  assert.match(source, /section === "plans" && available\("pg_store_plans"\)/)
  assert.match(source, /section === "plans" && available\("pg_store_plans_info"\)/)
  assert.match(source, /!current\.some[\s\S]*\[\.\.\.current, focus\]/)
  assert.match(source, /loadSeries\(hour, section, filters, fields, controller\.signal, row\.typeId\)/)
  assert.match(source, /\{ section, fields: \[field\], typeId: row\.typeId \}[\s\S]*fullText: true/)
})

test("an exact finding row wins over the previous PostgreSQL selection", () => {
  const first = { ...row("1001001", { pid: 7 }), ordinal: "0" }
  const focus = { ...row("1001001", { pid: 8 }), ordinal: "1" }
  assert.equal(helpers.selectedEntity([first, focus], first, focus, "pg_stat_activity"), focus)
})

test("database totals omit PostgreSQL's shared-object statistics row", () => {
  const shared = { ...row("1003001", { datid: 0 }), logicalName: "pg_stat_database" }
  const database = { ...row("1003001", { datid: 16_384 }), logicalName: "pg_stat_database" }
  assert.equal(helpers.postgresDatabaseCount([shared, database]), 1)
})
