import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const {
  POSTGRESQL_ACTIVITY_PAGE_SIZE,
  parsePostgresqlActivityPage,
  postgresqlActivityFilter,
  postgresqlActivityHourData,
  postgresqlActivityRequest,
} = await importModule('export * from "../src/postgres-activity-query.ts"; export * from "../src/postgres-activity-result.ts"')

const AT = 1_787_615_999_900_000
const OBSERVED = 1_787_615_999_500_000

test("Activity requests publish only semantic arguments with exact defaults", () => {
  const request = postgresqlActivityRequest(AT, "")
  const url = new URL(request.path, "http://kronika.invalid")
  assert.equal(url.pathname, "/api/postgresql/activity")
  assert.deepEqual([...url.searchParams], [
    ["at", String(AT)],
    ["sort", "query_duration_ms"],
    ["direction", "desc"],
    ["page_size", "200"],
  ])
  assert.equal(request.pageSize, POSTGRESQL_ACTIVITY_PAGE_SIZE)
  for (const physical of ["section", "field", "type_id", "text"]) assert.equal(url.searchParams.has(physical), false)
})

test("Activity request encodes the flat v6 filter, sort, direction, and continuation", () => {
  const search = 'database:"orders east" AND (text:"select orders*" OR text:"update orders*")'
  const request = postgresqlActivityRequest(AT, search, { column: "wait_event_type", descending: false }, "pc1_a/b+c==")
  assert.deepEqual(request.filter, [
    { database: { all_of: ["orders east"] }, text: { all_of: ["select orders*"] } },
    { database: { all_of: ["orders east"] }, text: { all_of: ["update orders*"] } },
  ])
  assert.match(request.path, /filter=%5B%7B/)
  assert.match(request.path, /orders%20east/)
  assert.doesNotMatch(request.path, /orders\+east/)
  const url = new URL(request.path, "http://kronika.invalid")
  assert.equal(url.searchParams.get("filter"), JSON.stringify(request.filter))
  assert.equal(url.searchParams.get("sort"), "wait_type")
  assert.equal(url.searchParams.get("direction"), "asc")
  assert.equal(url.searchParams.get("cursor"), "pc1_a/b+c==")
})

test("Activity maps every table order to the frozen semantic sort token", () => {
  const expected = {
    pid: "pid",
    datname: "database",
    usename: "role",
    query: "query_preview",
    query_preview: "query_preview",
    query_duration_ms: "query_duration_ms",
    transaction_duration_ms: "transaction_duration_ms",
    application_name: "application",
    client_addr: "client",
    state: "state",
    wait_event_type: "wait_type",
    wait_event: "wait_event",
    backend_type: "backend_type",
  }
  for (const [column, sort] of Object.entries(expected)) {
    assert.equal(postgresqlActivityRequest(AT, "", { column, descending: true }).sort, sort)
  }
})

test("Activity ignores an unsupported bookmarked sort and restores exact defaults", () => {
  const request = postgresqlActivityRequest(AT, "", { column: "calls_per_second", descending: false })
  const url = new URL(request.path, "http://kronika.invalid")
  assert.equal(request.sort, "query_duration_ms")
  assert.equal(url.searchParams.get("sort"), "query_duration_ms")
  assert.equal(url.searchParams.get("direction"), "desc")
})

test("Activity positive search becomes bounded flat clauses without changing boolean meaning", () => {
  assert.deepEqual(postgresqlActivityFilter("state:active AND state:idle"), [
    { state: { all_of: ["active", "idle"] } },
  ])
  assert.deepEqual(postgresqlActivityFilter("pid:0"), [])
  assert.deepEqual(postgresqlActivityFilter("pid:42 OR query_id:-7"), [
    { pid: { any_of: [42] } },
    { query_id: { any_of: ["-7"] } },
  ])
  assert.deepEqual(postgresqlActivityFilter("database:app AND text:update AND text:orders-api"), [
    { database: { all_of: ["app"] }, text: { all_of: ["update", "orders-api"] } },
  ])
  assert.deepEqual(postgresqlActivityFilter("orders api"), [{ text: { all_of: ["orders api"] } }])
})

