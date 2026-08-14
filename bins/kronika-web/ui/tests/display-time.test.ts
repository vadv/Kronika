import assert from "node:assert/strict"
import test from "node:test"

import {
  DISPLAY_TIME_ZONE_KEY,
  calendarMonthDays,
  createDisplayTimeFormatter,
  displayTimeZone,
  loadDisplayTimeZone,
  saveDisplayTimeZone,
} from "../src/display-time.ts"

test("display time defaults to Browser and persists only the explicit preference", () => {
  assert.equal(displayTimeZone(null), "browser")
  assert.equal(displayTimeZone("unexpected"), "browser")
  assert.equal(displayTimeZone("utc"), "utc")
  assert.equal(loadDisplayTimeZone({ getItem: () => { throw new Error("blocked") } }), "browser")
  const saved: [string, string][] = []
  saveDisplayTimeZone({ setItem: (key, value) => saved.push([key, value]) }, "utc")
  assert.deepEqual(saved, [[DISPLAY_TIME_ZONE_KEY, "utc"]])
})

test("mode switching formats the same exact instant and omits subseconds", () => {
  const instant = Date.UTC(2026, 7, 14, 5, 30, 45, 987) * 1_000 + 654
  const browser = createDisplayTimeFormatter("en", "browser", "Europe/Moscow")
  const utc = createDisplayTimeFormatter("en", "utc", "Europe/Moscow")
  assert.equal(browser.clock(instant), "08:30:45 GMT+3")
  assert.equal(utc.clock(instant), "05:30:45 UTC")
  assert.equal(browser.hourRange(instant).primary, "08:30–09:30 GMT+3")
  assert.equal(utc.hourRange(instant).primary, "05:30–06:30 UTC")
  assert.equal(instant, 1_786_685_445_987_654)
  assert.doesNotMatch(browser.timestamp(instant), /\.987|654/)
  assert.doesNotMatch(utc.timestamp(instant), /\.987|654/)
})

test("civil grouping follows date boundaries and a half-hour browser offset", () => {
  const instant = Date.UTC(2026, 7, 14, 1) * 1_000
  const losAngeles = createDisplayTimeFormatter("en", "browser", "America/Los_Angeles")
  const india = createDisplayTimeFormatter("en", "browser", "Asia/Kolkata")
  const utc = createDisplayTimeFormatter("en", "utc", "America/Los_Angeles")
  assert.equal(losAngeles.dayKey(instant), "2026-08-13")
  assert.equal(utc.dayKey(instant), "2026-08-14")
  assert.equal(india.hourLabel(instant), "06:30 GMT+5:30")
  assert.equal(india.dayKey(instant), "2026-08-14")
})

test("DST repeated local hours remain distinct by offset", () => {
  const eastern = createDisplayTimeFormatter("en", "browser", "America/New_York")
  const first = Date.UTC(2026, 10, 1, 5) * 1_000
  const second = Date.UTC(2026, 10, 1, 6) * 1_000
  assert.equal(eastern.hourLabel(first), "01:00 GMT-4")
  assert.equal(eastern.hourLabel(second), "01:00 GMT-5")
  assert.equal(eastern.dayKey(first), eastern.dayKey(second))
  assert.equal(eastern.hourRange(first).primary, "01:00 GMT-4–01:00 GMT-5")
  assert.notEqual(first, second)
})

test("calendar rendering enumerates civil dates without constructing selectable hours", () => {
  assert.equal(calendarMonthDays("2027-02").length, 28)
  assert.deepEqual(calendarMonthDays("bad"), [])
})
