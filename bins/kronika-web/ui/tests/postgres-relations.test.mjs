import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const relation = await importModule('export * from "../src/postgres-relations.ts"', { plugins: [registryPlugin([])] })
const view = await importModule(
  'export { relationColumns, relationDataRows, relationDetailColumns } from "../src/postgres-relations-view.tsx"',
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
    source: source === null ? null : { segment_id: "1709164800000000", ...source },
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
  assert.equal(state.fields.includes("unready_count"), true)
  assert.equal(state.fields.includes("indexdef"), false)
})

test("rendered relation columns hide numeric identity while requests retain it", () => {
  const hidden = ["datid", "relid", "indexrelid"]
  for (const section of relation.RELATION_SECTIONS) {
    const lenses = section === "pg_stat_user_tables" ? relation.TABLE_LENSES : relation.INDEX_LENSES
    for (const lens of lenses) {
      for (const group of relation.RELATION_GROUPS) {
        const request = relation.relationRequest(section, lens, group)
        assert.equal(request.fields.includes("datid"), true, `${section}:${lens}:${group}:datid projection`)
        assert.equal(request.fields.includes("relid"), group === "object", `${section}:${lens}:${group}:relid projection`)
        assert.equal(request.fields.includes("indexrelid"), section === "pg_stat_user_indexes" && group === "object", `${section}:${lens}:${group}:indexrelid projection`)
        assert.deepEqual(Object.keys(request.order).filter((field) => hidden.includes(field)), [], `${section}:${lens}:${group}:visible sorts`)
        for (const columns of [view.relationColumns(section, lens, group), view.relationDetailColumns(section, lens, group)]) {
          const fields = columns.map(({ field }) => field)
          assert.deepEqual(fields.filter((field) => hidden.includes(field)), [], `${section}:${lens}:${group}`)
          assert.equal(fields.includes("datname"), true, `${section}:${lens}:${group}:database name`)
          assert.equal(fields.includes("schemaname"), group !== "database", `${section}:${lens}:${group}:schema name`)
          assert.equal(fields.includes("relname"), group === "object", `${section}:${lens}:${group}:table name`)
          assert.equal(fields.includes("indexrelname"), section === "pg_stat_user_indexes" && group === "object", `${section}:${lens}:${group}:index name`)
        }
      }
    }
  }
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
  assert.deepEqual(row.relation, { group: "database" })
  assert.deepEqual(row.values, { datid: "42", datname: "app", table_count: 12, main_fork_bytes: 4096 })
  assert.equal(row.typeId, "")
  assert.equal(row.ordinal, "")
  assert.equal(row.timestamp, 2_000_000)
})

test("the wire unit matrix is the only source of relation rates", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_indexes", "database", [
    column("index_count", "number", "count", false),
    column("invalid_count", "number", "count", false),
    column("unready_count", "number", "count", false),
    column("unique_count", "number", "count", false),
    column("primary_count", "number", "count", false),
    column("idx_scan", "number", "per_second", true),
    column("tuples_per_scan", "number", "none", true),
    column("main_fork_bytes", "number", "bytes", true),
    column("buffer_hit_pct", "number", "percent", true),
  ]))
  assert.deepEqual(relation.relationRateFields(storedLayout), ["idx_scan"])
  const columns = view.relationColumns("pg_stat_user_indexes", "state", "database", relation.relationRateFields(storedLayout))
  for (const field of ["index_count", "invalid_count", "unready_count", "unique_count", "primary_count"]) {
    assert.equal(columns.find((column) => column.field === field)?.rate, false, field)
  }

  const usage = view.relationColumns("pg_stat_user_indexes", "usage", "database", relation.relationRateFields(storedLayout))
  assert.equal(usage.find((column) => column.field === "idx_scan")?.rate, true)
  assert.equal(usage.find((column) => column.field === "tuples_per_scan")?.rate, false)
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
  assert.equal(relation.relationRowKey(first), JSON.stringify(["pg_stat_user_tables", "schema", "11", "one", "public"]))
  assert.equal(relation.relationRowKey(second), JSON.stringify(["pg_stat_user_tables", "schema", "12", "two", "public"]))
  assert.deepEqual(relation.relationDrill(first), {
    section: "pg_stat_user_tables",
    group: "object",
    filters: { datid: "11", schemaname: "public" },
    selectedKey: null,
  })
})

