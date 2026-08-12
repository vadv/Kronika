import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const relation = await importModule('export * from "../src/postgres-relations.ts"', { plugins: [registryPlugin([])] })
const view = await importModule(
  'export { relationColumns, relationDataRows } from "../src/postgres-relations-view.tsx"',
  { plugins: [registryPlugin([])] },
)

const column = (name, kind = "number", unit = "count", nullable = true) => ({ name, kind, unit, nullable })
const layoutRecord = (logical_name, group, columns) => ({ record: "relation_layout", logical_name, group, columns })

function layout(record) {
  const parsed = relation.parseRelationLayout(record)
  assert.notEqual(parsed, null)
  return parsed
}

function relationRecord(logical_name, group, key, values, source = null) {
  return {
    record: "relation",
    logical_name,
    group,
    key,
    values,
    sample_from: "1000000",
    sample_to: "2000000",
    source,
  }
}

function parseRow(record, storedLayout, segmentId = "segment-a") {
  const layouts = new Map([[relation.relationLayoutKey(storedLayout), storedLayout]])
  const parsed = relation.parseRelationRow(record, layouts, segmentId)
  assert.notEqual(parsed, null)
  return parsed
}

test("relation requests keep hierarchy separate from fixed metric lenses", () => {
  assert.deepEqual(relation.TABLE_LENSES, ["access", "changes", "maintenance", "size_buffers", "freeze"])
  assert.deepEqual(relation.INDEX_LENSES, ["usage", "low_activity", "size_buffers", "state"])

  const access = relation.relationRequest("pg_stat_user_tables", "access", "schema")
  assert.equal(access.group, "schema")
  assert.equal(access.pageSize, 200)
  assert.deepEqual(access.defaultOrder, ["seq_scan"])
  assert.deepEqual(access.order.sequential_share_pct, ["derived.sequential_share_pct"])
  assert.equal(access.fields.includes("table_count"), true)
  assert.equal(access.fields.includes("last_seq_scan_oldest"), true)
  assert.equal(access.fields.includes("last_seq_scan_latest"), true)
  assert.equal(access.fields.includes("last_seq_scan_never_count"), true)

  const changes = relation.relationRequest("pg_stat_user_tables", "changes", "object")
  assert.deepEqual(changes.defaultOrder, ["n_tup_upd"])
  for (const field of ["dead_pct", "hot_pct", "new_page_pct"]) {
    assert.deepEqual(changes.order[field], [`derived.${field}`])
  }

  const low = relation.relationRequest("pg_stat_user_indexes", "low_activity", "object")
  assert.deepEqual(low.defaultOrder, ["main_fork_bytes"])
  assert.equal(low.fields.includes("no_scans"), true)
  assert.deepEqual(low.filters, { no_scans: "true" })

  const state = relation.relationRequest("pg_stat_user_indexes", "state", "database")
  assert.deepEqual(state.defaultOrder, ["derived.state_severity"])
  assert.equal(state.fields.includes("invalid_count"), true)
  assert.equal(state.fields.includes("not_ready_count"), true)
  assert.equal(state.fields.includes("indexdef"), false)
})

test("wire rows preserve explicit aggregate identity and never invent a physical locator", () => {
  const databaseLayout = layout(layoutRecord("pg_stat_user_tables", "database", [
    column("table_count", "number", "count", false),
    column("main_fork_bytes", "number", "bytes", false),
  ]))
  const row = parseRow(relationRecord(
    "pg_stat_user_tables",
    "database",
    { datid: "42", datname: "app" },
    { table_count: 12, main_fork_bytes: 4096 },
  ), databaseLayout)
  assert.deepEqual(row.key, { datid: "42", datname: "app" })
  assert.deepEqual(row.values, { datid: "42", datname: "app", table_count: 12, main_fork_bytes: 4096 })
  assert.equal(row.source, null)
  assert.equal(Object.hasOwn(row, "typeId"), false)
  assert.equal(Object.hasOwn(row, "ordinal"), false)
  assert.equal(row.sampleFrom, 1_000_000)
  assert.equal(row.sampleTo, 2_000_000)
})

