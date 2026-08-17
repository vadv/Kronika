import assert from "node:assert/strict"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const iconPlugin = {
  name: "icons",
  setup(context) {
    context.onResolve({ filter: /^lucide-react$/ }, () => ({ namespace: "icons", path: "icons" }))
    context.onLoad({ filter: /.*/, namespace: "icons" }, () => ({ contents: "export const Copy=()=>null" }))
  },
}
const plans = await importModule(
  'export * from "../src/plan-text.ts"; export * from "../src/plan-view.tsx"; export * from "../src/statement-navigation.ts"',
  { plugins: [iconPlugin] },
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

test("plan navigation uses exact IDs for OSSC and best-effort last IDs for vadv", () => {
  const exact = plans.statementTargetForPlan(row("1003001", { dbid: 7, planid: "22", queryid: "11", userid: 8 }))
  assert.deepEqual(exact, { dbId: "7", match: "exact", planId: "22", queryId: "11", sourceTypeId: "1003001", topLevel: null, userId: "8" })
  const last = plans.statementTargetForPlan(row("1004001", { dbid: 7, planid: "22", queryid: 0, queryid_stat_statements: "33", userid: 8 }))
  assert.deepEqual(last, { dbId: "7", match: "last", planId: "22", queryId: "33", sourceTypeId: "1004001", topLevel: null, userId: "8" })
  assert.deepEqual(plans.statementTargetFilters(last), { dbid: "7", queryid: "33", userid: "8" })
  assert.equal(plans.statementTargetForPlan(row("1004001", { dbid: 7, planid: "22", queryid: 0, queryid_stat_statements: 0, userid: 8 })), null)
})