test("same-named tables and indexes in different databases keep their complete wire identity", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "object", []))
  const first = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "11", datname: "one", schemaname: "public", relid: "42", relname: "orders" }, {},
    { type_id: "1013001", ordinal: "1", timestamp: "2000000" },
  ), storedLayout)
  const second = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "12", datname: "two", schemaname: "public", relid: "42", relname: "orders" }, {},
    { type_id: "1013001", ordinal: "2", timestamp: "2000000" },
  ), storedLayout)
  assert.equal(relation.relationRowKey(first), JSON.stringify(["pg_stat_user_tables", "object", "11", "one", "public", "42", "orders"]))
  assert.notEqual(relation.relationRowKey(first), relation.relationRowKey(second))

  const indexLayout = layout(layoutRecord("pg_stat_user_indexes", "object", []))
  const indexes = [first, second].map((table, ordinal) => parseRow(relationRecord(
    "pg_stat_user_indexes", "object",
    { ...table.values, indexrelid: "43", indexrelname: "orders_pkey" }, {},
    { type_id: "1014001", ordinal: String(ordinal), timestamp: "2000000" },
  ), indexLayout))
  assert.equal(relation.relationRowKey(indexes[0]), JSON.stringify(["pg_stat_user_indexes", "object", "11", "one", "public", "42", "orders", "43", "orders_pkey"]))
  assert.notEqual(relation.relationRowKey(indexes[0]), relation.relationRowKey(indexes[1]))
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
  assert.equal(target.at, 2_000_000)
  assert.equal(target.request.typeId, "1014002")
  assert.deepEqual(target.request.fields, ["indexdef"])
  assert.deepEqual(target.options, { typeId: "1014002", rowOrdinal: "17", fullText: true })

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
  const otherDatabase = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "43", datname: "other", schemaname: "sales", relid: "9001", relname: "orders" }, {},
    { type_id: "1013004", ordinal: "11", timestamp: "2000000" },
  ), tableLayout)
  assert.notEqual(back?.selectedKey, relation.relationRowKey(otherDatabase))
})

test("detail requests only the exact index definition", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_indexes", "object", []))
  const row = parseRow(relationRecord(
    "pg_stat_user_indexes", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    {}, { type_id: "1014002", ordinal: "17", timestamp: "2000000" },
  ), storedLayout)
  assert.equal(row.segmentId, "1709164800000000")
  assert.deepEqual(relation.relationDetailTarget(row).request.fields, ["indexdef"])

  const tableLayout = layout(layoutRecord("pg_stat_user_tables", "object", []))
  const table = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders" },
    {}, { type_id: "1013002", ordinal: "18", timestamp: "2000000" },
  ), tableLayout)
  assert.throws(() => relation.relationDetailTarget(table), /index definition source/)
})

