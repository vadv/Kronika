import assert from "node:assert/strict"
import test from "node:test"

import { DEFAULT_ADDRESS, pgSectionOf, readAddress, sourceOf, viewOf, writeAddress } from "../src/address.ts"

test("an address survives a round trip through the query string", () => {
  const address = {
    ...DEFAULT_ADDRESS,
    at: 1_786_445_580_254_226,
    view: "processes" as const,
    lens: "cpu" as const,
    pgLens: "load" as const,
    sort: { column: "utime", descending: true },
    row: "1244346:1784523346370000",
    find: "postgres*",
  }
  const written = writeAddress(address)

  assert.equal(written, "/?at=1786445580254226&sort=-utime&row=1244346%3A1784523346370000&find=postgres*")
  assert.deepEqual(readAddress(written.slice(1)), address)
})

test("a plain screen keeps a plain link", () => {
  assert.equal(writeAddress(DEFAULT_ADDRESS), "/")
  assert.deepEqual(readAddress(""), DEFAULT_ADDRESS)
})

test("relation tabs default to the unfiltered object level", () => {
  for (const view of ["pg.tables", "pg.indexes"] as const) {
    const address = readAddress(`view=${view}`)
    assert.equal(address.pgLevel, "object")
    assert.equal(address.datid, null)
    assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view }), `/?view=${view}`)
  }
})

test("the lens is written only where it means something", () => {
  const written = writeAddress({ ...DEFAULT_ADDRESS, lens: "disk", view: "pg.activity" })
  assert.equal(written, "/?view=pg.activity")
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, lens: "generic" }), "/?lens=generic")
})

test("a process row never leaks into a PostgreSQL address", () => {
  const written = writeAddress({ ...DEFAULT_ADDRESS, row: "1244346:1784523346370000", view: "pg.statements" })
  assert.equal(written, "/?view=pg.statements")
  assert.equal(readAddress("view=pg.statements&row=1244346:1784523346370000").row, null)
})

test("an unreadable value falls back instead of failing", () => {
  const address = readAddress("at=tomorrow&view=host.klingon&lens=quantum&sort=")

  assert.equal(address.at, null)
  assert.equal(address.view, "processes")
  assert.equal(address.lens, "cpu")
  assert.equal(address.sort, null)
})

test("ascending sort has no marker and descending has a minus", () => {
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, sort: { column: "rmem_kb", descending: false } }), "/?sort=rmem_kb")
  assert.deepEqual(readAddress("sort=rmem_kb").sort, { column: "rmem_kb", descending: false })
  assert.deepEqual(readAddress("sort=-rmem_kb").sort, { column: "rmem_kb", descending: true })
})

test("PostgreSQL plans have a stable address", () => {
  const address = { ...DEFAULT_ADDRESS, view: "pg.plans" as const }

  assert.equal(writeAddress(address), "/?view=pg.plans")
  assert.equal(readAddress("view=pg.plans").view, "pg.plans")
  assert.equal(sourceOf(address.view), "postgresql")
  assert.equal(pgSectionOf(address.view), "plans")
  assert.equal(viewOf("postgresql", "processes", "plans"), "pg.plans")
})

test("PostgreSQL lenses survive navigation only on their tables", () => {
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view: "pg.statements", pgLens: "stability" }), "/?view=pg.statements&pg_lens=stability")
  assert.equal(readAddress("view=pg.statements&pg_lens=stability").pgLens, "stability")
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view: "pg.plans", pgLens: "timing" }), "/?view=pg.plans&pg_lens=timing")
  assert.equal(readAddress("view=pg.plans&pg_lens=timing").pgLens, "timing")
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view: "host.overview", pgLens: "io" }), "/?view=host.overview")
  assert.equal(readAddress("view=pg.plans&pg_lens=regression").pgLens, "load")
})

test("relation drilldown keeps database-scoped identity", () => {
  const selected = JSON.stringify(["pg_stat_user_indexes", "object", "16384", "app", "public spaces", "24576", "orders", "24577", "orders_pkey"])
  const address = {
    ...DEFAULT_ADDRESS,
    view: "pg.indexes" as const,
    pgLens: "low_activity" as const,
    pgLevel: "object" as const,
    datid: "16384",
    schema: "public spaces",
    relid: "24576",
    indexrelid: "24577",
    row: selected,
  }
  const written = writeAddress(address)
  assert.equal(written, `/?view=pg.indexes&pg_lens=low_activity&datid=16384&schema=public+spaces&relid=24576&indexrelid=24577&${new URLSearchParams({ row: selected })}`)
  assert.deepEqual(readAddress(written.slice(1)), address)
  assert.equal(readAddress("view=pg.tables&level=schema&datid=oops").pgLevel, "schema")
})

