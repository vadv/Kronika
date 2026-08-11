import assert from "node:assert/strict"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  format: "esm",
  platform: "node",
  plugins: [{
    name: "registry",
    setup(context) {
      context.onResolve({ filter: /^kronika:registry$/ }, () => ({ namespace: "registry", path: "registry" }))
      context.onLoad({ filter: /.*/, namespace: "registry" }, () => ({ contents: "export const registry = []" }))
    },
  }],
  stdin: {
    contents: 'export { ACTIVITY_COLUMNS, columnsFor, isTimestampField, overviewValue, postgresDatabaseCount, sameEntity, selectedEntity, STATEMENT_COLUMNS } from "../src/postgres-view.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

function row(typeId, values) {
  return { logicalName: "pg_stat_statements", ordinal: "0", segmentId: "a", timestamp: 1, typeId, values }
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

test("statement histories keep top-level state and physical layouts distinct", () => {
  const base = row("1002003", { queryid: 7, userid: 8, dbid: 9, toplevel: true })
  assert.equal(helpers.sameEntity(base, row("1002003", { queryid: 7, userid: 8, dbid: 9, toplevel: false }), "pg_stat_statements"), false)
  assert.equal(helpers.sameEntity(base, row("1002004", { queryid: 7, userid: 8, dbid: 9, toplevel: true }), "pg_stat_statements"), false)
  assert.equal(helpers.STATEMENT_COLUMNS.some((column) => column.field === "total_time"), true)
  assert.equal(helpers.STATEMENT_COLUMNS.some((column) => column.field === "total_exec_time"), true)
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
