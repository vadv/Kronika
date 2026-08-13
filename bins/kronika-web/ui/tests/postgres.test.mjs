import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { renderToStaticMarkup } from "react-dom/server"

import { importModule, registryPlugin } from "./import-module.mjs"

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
const helpers = await importModule(
  'export { ACTIVITY_COLUMNS, ACTIVITY_DEFAULT_ORDER, ACTIVITY_DETAIL_COLUMNS, activityColumns, activityDurationMs, columnsFor, isIdleActivity, isSystemActivity, isTimestampField, overviewBackendCounts, overviewValue, PLAN_COLUMNS, planColumns, postgresDatabaseCount, registryCardFields, sameEntity, selectedEntity, STATEMENT_COLUMNS, statementColumns, tableState, transactionDurationMs, visibleActivityRows } from "../src/postgres-view.tsx"; export { decoratePostgresIntervalRow, findingSemanticField, physicalField, planDefaultOrder, planRequest, postgresIdentity, postgresProjection, statementDefaultOrder, statementRequest } from "../src/postgres-metrics.ts"; export { humanDuration } from "../src/model.ts"',
  { plugins: [registryPlugin(TEST_REGISTRY)] },
)

function layout(typeId, logicalName, identity, fields) {
  return { typeId, logicalName, identity, columns: [...new Set(fields)] }
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

function activityRow(ordinal, values, timestamp = 10_000_000) {
  return { logicalName: "pg_stat_activity", ordinal, segmentId: "a", timestamp, typeId: "1001003", values }
}

test("PostgreSQL durations are not formatted as Unix timestamps", () => {
  assert.equal(helpers.isTimestampField("write_time"), false)
  assert.equal(helpers.isTimestampField("stats_reset"), true)
  assert.equal(helpers.isTimestampField("last_archived_time"), true)
  assert.equal(helpers.overviewValue(123.4, "write_time", "en"), "123 ms")
  assert.equal(helpers.overviewValue(123.4, "max_age_us", "en"), "123 μs")
  assert.equal(helpers.overviewValue(123.4, "wal_bytes", "en"), "123 B")
  assert.equal(helpers.overviewValue(true, "datallowconn", "ru"), "да")
  assert.equal(helpers.columnsFor([row("1", { write_time: 123.4 })])[0].kind, "milliseconds")
  assert.equal(helpers.columnsFor([row("1", { max_age_us: 123.4 })])[0].kind, "microseconds")
})

test("generic registry cards never present raw collection or identity fields as metrics", () => {
  const stored = row("1002003", {
    ts: "1720000000000000",
    queryid: "42",
    userid: "10",
    dbid: "20",
    toplevel: true,
    stats_since: "1719990000000000",
    calls: 7,
  })
  assert.deepEqual(helpers.registryCardFields(stored).map(([field]) => field), ["calls"])
  assert.equal(helpers.columnsFor([stored]).some(({ field }) => field === "ts"), false)
})

test("activity keeps a compact operator table and moves diagnostics to detail", () => {
  assert.deepEqual(
    helpers.ACTIVITY_COLUMNS[0],
    { field: "pid", help: "pg.field.pid.help", kind: "id", label: "pg.field.pid.label", sticky: true, width: 78 },
  )
  assert.deepEqual(helpers.ACTIVITY_COLUMNS[1], {
    field: "query_duration_ms",
    help: "pg.field.query_duration_ms.help",
    kind: "duration",
    label: "pg.field.query_duration_ms.label",
    sticky: false,
    width: 145,
  })
  assert.deepEqual(helpers.ACTIVITY_COLUMNS[2], {
    field: "transaction_duration_ms",
    help: "pg.field.transaction_duration_ms.help",
    kind: "duration",
    label: "pg.field.transaction_duration_ms.label",
    sticky: false,
    width: 155,
  })
  assert.deepEqual(
    helpers.ACTIVITY_COLUMNS.map(({ field }) => field),
    ["pid", "query_duration_ms", "transaction_duration_ms", "state", "wait_event_type", "wait_event", "datname", "usename", "application_name", "client_addr", "query"],
  )
  for (const field of ["backend_type", "leader_pid", "query_id", "backend_xid_age", "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change"]) {
    assert.equal(helpers.ACTIVITY_COLUMNS.some((column) => column.field === field), false)
    assert.equal(helpers.ACTIVITY_DETAIL_COLUMNS.some((column) => column.field === field), true)
  }
  assert.deepEqual(helpers.activityColumns(false), helpers.ACTIVITY_COLUMNS)
  assert.deepEqual(helpers.activityColumns(true).slice(0, 3).map(({ field }) => field), ["pid", "backend_type", "query_duration_ms"])
  assert.deepEqual(helpers.ACTIVITY_DEFAULT_ORDER, { column: "query_duration_ms", descending: true })
})

test("activity hides only ordinary idle and derives query and transaction time from the row", () => {
  const active = activityRow("1", { backend_type: "client backend", state: "active", query_start: "4000000", xact_start: "1000000" })
  const idle = activityRow("2", { backend_type: "client backend", state: "idle", query_start: "1000000", xact_start: null })
  const idleTransaction = activityRow("3", { backend_type: "client backend", state: "idle in transaction", query_start: "2000000", xact_start: "2000000" })
  const aborted = activityRow("6", { backend_type: "client backend", state: "idle in transaction (aborted)", query_start: "3000000", xact_start: "3000000" })
  const system = activityRow("4", { backend_type: "checkpointer", state: null, query_start: null })
  const walsender = activityRow("7", { backend_type: "walsender", state: null, query_start: null })
  const legacy = activityRow("5", { backend_type: null, state: "active", query_start: "9000000", xact_start: "8000000" })

  assert.equal(helpers.isSystemActivity(active), false)
  assert.equal(helpers.isSystemActivity(system), true)
  assert.equal(helpers.isSystemActivity(walsender), true)
  assert.equal(helpers.isSystemActivity(legacy), false)
  assert.equal(helpers.isIdleActivity(idle), true)
  assert.equal(helpers.isIdleActivity(idleTransaction), false)
  assert.equal(helpers.isIdleActivity(aborted), false)
  assert.equal(helpers.activityDurationMs(active), 6_000)
  assert.equal(helpers.activityDurationMs(idle), null)
  assert.equal(helpers.activityDurationMs(idleTransaction), null)
  assert.equal(helpers.transactionDurationMs(active), 9_000)
  assert.equal(helpers.transactionDurationMs(idleTransaction), 8_000)
  assert.equal(helpers.transactionDurationMs(aborted), 7_000)
  assert.equal(helpers.transactionDurationMs(idle), null)
  assert.equal(helpers.activityDurationMs(activityRow("6", { state: "active", query_start: "11000000" })), null)
  assert.equal(helpers.transactionDurationMs(activityRow("7", { xact_start: "11000000" })), null)
  assert.equal(helpers.transactionDurationMs(activityRow("8", { xact_start: "0" })), null)

  const rows = [active, idle, idleTransaction, aborted, system, walsender, legacy]
  const defaults = helpers.visibleActivityRows(rows, { showIdle: false, showSystem: false })
  assert.deepEqual(defaults.map(({ ordinal }) => ordinal), ["1", "5", "3", "6"])
  assert.equal(defaults[0].values.query_duration_ms, 6_000)
  assert.equal(defaults[0].values.transaction_duration_ms, 9_000)
  assert.equal(defaults[1].values.query_duration_ms, 1_000)
  assert.equal(defaults[2].values.query_duration_ms, null)
  assert.equal(defaults[2].values.transaction_duration_ms, 8_000)
  assert.deepEqual(
    helpers.visibleActivityRows(rows, { showIdle: true, showSystem: false }).map(({ ordinal }) => ordinal),
    ["1", "5", "3", "6", "2"],
  )
  assert.deepEqual(
    helpers.visibleActivityRows(rows, { showIdle: false, showSystem: true }).map(({ ordinal }) => ordinal),
    ["1", "5", "3", "6", "4", "7"],
  )
  assert.deepEqual(
    helpers.visibleActivityRows(rows, { showIdle: false, showSystem: false }, system).map(({ ordinal }) => ordinal),
    ["1", "5", "3", "6", "4"],
  )
  assert.deepEqual(helpers.overviewBackendCounts(rows), { active: 2, idle: 3, total: 5 })
})

test("elapsed Activity values use compact wall-time formatting", () => {
  assert.equal(helpers.humanDuration(850, "en"), "850 ms")
  assert.equal(helpers.humanDuration(5_200, "en"), "5.2 s")
  assert.equal(helpers.humanDuration(59_999, "en"), "59.9 s")
  assert.equal(helpers.humanDuration(60_000, "en"), "1m 00s")
  assert.equal(helpers.humanDuration(194_000, "en"), "3m 14s")
  assert.equal(helpers.humanDuration(7_560_000, "en"), "2h 06m")
  assert.equal(helpers.humanDuration(5_200, "ru"), "5,2 с")
  assert.equal(helpers.humanDuration(null, "ru"), "—")
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
  assert.match(source, /tab\.id === "plans"/)
  assert.match(source, /pg-plans-empty/)
  assert.match(source, /columns\.some\(\(\{ field \}\) => field === order\.column\)/)
  assert.match(source, /detailColumns=\{ACTIVITY_DETAIL_COLUMNS\}/)
  assert.match(source, /section === "plans" && available\("pg_store_plans_info"\)/)
  assert.match(source, /allRows\.map\(decoratePostgresIntervalRow\)/)
  assert.match(source, /tableState\([^)]*ranked\.length/)
  assert.match(source, /serverSorted=\{dense\}/)
  assert.match(source, /onNearEnd=\{densePageState === "idle" && canLoadMore \? onLoadMore : undefined\}/)
  assert.match(source, /densePageState === "error" \? onRetry : onLoadMore/)
  assert.match(source, /loadSeries\(hour, section, filters, fields, controller\.signal, row\.typeId, row\.timestamp\)/)
  assert.match(source, /\{ section, fields: \[field\], typeId: row\.typeId \}[\s\S]*fullText: true/)
})

test("statement and plan lenses have compact ordered columns", () => {
  assert.deepEqual(helpers.statementColumns("load").slice(0, 5).map(({ field }) => field), ["query", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"])
  assert.equal(helpers.statementColumns("load").some(({ field }) => field === "exec_load"), false)
  assert.equal(helpers.planColumns("load").some(({ field }) => field === "exec_load"), false)
  assert.deepEqual(helpers.statementColumns("per_call").slice(0, 4).map(({ field }) => field), ["query", "mean_exec_ms_per_call", "rows_per_call", "blocks_per_call"])
  assert.ok(helpers.statementColumns("io").some(({ field }) => field === "hit_pct"))
  assert.ok(helpers.statementColumns("resources").some(({ field }) => field === "plan_time_pct"))
  assert.ok(helpers.statementColumns("stability").some(({ field }) => field === "cv"))
  assert.ok(helpers.planColumns("timing").some(({ field }) => field === "stddev_exec_time_ms"))
  assert.ok(helpers.planColumns("identity").some(({ field }) => field === "queryid_stat_statements"))
  assert.ok(helpers.planColumns("identity").some(({ field }) => field === "calls_per_second"))
  assert.equal(helpers.statementDefaultOrder("load"), "execution_ms_per_second")
  assert.equal(helpers.statementDefaultOrder("per_call"), "calls_per_second")
  assert.equal(helpers.statementDefaultOrder("io"), "shared_blks_read")
  assert.equal(helpers.statementDefaultOrder("resources"), "wal_bytes")
  assert.equal(helpers.statementDefaultOrder("stability"), "calls_per_second")
  assert.equal(helpers.planDefaultOrder("timing"), "calls_per_second")
})

test("dense numeric columns advertise server sorting and text identities do not", () => {
  const lenses = [
    ...["load", "per_call", "io", "resources", "stability"].map((lens) => [helpers.statementColumns(lens), helpers.statementRequest(lens)]),
    ...["load", "timing", "io", "identity"].map((lens) => [helpers.planColumns(lens), helpers.planRequest(lens)]),
  ]
  for (const [columns, request] of lenses) {
    for (const column of columns) {
      const quantitative = ["number", "bytes", "milliseconds", "percent", "timestamp"].includes(column.kind)
      assert.equal(column.sortable === true, quantitative, column.field)
      if (quantitative) assert.ok(request.order[column.field]?.length > 0, column.field)
    }
  }
})

test("dense rows retain the physical server order across layouts", () => {
  const oldest = { ...row("1002001", { total_time: 5, query: "old" }), ordinal: "1", timestamp: 100 }
  const middle = { ...row("1002001", { total_time: 7, query: "middle" }), ordinal: "2", timestamp: 100 }
  const newest = { ...row("1002002", { total_exec_time: 9, query: "new" }), ordinal: "3", timestamp: 200 }
  const decorated = [newest, oldest, middle].map(helpers.decoratePostgresIntervalRow)
  assert.deepEqual(decorated.map(({ ordinal }) => ordinal), ["3", "1", "2"])
  assert.deepEqual(decorated.map(({ timestamp }) => timestamp), [200, 100, 100])
})

test("dense table state names cursor, interval, filter, physical server order, and total count", () => {
  const dictionary = {
    "pg.table.cursor": "Cursor {time}", "pg.table.interval": "Interval {from} to {to}",
    "pg.table.filter": "Filter {pattern}", "pg.table.no_filter": "No filter",
    "pg.table.order": "Order {semantic}; {physical}; {direction}", "pg.table.order_default": "Default order",
    "pg.table.desc": "descending", "pg.table.shown": "Loaded {returned} of {eligible}",
    "pg.table.interval_unavailable": "No interval",
    "pg.field.execution_ms_per_second.label": "Execution ms/s",
  }
  const t = (key, slots = {}) => Object.entries(slots).reduce((text, [name, value]) => text.replace(`{${name}}`, value), dictionary[key] ?? key)
  const markup = renderToStaticMarkup(helpers.tableState({
    logicalName: "pg_stat_statements", eligible: 4873, returned: 200,
    hasMore: true, truncated: true, nextCursor: "opaque", pageSize: 200,
    orderBy: ["total_exec_time"], orderDirection: "desc",
    from: 1_800_000_000_000_000, to: 1_800_000_010_000_000,
  }, 200, 1_800_000_010_000_000, "vacuum*", { column: "execution_ms_per_second", descending: true }, "en", t))
  assert.match(markup, /Cursor .* UTC/)
  assert.match(markup, /Interval .* UTC to .* UTC/)
  assert.match(markup, /Filter vacuum\*/)
  assert.match(markup, /Execution ms\/s; total_exec_time; descending/)
  assert.match(markup, /Loaded 200 of 4,873/)
})

test("later pages report the accumulated loaded count without inflating the eligible total", () => {
  const dictionary = {
    "pg.table.cursor": "Cursor {time}", "pg.table.interval": "Interval {from} to {to}",
    "pg.table.interval_unavailable": "No interval",
    "pg.table.filter": "Filter {pattern}", "pg.table.no_filter": "No filter",
    "pg.table.order": "Order {semantic}; {physical}; {direction}", "pg.table.order_default": "Default order",
    "pg.table.desc": "descending", "pg.table.shown": "Loaded {returned} of {eligible}",
    "pg.field.execution_ms_per_second.label": "Execution ms/s",
  }
  const t = (key, slots = {}) => Object.entries(slots).reduce((text, [name, value]) => text.replace(`{${name}}`, value), dictionary[key] ?? key)
  const metadata = {
    logicalName: "pg_stat_statements", eligible: 4873, returned: 200,
    hasMore: true, truncated: true, nextCursor: "opaque", pageSize: 200,
    orderBy: ["total_time", "total_exec_time"], orderDirection: "desc", from: 1_000_000, to: 4_000_000,
  }
  const markup = renderToStaticMarkup(helpers.tableState(metadata, 400, 210, "", { column: "execution_ms_per_second", descending: true }, "en", t))
  assert.match(markup, /Loaded 400 of 4,873/)
  assert.match(markup, /total_time, total_exec_time/)
  assert.doesNotMatch(markup, /Loaded 200/)
})

test("dense paging resets, ignores stale work, and preserves retry state", async () => {
  const source = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(source, /const generation = \+\+snapshotGeneration\.current/)
  assert.match(source, /const stale = \(\) => controller\.signal\.aborted \|\| generation !== snapshotGeneration\.current/)
  assert.match(source, /let inFlight = false/)
  assert.match(source, /if \(inFlight \|\| stale\(\)\)/)
  assert.match(source, /pageCursor === undefined[\s\S]*mergeSnapshotData\(companion[\s\S]*mergeSnapshotData\(current/)
  assert.match(source, /action\.failed = pageCursor[\s\S]*setDensePageState\("error"\)/)
  assert.match(source, /return \(\) => \{ clearTimeout\(timer\); controller\.abort\(\) \}/)
  assert.match(source, /\}, \[context, cursor, cursorSegmentId, densePattern, finishRefresh, hour, order, relationFilters, viewRequests\]\)/)
  assert.doesNotMatch(source, /snapshotRefreshVersion/)
  assert.match(source, /const refreshReady = !loading && cursorState === "ready" && densePageState !== "loading"/)
  assert.match(source, /disabled=\{refreshing \|\| !refreshReady\}/)
  assert.match(source, /typeIds: \[context\.typeId\]/)
  assert.match(source, /Object\.fromEntries\(pageContext\.identity\)/)
  assert.match(source, /denseMetadata\?\.hasMore === true \? denseMetadata\.nextCursor/)
  assert.match(source, /action\.load\(action\.failed\)/)
})

test("an exact finding never opens PostgreSQL detail without an explicit row selection", () => {
  const first = { ...row("1001001", { pid: 7 }), ordinal: "0" }
  const focus = { ...row("1001001", { pid: 8 }), ordinal: "1" }
  assert.equal(helpers.selectedEntity([first, focus], null, "pg_stat_activity"), null)
  assert.equal(helpers.selectedEntity([first, focus], first, "pg_stat_activity"), first)
})

test("an off-page finding is contextualized without appending to the ranked page", async () => {
  const source = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /contextualRows\(ranked, activeContext, exactFocus\)/)
  assert.doesNotMatch(source, /\[\.\.\.ranked,[^\]]*focus/)
  assert.match(source, /contextLabel=\{activeContext\?\.label\}/)
})

test("PostgreSQL detail never opens a row that was not selected", () => {
  const first = { ...row("1002006", { queryid: 7, userid: 8, dbid: 9, toplevel: true }), ordinal: "0" }
  assert.equal(helpers.selectedEntity([first], null, "pg_stat_statements"), null)
  assert.equal(helpers.selectedEntity([], first, "pg_stat_statements"), null)
})

test("an exact locator preview never inherits settled zero page counts", () => {
  const dictionary = {
    "pg.table.cursor": "Cursor {time}", "pg.table.focus_loading": "Exact result; page loading",
    "pg.table.focus_outside": "Exact result outside page", "pg.table.focus_exact": "Exact focused result",
    "pg.table.interval": "Interval {from} to {to}", "pg.table.interval_unavailable": "No interval",
    "pg.table.filter": "Filter {pattern}", "pg.table.no_filter": "No filter", "pg.table.order_default": "Default",
    "pg.table.shown": "Loaded {returned} of {eligible}",
  }
  const t = (key, slots = {}) => Object.entries(slots).reduce((text, [name, value]) => text.replace(`{${name}}`, value), dictionary[key] ?? key)
  const loading = renderToStaticMarkup(helpers.tableState(undefined, 0, 1_800_000_000_000_000, "", undefined, "en", t, "loading"))
  assert.match(loading, /Exact result; page loading/)
  assert.match(loading, /Exact focused result/)
  assert.doesNotMatch(loading, /Loaded 0 of 0/)

  const settled = renderToStaticMarkup(helpers.tableState({
    logicalName: "pg_stat_stat_statements", eligible: 1, returned: 1, hasMore: false, truncated: false,
    nextCursor: null, pageSize: 200, orderBy: ["total_exec_time"], orderDirection: "desc", from: 1, to: 2,
  }, 1, 1_800_000_000_000_000, "", undefined, "en", t))
  assert.match(settled, /Loaded 1 of 1/)
})

test("database totals omit PostgreSQL's shared-object statistics row", () => {
  const shared = { ...row("1003001", { datid: 0 }), logicalName: "pg_stat_database" }
  const database = { ...row("1003001", { datid: 16_384 }), logicalName: "pg_stat_database" }
  assert.equal(helpers.postgresDatabaseCount([shared, database]), 1)
})