test("relation levels round-trip with or without a drill-down context", () => {
  for (const pgLevel of ["object", "schema", "database"] as const) {
    const address = { ...DEFAULT_ADDRESS, view: "pg.indexes" as const, pgLevel, find: "orders*", pgLens: "state" as const }
    const written = writeAddress(address)
    assert.deepEqual(readAddress(written.slice(1)), address)
  }
})

test("relation selection is separate from hierarchy filters", () => {
  const selected = JSON.stringify(["pg_stat_user_tables", "object", "16384", "app", "public", "24576", "orders"])
  const written = writeAddress({
    ...DEFAULT_ADDRESS,
    view: "pg.tables",
    pgLens: "changes",
    pgLevel: "object",
    datid: "16384",
    schema: "public",
    row: selected,
  })

  assert.match(written, /row=/)
  assert.equal(readAddress(written.slice(1)).row, selected)
  assert.equal(readAddress("view=pg.statements&row=hidden").row, null)
})

test("Host master/detail modes and plan-to-query context are URL-native and route-scoped", () => {
  const target = { dbId: "16384", origin: "plan" as const, planId: "77", queryId: "42", relation: "last" as const, sourceTypeId: "1004001", userId: "10" }
  const host = writeAddress({ ...DEFAULT_ADDRESS, view: "host.overview", metric: "cpu_used_cores" })
  assert.equal(host, "/?view=host.overview&metric=cpu_used_cores")
  assert.equal(readAddress(host.slice(1)).metric, "cpu_used_cores")
  assert.equal(readAddress("?view=host.cpu&metric=cpu_user").metric, "cpu_user")
  const filesystem = writeAddress({ ...DEFAULT_ADDRESS, view: "host.storage", mode: "filesystems", row: "mount:8:1:/", metric: "free_bytes" })
  assert.equal(filesystem, "/?view=host.storage&row=mount%3A8%3A1%3A%2F&metric=free_bytes&mode=filesystems")
  assert.equal(readAddress(filesystem.slice(1)).mode, "filesystems")
  assert.equal(readAddress(filesystem.slice(1)).row, "mount:8:1:/")
  assert.equal(readAddress("?view=host.storage&mode=made-up").mode, "io")
  assert.equal(readAddress("?view=host.cpu&mode=topology").mode, "topology")

  const query = writeAddress({ ...DEFAULT_ADDRESS, at: 1_700_000_000_000_000, view: "pg.statements", statementTarget: target })
  assert.equal(query, "/?at=1700000000000000&view=pg.statements&stmt_origin=plan&stmt_qid=42&stmt_dbid=16384&stmt_userid=10&stmt_relation=last&stmt_source=1004001&stmt_plan=77")
  assert.deepEqual(readAddress(query).statementTarget, target)
  assert.deepEqual(readAddress("?view=pg.statements&stmt_origin=plan&stmt_qid=-42&stmt_dbid=16384&stmt_userid=10&stmt_relation=shared&stmt_source=1003001&stmt_plan=-77").statementTarget, {
    dbId: "16384", origin: "plan", planId: "-77", queryId: "-42", relation: "shared", sourceTypeId: "1003001", userId: "10",
  })
  const activity = { dbId: "4294967295", origin: "activity" as const, queryId: "-9223372036854775808" }
  const activityQuery = writeAddress({ ...DEFAULT_ADDRESS, view: "pg.statements", statementTarget: activity })
  assert.equal(activityQuery, "/?view=pg.statements&stmt_origin=activity&stmt_qid=-9223372036854775808&stmt_dbid=4294967295")
  assert.deepEqual(readAddress(activityQuery.slice(1)).statementTarget, activity)
  const activityRow = writeAddress({ ...DEFAULT_ADDRESS, view: "pg.activity", row: "1700000000000000:1001004:73" })
  assert.equal(activityRow, "/?view=pg.activity&row=1700000000000000%3A1001004%3A73")
  assert.equal(readAddress(activityRow.slice(1)).row, "1700000000000000:1001004:73")
  assert.equal(readAddress("?view=pg.plans&stmt_origin=plan&stmt_qid=42&stmt_dbid=1&stmt_userid=1&stmt_relation=last&stmt_source=1004001&stmt_plan=77").statementTarget, null)
  assert.equal(readAddress("?view=pg.statements&stmt_origin=activity&stmt_qid=0&stmt_dbid=1").statementTarget, null)
  assert.equal(readAddress("?view=pg.statements&stmt_origin=activity&stmt_qid=42&stmt_dbid=4294967296").statementTarget, null)
})
