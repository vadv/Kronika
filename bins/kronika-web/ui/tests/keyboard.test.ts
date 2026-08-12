import assert from "node:assert/strict"
import test from "node:test"

import { moveCursor, ownsArrowKeys } from "../src/keyboard.ts"

test("global arrows move one minute and stay inside the selected hour", () => {
  const hour = 3_600_000_000
  assert.equal(moveCursor(hour, hour, "ArrowLeft"), hour)
  assert.equal(moveCursor(hour, hour, "ArrowRight"), hour + 60_000_000)
  assert.equal(moveCursor(hour + 3_599_999_000, hour, "ArrowRight"), hour + 3_599_999_000)
  assert.equal(moveCursor(hour + 1, hour, "Enter"), hour + 1)
})

test("editing controls retain their arrow keys", () => {
  for (const tag of ["button", "input", "select", "textarea"]) assert.equal(ownsArrowKeys(tag, false), true)
  assert.equal(ownsArrowKeys("div", true), true)
  assert.equal(ownsArrowKeys("div", false), false)
})
