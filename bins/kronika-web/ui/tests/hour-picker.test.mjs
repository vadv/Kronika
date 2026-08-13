import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const picker = await importModule(`export {
  hourHasData,
  pickerDateLabel,
  pickerFocusHour,
  pickerMonthDays,
  pickerMonthLabel,
  pickerMonthStart,
  pickerRangeLabel,
} from "../src/hour-picker.tsx"`)

const HOUR = 3_600_000_000
const START = Date.UTC(2026, 7, 10, 15) * 1_000
const DECEMBER = Date.UTC(2026, 11, 31, 23) * 1_000
const JANUARY = Date.UTC(2027, 0, 15, 8) * 1_000

test("the picker keeps one UTC calendar hour across date boundaries", () => {
  assert.equal(picker.pickerRangeLabel(START), "15:00–16:00")
  assert.equal(picker.pickerRangeLabel(Date.UTC(2026, 7, 10, 23) * 1_000), "23:00–00:00")
  assert.match(picker.pickerDateLabel(START, "en"), /Aug.*10.*2026/i)
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
  assert.equal(picker.pickerFocusHour(8, "ArrowRight", [8, 11, 23]), 11)
  assert.equal(picker.pickerFocusHour(11, "ArrowDown", [8, 11, 23]), 23)
  assert.equal(picker.pickerFocusHour(11, "Home", [8, 11, 23]), 8)
  assert.equal(picker.pickerFocusHour(11, "End", [8, 11, 23]), 23)
})

test("catalog endpoints mark captured hours without filling blank time", () => {
  const available = [START, START + HOUR]
  assert.equal(picker.hourHasData(START - HOUR, available), false)
  assert.equal(picker.hourHasData(START, available), true)
  assert.equal(picker.hourHasData(START + HOUR, available), true)
  assert.equal(picker.hourHasData(START + 2 * HOUR, available), false)
})

test("month calculations cross a year boundary", () => {
  const december = picker.pickerMonthStart(DECEMBER)
  const january = picker.pickerMonthStart(JANUARY)
  assert.ok(picker.pickerMonthDays(december).includes("2026-12-31"))
  assert.ok(picker.pickerMonthDays(january).includes("2027-01-15"))
  assert.equal(picker.pickerMonthDays(january).length, 31)
})

test("calendar month labels are human in both locales", () => {
  assert.match(picker.pickerMonthLabel(JANUARY, "en"), /January.*2027/i)
  assert.match(picker.pickerMonthLabel(JANUARY, "ru"), /январ.*2027/i)
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
  assert.match(pickerSource, /data-testid="picker-month"/)
  assert.doesNotMatch(pickerSource, /<button[^>]*data-testid="hour-current"/)
  assert.match(pickerSource, /pickerMonthDays\(month\)/)
  assert.match(pickerSource, /ALL_HOURS\.map/)
  assert.match(pickerSource, /aria-pressed=/)
  assert.match(pickerSource, /disabled=\{!hasData\}/)
  assert.match(pickerSource, /document\.addEventListener\("pointerdown"/)
  assert.match(pickerSource, /event\.key !== "Escape"/)
  assert.match(styles, /\.day-grid[^}]*grid-template-columns:\s*repeat\(7,/s)
  assert.match(styles, /\.hour-grid[^}]*grid-template-columns:\s*repeat\(6,/s)
  assert.match(styles, /\.hour-popover[^}]*calc\(100vw - 20px\)/s)
  assert.match(styles, /@media \(max-width: 760px\)/)
})
