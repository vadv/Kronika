import assert from "node:assert/strict"
import test from "node:test"

import { DEFAULT_ADDRESS, pgSectionOf, readAddress, sourceOf, viewOf, writeAddress } from "../src/address.ts"

test("an address survives a round trip through the query string", () => {
  const address = {
    ...DEFAULT_ADDRESS,
    at: 1_786_445_580_254_226,
    view: "host.processes" as const,
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
  assert.equal(address.view, "host.processes")
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
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view: "host.system", pgLens: "io" }), "/?view=host.system")
  assert.equal(readAddress("view=pg.plans&pg_lens=regression").pgLens, "load")
})

test("relation drilldown keeps database-scoped identity", () => {
  const selected = JSON.stringify(["pg_stat_user_indexes", "object", "16384", "24577"])
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
  assert.equal(written, `/?view=pg.indexes&pg_lens=low_activity&level=object&datid=16384&schema=public+spaces&relid=24576&indexrelid=24577&row=${encodeURIComponent(selected)}`)
  assert.deepEqual(readAddress(written.slice(1)), address)
  assert.equal(readAddress("view=pg.tables&level=schema&datid=oops").pgLevel, "database")
})

test("relation selection is separate from hierarchy filters", () => {
  const selected = JSON.stringify(["pg_stat_user_tables", "object", "16384", "24576"])
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
