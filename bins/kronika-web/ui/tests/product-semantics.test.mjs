import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const semantics = await importModule(
  'export { productSemantics, semantic } from "../src/product-semantics.ts"',
)

test("the accepted product registry has stable unique definitions", () => {
  assert.equal(semantics.productSemantics.length, 23)
  assert.equal(new Set(semantics.productSemantics.map((definition) => definition.id)).size, 23)

  const duration = semantics.semantic("value_tone.query_duration_ms", "numeric_value_tone")
  assert.equal(duration.origin, "accepted_presentation")
  assert.equal(duration.unit, "milliseconds")
  assert.deepEqual(duration.thresholds, [
    { operator: "gte", value: 5000, tone: "critical" },
    { operator: "gte", value: 1000, tone: "warning" },
  ])
  assert.deepEqual(duration.expected_band, { min_inclusive: null, max_exclusive: 1000 })
})

test("Vacuum, relation, and event policies keep exact accepted mappings", () => {
  const vacuum = semantics.semantic("vacuum.phase_risk", "vacuum_risk").policy
  assert.equal(vacuum.default, "ordinary")
  assert.deepEqual(vacuum.order, ["dangerous", "heavy", "ordinary"])
  assert.equal(vacuum.phases["truncating heap"], "dangerous")

  const relation = semantics.semantic("relation.index_state_severity", "relation_severity")
  assert.equal(relation.origin, "kronika_derived")
  assert.deepEqual(relation.policy.states, [
    { valid: true, ready: true, severity: 0 },
    { valid: true, ready: false, severity: 1 },
    { valid: false, ready: null, severity: 2 },
  ])

  const event = semantics.semantic("event.pg_log_errors.tier", "event_tier").policy
  assert.equal(event.provenance, "recorded")
  assert.deepEqual(event.tiers, ["notable", "critical", "critical", "routine", "routine"])
})

test("indexed findings and health keep their existing Rust authority", () => {
  assert.equal(semantics.productSemantics.some((definition) => definition.id.startsWith("finding.") || definition.id.startsWith("health.")), false)
})
