import assert from "node:assert/strict"
import test from "node:test"

import { globMatcher } from "../src/glob.ts"

test("a pattern without wildcards matches a substring", () => {
  const match = globMatcher("postgres")
  assert.ok(match !== null)
  assert.equal(match("postgres: checkpointer"), true)
  assert.equal(match("POSTGRES: walwriter"), true)
  assert.equal(match("kthreadd"), false)
})

test("wildcards anchor the pattern to the whole value", () => {
  const star = globMatcher("rcu*")
  assert.ok(star !== null)
  assert.equal(star("rcu_gp"), true)
  assert.equal(star("migration/1"), false)
  assert.equal(star("kworker rcu"), false)

  const single = globMatcher("cpuhp/?")
  assert.ok(single !== null)
  assert.equal(single("cpuhp/2"), true)
  assert.equal(single("cpuhp/12"), false)
})

test("regular expression characters in a pattern are literal", () => {
  const match = globMatcher("kworker/*:0H-*")
  assert.ok(match !== null)
  assert.equal(match("kworker/0:0H-events_highpri"), true)

  const dotted = globMatcher("10.100.0.4")
  assert.ok(dotted !== null)
  assert.equal(dotted("client 10.100.0.4:49518"), true)
  assert.equal(dotted("client 10a100b0c4"), false)
})

test("an empty pattern filters nothing", () => {
  assert.equal(globMatcher(""), null)
  assert.equal(globMatcher("   "), null)
})