test("table detail lenses render five distinct semantic field matrices", () => {
  const expectedMetrics = {
    access: ["seq_scan", "idx_scan", "sequential_share_pct", "seq_tup_read", "idx_tup_fetch", "seq_tuples_per_scan", "idx_tuples_per_scan", "last_seq_scan", "last_idx_scan"],
    changes: ["n_tup_ins", "n_tup_upd", "n_tup_del", "n_tup_hot_upd", "n_tup_newpage_upd", "dead_pct", "hot_pct", "new_page_pct", "n_live_tup", "n_dead_tup", "n_mod_since_analyze", "n_ins_since_vacuum"],
    maintenance: ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", "last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum", "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    size_buffers: ["main_fork_bytes", "toast_bytes", "reltuples", "toast_n_live_tup", "toast_n_dead_tup", "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit", "buffer_hit_pct"],
    freeze: ["xid_age", "mxid_age", "n_ins_since_vacuum", "last_vacuum", "last_autovacuum"],
  }
  const matrices = relation.TABLE_LENSES.map((lens) => {
    const fields = view.relationDetailColumns("pg_stat_user_tables", lens, "object", ["seq_scan", "heap_blks_read"]).map(({ field }) => field)
    assert.deepEqual(fields.slice(-expectedMetrics[lens].length), expectedMetrics[lens], lens)
    assert.equal(fields.includes("datid"), false, lens)
    assert.equal(fields.includes("relid"), false, lens)
    return fields.join(",")
  })
  assert.equal(new Set(matrices).size, relation.TABLE_LENSES.length)

  const size = view.relationDetailColumns("pg_stat_user_tables", "size_buffers", "object", ["heap_blks_read"])
  assert.equal(size.find(({ field }) => field === "heap_blks_read")?.rate, true)
  assert.equal(size.find(({ field }) => field === "heap_blks_hit")?.rate, false)

  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "object", [
    column("heap_blks_read", "number", "per_second"),
    column("heap_blks_hit", "number", "per_second"),
  ]))
  const row = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders" },
    { heap_blks_read: null, heap_blks_hit: 0 },
    { type_id: "1013002", ordinal: "18", timestamp: "2000000" },
  ), storedLayout)
  assert.equal(row.values.heap_blks_read, null)
  assert.equal(row.values.heap_blks_hit, 0)
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
  for (const field of ["state_severity", "invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"]) {
    assert.equal(state.find((column) => column.field === field)?.sortable, true, field)
  }
  assert.equal(state.some((column) => column.field === "indexdef"), false)
})

test("the relation view consumes only rows with the authoritative discriminator", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "database", [column("table_count")]))
  const relationRow = parseRow(relationRecord(
    "pg_stat_user_tables", "database", { datid: "42", datname: "app" }, { table_count: 2 },
  ), storedLayout)
  const physical = { ...relationRow, relation: undefined }
  assert.deepEqual(view.relationDataRows([relationRow, physical], "pg_stat_user_tables", "database"), [relationRow])
  assert.deepEqual(view.relationDataRows([relationRow], "pg_stat_user_tables", "schema"), [])
})

test("detail, empty, navigation, and paging behavior stays on generic exact APIs", async () => {
  const source = await readFile(new URL("../src/postgres-relations-view.tsx", import.meta.url), "utf8")
  const postgres = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /serverSorted/)
  assert.match(source, /emptyHourStatusKey\(hour\)/)
  assert.match(source, /relationDrill\(row\)/)
  assert.match(source, /linkedRelation\(row\)/)
  assert.match(source, /row\.logicalName === "pg_stat_user_indexes" \? relationDetailTarget\(row\) : null/)
  assert.match(source, /loadSnapshot\(row\.segmentId, definitionTarget\.at, \[definitionTarget\.request\]/)
  assert.match(source, /loadSeries\(hour, row\.logicalName, historyFilters\(row\), \[historyField\], controller\.signal, undefined, row\.timestamp\)/)
  assert.match(source, /relationDetailColumns\(row\.logicalName as RelationSection, lens, row\.relation!\.group, rateFields\)/)
  assert.match(source, /display\(value\(row, column\.field\), column, locale, t\)/)
  assert.doesNotMatch(source, /Object\.keys\(exact\.values\)|rate: false/)
  assert.match(source, /data-testid="pg-exact-indexdef"/)
  assert.match(source, /rawText\(values\?\.datname/)
  assert.match(source, /filters\.schemaname \?\? null/)
  assert.doesNotMatch(source, /pg\.relation\.scope\.(?:database|schema|table|index)/)
  assert.match(source, /tableState\(metadata/)
  assert.doesNotMatch(source, /\$\{name\}=\$\{stored\}|metadata\?\.orderBy/)
  assert.doesNotMatch(source, /DROP|drop recommendation|unused index/i)
  assert.match(postgres, /id: "tables"[\s\S]*sections: \["pg_stat_user_tables"\]/)
  assert.match(postgres, /id: "indexes"[\s\S]*sections: \["pg_stat_user_indexes"\]/)
  assert.match(postgres, /tab\.id === "tables" \|\| tab\.id === "indexes"/)
})
