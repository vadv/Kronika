import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const plans = await importModule('export * from "../src/plan-presentation.ts"; export * from "../src/statement-navigation.ts"')

test("standard EXPLAIN JSON becomes a factual plan hierarchy", () => {
  const raw = JSON.stringify([{
    Plan: {
      "Node Type": "Hash Join", "Join Type": "Inner", "Plan Rows": 14, "Total Cost": 42.5,
      "Hash Cond": "(orders.customer_id = customers.id)",
      Plans: [
        { "Node Type": "Seq Scan", "Relation Name": "orders", Schema: "public", Filter: "(status = 'open')" },
        { "Node Type": "Index Scan", "Relation Name": "customers", Schema: "public", "Index Name": "customers_pkey", "Index Cond": "(id > 0)" },
      ],
    },
  }])
  const shown = plans.presentPlan(raw)
  assert.equal(shown.kind, "tree")
  assert.equal(shown.summary, "Hash Join → Seq Scan + Index Scan")
  assert.deepEqual(shown.root.children.map(({ nodeType, relation, index }) => [nodeType, relation, index]), [
    ["Seq Scan", "public.orders", null],
    ["Index Scan", "public.customers", "customers_pkey"],
  ])
  assert.deepEqual(shown.root.attributes.slice(0, 3), [
    { label: "Join Type", value: "Inner" },
    { label: "Total Cost", value: "42.5" },
    { label: "Plan Rows", value: "14" },
  ])
})

test("vadv compact nodes decode even when extension fragments follow the plan", () => {
  const raw = '{"p":{"t":"u","j":"l","7":"(a.id = b.id)","l":[{"t":"h","n":"orders","s":"public","5":"(open = true)"},{"t":"i","n":"customers","s":"public","i":"customers_pkey","8":"(id > 0)"}]}}{},"r":[]'
  const shown = plans.presentPlan(raw)
  assert.equal(shown.kind, "tree")
  assert.equal(shown.summary, "Merge Join → Seq Scan + Index Scan")
  assert.deepEqual(shown.root.attributes, [
    { label: "Join type", value: "Left" },
    { label: "Hash condition", value: "(a.id = b.id)" },
  ])
})

test("text plans stay readable and malformed payloads fall back honestly", () => {
  const text = "Index Scan using orders_pkey on orders  (cost=0.42..8.44 rows=1 width=8)\n  Index Cond: (id = 1)"
  assert.deepEqual(plans.presentPlan(text), { kind: "text", lines: text.split("\n"), summary: "Index Scan using orders_pkey on orders" })
  assert.equal(plans.presentPlan('{"unknown":true}').kind, "raw")
  assert.equal(plans.presentPlan("{broken").kind, "raw")
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
