import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { canonicalSearch, parseSearch, rowMatchesSearch, SEARCH_MAX_CLAUSES, SEARCH_MAX_EXPRESSION, SEARCH_MAX_VALUE, searchFields, withoutSearchClause } = await importModule('export * from "../src/search.ts"')

const row = (logicalName, typeId, values) => ({
  logicalName, ordinal: "1", segmentId: "7", timestamp: 10, typeId, values,
})

test("structured search canonicalizes aliases, AND casing, quotes, and escapes", () => {
  const parsed = parseSearch('query_id:-912345 and db:"Sales \\"East\\""', "pg_stat_statements")
  assert.equal(parsed.ok, true)
  if (!parsed.ok) return
  assert.equal(parsed.query.canonical, 'query_id:-912345 AND database:"Sales \\"East\\""')
  assert.deepEqual(parsed.query.clauses.map(({ key, value }) => [key, value]), [
    ["query_id", "-912345"], ["database", 'Sales "East"'],
  ])
  assert.equal(withoutSearchClause(parsed.query, 0), 'database:"Sales \\"East\\""')
})

test("ordinary free text stays unambiguous and explicit text mixes with selectors", () => {
  const plain = parseSearch("select orders*", "pg_stat_statements")
  assert.equal(plain.ok && plain.query.freeText, "select orders*")
  const mixed = parseSearch('text:"select orders*" AND database:app', "pg_stat_statements")
  assert.equal(mixed.ok && mixed.query.canonical, 'text:"select orders*" AND database:app')
  assert.equal(parseSearch("select AND orders", "pg_stat_statements").ok, false)
})

test("invalid selectors identify the exact offending span and never become text", () => {
  const unknown = parseSearch("query_id:42 AND taname:foo", "pg_stat_statements")
  assert.equal(unknown.ok, false)
  if (unknown.ok) return
  assert.equal(unknown.error.code, "unknown_field")
  assert.equal("query_id:42 AND taname:foo".slice(unknown.error.start, unknown.error.end), "taname")
  for (const expression of ["query_id:*", "query_id:01", "query_id:42 OR query_id:43", 'database:"open', 'database:"bad\\n"']) {
    assert.equal(parseSearch(expression, "pg_stat_statements").ok, false, expression)
  }
  assert.equal(parseSearch('database:""', "pg_stat_statements").ok, false)
  assert.equal(parseSearch("queryid:42", "pg_stat_statements").ok, false)
  assert.equal(parseSearch("planid:42", "pg_store_plans").ok, false)
})

test("parser limits expression, clauses, values, and exact signed bigint text", () => {
  assert.equal(parseSearch("x".repeat(SEARCH_MAX_EXPRESSION + 1), "os_process").ok, false)
  assert.equal(parseSearch(`text:${"x".repeat(SEARCH_MAX_VALUE + 1)}`, "os_process").ok, false)
  assert.equal(parseSearch(Array.from({ length: SEARCH_MAX_CLAUSES + 1 }, () => "state:active").join(" AND "), "pg_stat_activity").ok, false)
  assert.equal(parseSearch("query_id:-9223372036854775808", "pg_stat_statements").ok, true)
  assert.equal(parseSearch("query_id:9223372036854775808", "pg_stat_statements").ok, false)
  assert.equal(parseSearch("pid:18446744073709551615", "os_process").ok, true)
  assert.equal(parseSearch("pid:18446744073709551616", "os_process").ok, false)
})

test("each surface exposes only its useful canonical public fields", () => {
  assert.deepEqual(searchFields("pg_stat_user_tables").map(({ key }) => key), ["text", "database", "schema", "table_name", "tablespace"])
  assert.deepEqual(searchFields("pg_store_plans").map(({ key }) => key), ["text", "query_id", "plan_id", "database", "role"])
  assert.equal(parseSearch("plan_id:42", "pg_stat_statements").ok, false)
  assert.equal(parseSearch("table_name:orders", "pg_stat_user_tables").ok, true)
  assert.equal(searchFields("pg_store_plans").some((field) => field.key.includes("queryid_stat")), false)
})

test("client matching is conjunctive, globbed only for strings, and fork-transparent", () => {
  const statement = row("pg_stat_statements", "1002002", { datname: "app", query: "select orders", queryid: "-912345", usename: "reader" })
  const parsed = parseSearch("database:app AND query_id:-912345", "pg_stat_statements")
  assert.equal(parsed.ok && rowMatchesSearch(statement, parsed.query, "pg_stat_statements"), true)
  const vadv = row("pg_store_plans", "1004001", { datname: "app", plan: "Index Scan", queryid: "0", queryid_stat_statements: "-912345", usename: "reader" })
  const planSearch = parseSearch("query_id:-912345", "pg_store_plans")
  assert.equal(planSearch.ok && rowMatchesSearch(vadv, planSearch.query, "pg_store_plans"), true)
  const zero = parseSearch("query_id:0", "pg_store_plans")
  assert.equal(zero.ok && rowMatchesSearch(vadv, zero.query, "pg_store_plans"), false)
  assert.equal(canonicalSearch([{ key: "database", value: "Sales Data" }, { key: "query_id", value: "-912345" }], "pg_store_plans"), 'database:"Sales Data" AND query_id:-912345')
})
