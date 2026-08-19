import assert from "node:assert/strict"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const iconPlugin = {
  name: "icons",
  setup(context) {
    context.onResolve({ filter: /^lucide-react$/ }, () => ({ namespace: "icons", path: "icons" }))
    context.onLoad({ filter: /.*/, namespace: "icons" }, () => ({ contents: "export const Copy=()=>null" }))
  },
}
const displayTimePlugin = {
  name: "display-time",
  setup(context) {
    context.onResolve({ filter: /display-time-context$/ }, () => ({ namespace: "display-time", path: "display-time" }))
    context.onLoad({ filter: /.*/, namespace: "display-time" }, () => ({ contents: "export const useDisplayTime=()=>({timestamp:(value)=>String(value)})" }))
  },
}
const plans = await importModule(
  'export * from "../src/plan-text.ts"; export * from "../src/plan-view.tsx"; export * from "../src/statement-navigation.ts"',
  { plugins: [iconPlugin, displayTimePlugin] },
)
const queryHelpers = await importModule(
  'export { recordedQueryTexts } from "../src/plan-query.ts"',
  { plugins: [registryPlugin([])] },
)
const t = (key) => key

const nativePlan = [
  "",
  "  Merge Join  (cost=0.85..81.42 rows=12 width=64)",
  "    Merge Cond: (orders.customer_id = customers.id)",
  "    ->  Seq Scan on orders  (cost=0.00..20.00 rows=400 width=32)",
  "    ->  Index Scan using customers_pkey on customers  (cost=0.42..8.44 rows=1 width=32)",
].join("\n")

test("native text plans use their first non-empty line as the dense summary", () => {
  assert.equal(plans.planTextSummary(nativePlan), "Merge Join  (cost=0.85..81.42 rows=12 width=64)")
  assert.equal(plans.planTextSummary(`  ${"x".repeat(100)}`), `${"x".repeat(83)}…`)
})

test("the plan detail presents the exact server text once", () => {
  const markup = renderToStaticMarkup(createElement(plans.PlanView, { raw: nativePlan, t }))
  assert.match(markup, /data-testid="pg-text-plan"/)
  assert.match(markup, /Merge Join  \(cost=0\.85\.\.81\.42 rows=12 width=64\)/)
  assert.match(markup, /-&gt;  Seq Scan on orders/)
  assert.equal((markup.match(/data-testid="pg-text-plan"/g) ?? []).length, 1)
  assert.doesNotMatch(markup, /<details/)
})

test("null and blank plans have an honest unavailable state", () => {
  for (const raw of [null, "", " \n\t "]) {
    assert.equal(plans.planTextSummary(raw), null)
    const summary = renderToStaticMarkup(createElement(plans.PlanSummary, { raw }))
    const detail = renderToStaticMarkup(createElement(plans.PlanView, { raw, t }))
    assert.match(summary, />—<\/span>/)
    assert.match(detail, /data-testid="pg-plan-unavailable"/)
    assert.doesNotMatch(detail, /data-testid="pg-text-plan"|<button/)
  }
})

const row = (typeId, values) => ({ logicalName: "pg_store_plans", ordinal: "0", segmentId: "s", timestamp: 10, typeId, values })

test("plan navigation uses public shared IDs for OSSC and Datasentinel and the statement ID for vadv", () => {
  const identity = { datname: "app db", dbid: 7, planid: "22", usename: "reader", userid: 8 }
  for (const typeId of ["1003001", "1018001"]) {
    const shared = plans.statementsForPlan(row(typeId, { ...identity, queryid: "11" }))
    assert.deepEqual(shared, { expression: 'database:"app db" AND role:reader AND query_id:11', queryId: "11", section: "statements" })
  }
  const last = plans.statementsForPlan(row("1004001", { ...identity, queryid: 0, queryid_stat_statements: "33" }))
  assert.deepEqual(last, { expression: 'database:"app db" AND role:reader AND query_id:33', queryId: "33", section: "statements" })
  assert.equal(plans.statementsForPlan(row("1004001", { ...identity, queryid: 0, queryid_stat_statements: 0 })), null)
  assert.equal(plans.statementsForPlan(row("1003001", { ...identity, queryid: 0 })), null)
  assert.equal(plans.statementsForPlan(row("1003001", { ...identity, dbid: 0, queryid: 11 })), null)
  assert.equal(plans.statementsForPlan(row("1003001", { ...identity, queryid: 11, userid: null })), null)
  assert.deepEqual(plans.plansForPlanId(row("1004001", identity)), { expression: "plan_id:22", planId: "22", section: "plans" })
})