test("same schema names in different databases remain distinct", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "schema", [column("table_count")]))
  const first = parseRow(relationRecord(
    "pg_stat_user_tables", "schema",
    { datid: "11", datname: "one", schemaname: "public" },
    { table_count: 2 },
  ), storedLayout)
  const second = parseRow(relationRecord(
    "pg_stat_user_tables", "schema",
    { datid: "12", datname: "two", schemaname: "public" },
    { table_count: 3 },
  ), storedLayout)
  assert.notEqual(relation.relationRowKey(first), relation.relationRowKey(second))
  assert.deepEqual(relation.relationDrill(first), {
    section: "pg_stat_user_tables",
    group: "object",
    filters: { datid: "11", schemaname: "public" },
    selectedKey: null,
  })
})

test("object source locators are exact and index definitions stay lazy and object-only", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_indexes", "object", [
    column("idx_scan", "number", "per_second"),
  ]))
  const row = parseRow(relationRecord(
    "pg_stat_user_indexes",
    "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    { idx_scan: 0 },
    { type_id: "1014002", ordinal: "17", timestamp: "2000000" },
  ), storedLayout)
  const target = relation.relationDetailTarget(row)
  assert.equal(target.segmentId, "segment-a")
  assert.equal(target.at, 2_000_000)
  assert.equal(target.request.typeId, "1014002")
  assert.equal(target.request.fields, undefined)
  assert.deepEqual(target.options, { typeId: "1014002", rowOrdinal: "17", fullText: true })
  assert.equal(relation.intervalHasNoScans(row), true)

  assert.throws(() => relation.parseRelationLayout(layoutRecord(
    "pg_stat_user_indexes", "schema", [column("indexdef", "text", "none")],
  )), /aggregate index definition/)
  assert.throws(() => parseRow({ ...relationRecord(
    "pg_stat_user_indexes", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    { idx_scan: 0 },
  ) }, storedLayout), /relation source/)
})

test("table and index navigation uses exact database-scoped table identity", () => {
  const tableLayout = layout(layoutRecord("pg_stat_user_tables", "object", []))
  const table = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "42", datname: "app", schemaname: "sales", relid: "9001", relname: "orders" },
    {},
    { type_id: "1013004", ordinal: "9", timestamp: "2000000" },
  ), tableLayout)
  assert.deepEqual(relation.linkedRelation(table), {
    section: "pg_stat_user_indexes",
    group: "object",
    filters: { datid: "42", relid: "9001" },
    selectedKey: null,
  })

  const indexLayout = layout(layoutRecord("pg_stat_user_indexes", "object", []))
  const index = parseRow(relationRecord(
    "pg_stat_user_indexes", "object",
    { datid: "42", datname: "app", schemaname: "sales", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    {},
    { type_id: "1014002", ordinal: "10", timestamp: "2000000" },
  ), indexLayout)
  const back = relation.linkedRelation(index)
  assert.deepEqual(back?.filters, { datid: "42", relid: "9001" })
  assert.equal(back?.selectedKey, relation.relationRowKey(table))
})

test("detail requests the exact physical row without assuming a layout", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_indexes", "object", []))
  const row = parseRow(relationRecord(
    "pg_stat_user_indexes", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    {}, { type_id: "1014002", ordinal: "17", timestamp: "2000000" },
  ), storedLayout)
  assert.equal(relation.relationDetailTarget(row).request.fields, undefined)
})

test("object history is reset-safe, layout-safe, and keeps missing values unavailable", () => {
  const row = (timestamp, typeId, value) => ({
    segmentId: "segment-a",
    logicalName: "pg_stat_user_indexes",
    typeId,
    ordinal: String(timestamp),
    timestamp,
    values: { idx_scan: value, main_fork_bytes: value },
  })
  const rows = [
    row(1_000_000, "1014001", 10),
    row(3_000_000, "1014001", 14),
    row(5_000_000, "1014001", 3),
    row(7_000_000, "1014002", 7),
    row(9_000_000, "1014002", null),
    row(11_000_000, "1014002", 9),
  ]
  assert.deepEqual(relation.relationHistory(rows, "idx_scan").map(({ value }) => value), [null, 2, null, null, null, null])
  assert.deepEqual(relation.relationHistory(rows.slice(0, 3), "main_fork_bytes").map(({ value }) => value), [10, 14, 3])
})

