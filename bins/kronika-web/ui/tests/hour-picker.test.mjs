import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const picker = await importModule('export { hourHasData, pickerDateLabel, pickerFocusHour, pickerRangeLabel } from "../src/hour-picker.tsx"')

const HOUR = 3_600_000_000
const START = Date.UTC(2026, 7, 10, 15) * 1_000

test("the picker keeps one UTC calendar hour across date boundaries", () => {
  assert.equal(picker.pickerRangeLabel(START), "15:00–16:00")
  assert.equal(picker.pickerRangeLabel(Date.UTC(2026, 7, 10, 23) * 1_000), "23:00–00:00")
  assert.match(picker.pickerDateLabel(START, "en"), /10.*Aug.*2026/i)
  assert.match(picker.pickerDateLabel(START, "ru"), /10.*авг.*2026/i)
})

test("the six-column hour grid has bounded keyboard navigation", () => {
  assert.equal(picker.pickerFocusHour(7, "ArrowLeft"), 6)
  assert.equal(picker.pickerFocusHour(7, "ArrowRight"), 8)
  assert.equal(picker.pickerFocusHour(7, "ArrowUp"), 1)
  assert.equal(picker.pickerFocusHour(7, "ArrowDown"), 13)
  assert.equal(picker.pickerFocusHour(1, "ArrowUp"), 0)
  assert.equal(picker.pickerFocusHour(22, "ArrowDown"), 23)
  assert.equal(picker.pickerFocusHour(12, "Home"), 0)
  assert.equal(picker.pickerFocusHour(12, "End"), 23)
  assert.equal(picker.pickerFocusHour(12, "Enter"), null)
})

test("catalog endpoints mark captured hours without filling blank time", () => {
  const available = [START, START + HOUR]
  assert.equal(picker.hourHasData(START - HOUR, available), false)
  assert.equal(picker.hourHasData(START, available), true)
  assert.equal(picker.hourHasData(START + HOUR, available), true)
  assert.equal(picker.hourHasData(START + 2 * HOUR, available), false)
})

test("the combined picker has no native separate date or hour controls", async () => {
  const [pickerSource, appSource, styles] = await Promise.all([
    readFile(new URL("../src/hour-picker.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(`${pickerSource}\n${appSource}`, /<input\b|<select\b|type=["']date["']|type=["']time["']/)
  assert.match(pickerSource, /data-testid="hour-picker-trigger"/)
  assert.match(pickerSource, /data-testid="hour-popover"/)
  assert.match(pickerSource, /Array\.from\(\{ length: 24 \}/)
  assert.match(pickerSource, /aria-pressed=/)
  assert.match(pickerSource, /document\.addEventListener\("pointerdown"/)
  assert.match(pickerSource, /event\.key !== "Escape"/)
  assert.match(styles, /\.hour-grid[^}]*grid-template-columns:\s*repeat\(6,/s)
  assert.match(styles, /\.hour-popover[^}]*calc\(100vw - 20px\)/s)
})