test("recorded plan query texts keep exact bytes, deduplicate identical rows, and retain every distinct text", () => {
  const recorded = (ordinal, timestamp, query) => ({
    logicalName: "pg_stat_statements", ordinal, segmentId: ordinal === "3" ? "old" : "current", timestamp,
    typeId: "1002002", values: { datname: "app", dbid: 20, query, queryid: "42", toplevel: true, userid: 10, usename: "reader" },
  })
  const first = "  SELECT *\n  FROM jobs\n"
  const second = `select '${"x".repeat(900)}'`
  const texts = queryHelpers.recordedQueryTexts([
    recorded("1", 30, first), recorded("2", 30, first), recorded("3", 29, second), recorded("4", 30, null),
  ])
  assert.deepEqual(texts.map(({ occurrences, text }) => [occurrences, text]), [[2, first], [1, second]])
  assert.equal(texts[0]?.database, "app")
  assert.equal(texts[0]?.role, "reader")

  const markup = renderToStaticMarkup(createElement(plans.QueryView, {
    retry() {}, status: "ready", texts, t,
  }))
  assert.equal((markup.match(/data-testid="pg-plan-query-text"/g) ?? []).length, 2)
  assert.match(markup, /  SELECT \*/)
  assert.match(markup, /pg\.query\.plan\.identical/)
  assert.equal((markup.match(/pg\.plan\.copy/g) ?? []).length, 2)
  assert.match(markup, /max-h-\[min\(320px,35vh\)\] overflow-auto/)
})

test("query failure and unavailable states remain separate from the execution plan", () => {
  for (const status of ["unavailable", "no_bridge", "error"]) {
    const query = renderToStaticMarkup(createElement(plans.QueryView, { retry() {}, status, texts: [], t }))
    assert.match(query, new RegExp(`data-query-status="${status}"`))
    assert.match(query, new RegExp(`pg\\.query\\.plan\\.${status}`))
  }
  const plan = renderToStaticMarkup(createElement(plans.PlanView, { raw: nativePlan, t }))
  assert.match(plan, /data-testid="pg-text-plan"/)
})

test("inline plan query retrieval never joins the visible Statements rows", async () => {
  const [querySource, viewSource] = await Promise.all([
    readFile(new URL("../src/plan-query.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
  ])
  assert.match(querySource, /loadRelatedStatementTextRows\(segments, cursor, target\.expression/)
  assert.doesNotMatch(querySource, /allRows|data\.sections|pg_stat_statements\s*\?\?/)
  assert.match(viewSource, /<PlanTextBlocks cursor=\{cursor\} plan=\{wholeText\} revision=\{historyRevision\} row=\{row\} segments=\{segments\}/)
  assert.doesNotMatch(viewSource, /<PlanTextBlocks[^>]*allRows=/)
})

test("Activity navigation shows every related statement candidate for database and query ID", () => {
  const target = plans.statementsForActivity({ ...row("1001004", { datid: 16384, datname: "app", query_id: "-42" }), logicalName: "pg_stat_activity" })
  assert.deepEqual(target, { expression: "database:app AND query_id:-42", queryId: "-42", section: "statements" })
  assert.equal(plans.statementsForActivity({ ...row("1001004", { datid: 16384, datname: "app", query_id: 0 }), logicalName: "pg_stat_activity" }), null)
  assert.equal(plans.statementsForActivity({ ...row("1001004", { datid: null, datname: "app", query_id: 42 }), logicalName: "pg_stat_activity" }), null)
})

test("Statement navigation opens every matching plan without selecting one", () => {
  const statement = { ...row("1002002", { datname: "app", dbid: 7, queryid: "-42", usename: "reader", userid: 8 }), logicalName: "pg_stat_statements" }
  assert.deepEqual(plans.plansForStatement(statement), {
    expression: "database:app AND role:reader AND query_id:-42", queryId: "-42", section: "plans",
  })
})