test("wire validation rejects mismatched keys, values, intervals, and fake aggregate sources", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "database", [column("table_count", "number", "count", false)]))
  const valid = relationRecord("pg_stat_user_tables", "database", { datid: "42", datname: "app" }, { table_count: 2 })
  assert.throws(() => parseRow({ ...valid, key: { datid: "42", datname: "app", schemaname: "public" } }, storedLayout), /relation key/)
  assert.throws(() => parseRow({ ...valid, values: {} }, storedLayout), /relation values/)
  assert.throws(() => parseRow({ ...valid, values: { table_count: null } }, storedLayout), /relation null/)
  assert.throws(() => parseRow({ ...valid, sample_from: "3", sample_to: "2" }, storedLayout), /sample interval/)
  assert.throws(() => parseRow({ ...valid, source: { type_id: "1013001", ordinal: "1", timestamp: "2" } }, storedLayout), /relation source/)
})

test("the relation table exposes complete server-sortable quantitative lenses", () => {
  const changes = view.relationColumns("pg_stat_user_tables", "changes", "object")
  for (const field of ["n_tup_ins", "n_tup_upd", "n_tup_del", "dead_pct", "hot_pct", "new_page_pct", "n_live_tup"]) {
    assert.equal(changes.find((column) => column.field === field)?.sortable, true, field)
  }
  assert.equal(changes.find((column) => column.field === "relname")?.sticky, true)
  assert.notEqual(changes.find((column) => column.field === "relname")?.sortable, true)

  const state = view.relationColumns("pg_stat_user_indexes", "state", "schema")
  for (const field of ["state_severity", "invalid_count", "not_ready_count", "unique_count", "primary_count", "exclusion_count"]) {
    assert.equal(state.find((column) => column.field === field)?.sortable, true, field)
  }
  assert.equal(state.some((column) => column.field === "indexdef"), false)
})

test("the relation view consumes only rows with the authoritative discriminator", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "database", [column("table_count")]))
  const relationRow = parseRow(relationRecord(
    "pg_stat_user_tables", "database", { datid: "42", datname: "app" }, { table_count: 2 },
  ), storedLayout)
  const adapted = { segmentId: "segment-a", logicalName: relationRow.logicalName, typeId: "", ordinal: "", timestamp: 2_000_000, values: relationRow.values, relation: relationRow }
  const physical = { ...adapted, relation: undefined }
  assert.deepEqual(view.relationDataRows([adapted, physical], "pg_stat_user_tables", "database"), [adapted])
  assert.deepEqual(view.relationDataRows([adapted], "pg_stat_user_tables", "schema"), [])
})

test("detail, empty, navigation, and paging behavior stays on generic exact APIs", async () => {
  const source = await readFile(new URL("../src/postgres-relations-view.tsx", import.meta.url), "utf8")
  const postgres = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /serverSorted/)
  assert.match(source, /emptyHourStatusKey\(hour\)/)
  assert.match(source, /relationDrill\(relation\)/)
  assert.match(source, /linkedRelation\(row\)/)
  assert.match(source, /relationDetailTarget\(row\)/)
  assert.match(source, /loadSnapshot\(target\.segmentId, target\.at, \[target\.request\]/)
  assert.match(source, /loadSeries\(hour, row\.logicalName, historyFilters\(row\), \[historyField\], controller\.signal, undefined, target\.at\)/)
  assert.match(source, /data-testid="pg-exact-indexdef"/)
  assert.match(source, /pg\.relation\.scope\.database/)
  assert.match(source, /pg\.relation\.search\.active/)
  assert.match(source, /pg\.relation\.loading/)
  assert.doesNotMatch(source, /\$\{name\}=\$\{stored\}|metadata\?\.orderBy/)
  assert.doesNotMatch(source, /DROP|drop recommendation|unused index/i)
  assert.match(postgres, /id: "tables"[\s\S]*sections: \["pg_stat_user_tables"\]/)
  assert.match(postgres, /id: "indexes"[\s\S]*sections: \["pg_stat_user_indexes"\]/)
  assert.match(postgres, /tab\.id === "tables" \|\| tab\.id === "indexes"/)
})