test("Activity typed rows retain PG10 nullability and server durations", () => {
  const page = parsePostgresqlActivityPage(result({
    leader_pid: null,
    datid: null,
    datname: null,
    usename: null,
    state: null,
    wait_event_type: null,
    wait_event: null,
    query_preview: null,
    query_id: null,
    backend_xid_age: null,
    backend_xmin_age: null,
    xact_start: null,
    query_start: null,
    state_change: null,
    backend_age_ms: 123.25,
    query_duration_ms: null,
    transaction_duration_ms: null,
    state_duration_ms: null,
  }), AT)
  assert.equal(page.observedAt, OBSERVED)
  assert.equal(page.rows.length, 1)
  const row = page.rows[0]!
  assert.equal(row.timestamp, OBSERVED)
  assert.equal(row.typeId, "postgresql_activity")
  assert.equal(row.values.query_preview, null)
  assert.equal(row.values.datid, null)
  assert.equal(row.values.query_id, null)
  assert.equal(row.values.backend_age_ms, 123.25)
  assert.equal(row.values.query_duration_ms, null)
  assert.equal(Object.hasOwn(row.values, "query"), false)
})

test("Activity page metadata carries opaque continuation and semantic order", () => {
  const parsed = parsePostgresqlActivityPage(result({}, "opaque-next"), AT)
  const data = postgresqlActivityHourData(parsed, "transaction_duration_ms", "asc", 200)
  assert.equal(data.activities, data.sections.pg_stat_activity)
  assert.equal(data.snapshotRows[0]?.nextCursor, "opaque-next")
  assert.equal(data.snapshotRows[0]?.hasMore, true)
  assert.deepEqual(data.snapshotRows[0]?.orderBy, ["transaction_duration_ms"])
  assert.equal(data.snapshotRows[0]?.orderDirection, "asc")
  const next = postgresqlActivityRequest(AT, "state:active", { column: "transaction_duration_ms", descending: false }, "opaque-next")
  assert.equal(new URL(next.path, "http://kronika.invalid").searchParams.get("cursor"), "opaque-next")
})

test("Activity result parser rejects shape drift and unsafe UI timestamps", () => {
  const extra = result({}) as Record<string, unknown>
  extra.extra = true
  assert.throws(() => parsePostgresqlActivityPage(extra, AT), /Activity result is invalid/)
  assert.throws(() => parsePostgresqlActivityPage({ ...result({}), observed_at: "9007199254740992" }, AT), /outside the UI timestamp range/)
  assert.throws(() => parsePostgresqlActivityPage(result({ query_duration_ms: Number.NaN }), AT), /query_duration_ms is invalid/)
})

test("Activity product request source contains no physical transport grammar", async () => {
  const sources = await Promise.all([
    readFile(new URL("../src/postgres-activity-product.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-activity-query.ts", import.meta.url), "utf8"),
  ])
  const source = sources.join("\n")
  assert.doesNotMatch(source, /[?&](?:section|field|type_id|text)=/)
  assert.doesNotMatch(source, /application\/x-ndjson/)
  assert.match(source, /\/api\/postgresql\/activity/)
})

function result(overrides: Record<string, unknown>, nextCursor: string | null = null): unknown {
  return {
    requested_at: String(AT),
    observed_at: String(OBSERVED),
    rows: [{
      observed_at: String(OBSERVED),
      pid: 8124,
      leader_pid: null,
      datid: 16384,
      datname: "orders",
      usename: "app",
      application_name: "orders-api",
      client_addr: "10.24.3.17",
      backend_type: "client backend",
      state: "active",
      wait_event_type: "Lock",
      wait_event: "transactionid",
      query_preview: "UPDATE orders SET status = $1",
      query_id: "-6172849912508319940",
      backend_xid_age: "42",
      backend_xmin_age: "57",
      backend_start: "1787612400000000",
      xact_start: "1787615989000000",
      query_start: "1787615992000000",
      state_change: "1787615992000000",
      backend_age_ms: 3_599_500,
      query_duration_ms: 7_500,
      transaction_duration_ms: 10_500,
      state_duration_ms: 7_500,
      ...overrides,
    }],
    next_cursor: nextCursor,
  }
}
