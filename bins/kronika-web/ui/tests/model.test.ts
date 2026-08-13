import assert from "node:assert/strict"
import test from "node:test"

import type { Cell, DataRow } from "../src/api.ts"
import { fittedWidth } from "../src/column-size.ts"
import { activityFor, compact, estimatedRows, formatUtc, humanBytes, identifier, localHourPair, localTimePair, measure, nearestTime, processCommand, processDefaultSort, processKey, processLens, rawText, shownMoment, selectedHour, stateText, type Locale } from "../src/model.ts"

function row(timestamp: number): DataRow {
  return { segmentId: "7", logicalName: "os_process", typeId: "1100001", ordinal: "0", timestamp, values: {} }
}

test("UTC hour selection and timestamp presentation are exact", () => {
  const hour = selectedHour("2026-02-03", 4)
  assert.equal(hour, Date.UTC(2026, 1, 3, 4) * 1_000)
  const timestamp = Date.UTC(2026, 1, 3, 14, 44, 5, 678) * 1_000 + 901
  assert.equal(formatUtc(timestamp), "2026-02-03 14:44:05.678 UTC")
})

test("nearest stored time chooses the earlier value on a tie", () => {
  assert.equal(nearestTime([row(100), row(300)], 200), 100)
})

test("null is a dash while zero remains data and identifiers are ungrouped", () => {
  assert.equal(measure(null, "en"), "—")
  assert.equal(measure(0, "en"), "0")
  assert.equal(identifier("1234567"), "1234567")
  assert.equal(stateText(82), "R")
})

test("text payloads retain their exact text value", () => {
  const payload: Cell = { representation: "text", stored_text: "SELECT 1", full_len: "8", truncated: true, sha256: "abc" }
  assert.equal(rawText(payload), "SELECT 1")
})

test("integer lists retain exact ungrouped identifiers", () => {
  assert.equal(rawText([1234, 5678]), "[1234,5678]")
})

test("activity linking uses the cursor-nearest PG snapshot and exact PID", () => {
  const process = { ...row(100), values: { pid: 77, comm: "worker", cmdline: null } }
  const activityAt100 = { ...row(100), values: { pid: 77, backend_type: "old" } }
  const activityAt300 = { ...row(300), values: { pid: 77, backend_type: "nearest" } }
  const activityOtherPid = { ...row(300), ordinal: "1", values: { pid: 78, backend_type: "other" } }
  const linked = activityFor(process, [activityAt100, activityAt300, activityOtherPid], 290)
  assert.equal(linked.snapshotTime, 300)
  assert.equal(linked.row?.values.backend_type, "nearest")
  assert.equal(processCommand(process), "worker")
})

test("finding fields select the matching process lens", () => {
  assert.equal(processLens("read_bytes"), "disk")
  assert.equal(processLens("rmem_kb"), "memory")
  assert.equal(processLens("rundelay_ns"), "cpu")
  assert.equal(processLens("state"), "generic")
})

test("process identity includes the captured start time", () => {
  const process = (pid: number, starttime: string): DataRow => ({
    ...row(100),
    values: { pid, starttime },
  })
  assert.equal(processKey(process(41, "99")), processKey(process(41, "99")))
  assert.notEqual(processKey(process(41, "99")), processKey(process(41, "100")))
})

test("process tables start with CPU and disk sorting chooses the first active signal", () => {
  const process = (values: DataRow["values"]): DataRow => ({ ...row(100), values })
  assert.equal(processDefaultSort("cpu", []), "utime")
  assert.equal(processDefaultSort("disk", [process({ read_bytes: 0, write_bytes: 4, syscr: 9 })]), "write_bytes")
  assert.equal(processDefaultSort("disk", [process({ read_bytes: 0, write_bytes: 0, syscr: 9 })]), "syscr")
  assert.equal(processDefaultSort("disk", [process({ read_bytes: 0, write_bytes: 0, syscr: 0, syscw: 0 })]), "read_bytes")
})

test("counters that arrive as rates are read in units a person thinks in", async () => {
  const model = await import("../src/model.ts")
  assert.equal(model.humanBytes(1_572_864, "en"), "1.5 MiB")
  assert.equal(model.humanBytes(512, "en"), "512 B")
  assert.equal(model.humanBytes(null, "en"), "—")
  assert.equal(model.cores(161.01, "en", 100), "1.61")
  assert.equal(model.cores(161.01, "en", null), "—")
  assert.equal(model.cores(161.01, "en", 0), "—")
  assert.equal(model.millisecondsPerSecond(111_622_111.53, "en"), "111.6")
})

test("byte values use locale-aware binary units without compact decimal words", () => {
  const bytes = 19_757_000_000
  assert.equal(humanBytes(bytes, "en"), "18.4 GiB")
  assert.equal(humanBytes(bytes, "ru"), "18,4 GiB")
  assert.doesNotMatch(humanBytes(bytes, "ru"), /млрд Б/)
  assert.equal(identifier(String(bytes)), "19757000000")
})

test("the shown moment is the last sample at or before the cursor", () => {
  const row = (timestamp: number) => ({ segmentId: "s", logicalName: "os_cpu", typeId: "1", ordinal: "0", timestamp, values: {} })
  const sections = { os_cpu: [row(10), row(30)], pg_stat_activity: [row(20), row(90)] }

  assert.equal(shownMoment(sections, 50), 30)
  assert.equal(shownMoment(sections, 90), 90)
  assert.equal(shownMoment(sections, 5), null)
  assert.equal(shownMoment({}, 50), null)
})

test("a fitted column takes the widest cell within bounds", () => {
  assert.equal(fittedWidth(200), 212)
  assert.equal(fittedWidth(0), 64)
  assert.equal(fittedWidth(4000), 720)
  assert.equal(fittedWidth(120.2), 133)
})

test("metric numbers use three significant digits and locale-aware compact scales", () => {
  assert.equal(compact(0, "en"), "0")
  assert.equal(compact(0.03, "en"), "0.03")
  assert.equal(compact(9.876, "en"), "9.88")
  assert.equal(compact(999, "en"), "999")
  assert.equal(compact(1407.48, "en"), "1.41K")
  assert.equal(compact(9999, "en"), "10K")
  assert.equal(compact(21_471, "en"), "21.5K")
  assert.equal(compact(3_052_945.27, "en"), "3.05M")
  assert.equal(compact(452_000_000, "en"), "452M")
  assert.equal(compact(-21_471, "en"), "-21.5K")
  assert.equal(compact(4.5e12, "en"), "4.5T")
  assert.equal(compact(Number.MAX_VALUE, "en"), "1.8E308")
  assert.equal(compact(Number.NaN, "en"), "—")
  assert.equal(compact(Number.POSITIVE_INFINITY, "en"), "—")
  assert.equal(compact(1407.48, "ru"), "1,41 тыс.")
  assert.equal(compact(8_117_857, "ru"), "8,12 млн")
  assert.equal(compact(1_360_000_000, "ru"), "1,36 млрд")
  assert.equal(measure(1407.48, "en", " ms/s"), "1.41K ms/s")
  assert.equal(identifier("9007199254740993"), "9007199254740993")
})

function rowTranslator(locale: Locale) {
  const copy = locale === "ru"
    ? { one: "≈{value} строка", few: "≈{value} строки", many: "≈{value} строк" }
    : { one: "≈{value} row", few: "≈{value} rows", many: "≈{value} rows" }
  return (key: string, slots: Readonly<Record<string, string | number>> = {}) => copy[key.slice(key.lastIndexOf(".") + 1) as keyof typeof copy].replace("{value}", String(slots.value))
}

test("estimated row gauges keep compact and exact bigint labels", () => {
  assert.deepEqual(estimatedRows(713_456, "en", rowTranslator("en")), { primary: "≈713K rows", secondary: "≈713,456 rows" })
  assert.deepEqual(estimatedRows(12_876, "ru", rowTranslator("ru")), { primary: "≈12,9 тыс. строк", secondary: "≈12 876 строк" })
  assert.equal(estimatedRows(999, "en", rowTranslator("en"))?.primary, "≈999 rows")
  assert.equal(estimatedRows(1_000, "en", rowTranslator("en"))?.primary, "≈1K rows")
  assert.equal(estimatedRows(999_499, "en", rowTranslator("en"))?.primary, "≈999K rows")
  assert.equal(estimatedRows(999_500, "en", rowTranslator("en"))?.primary, "≈1M rows")
  assert.equal(estimatedRows("9994999999999999", "en", rowTranslator("en"))?.primary, "≈9.99E15 rows")
  assert.equal(estimatedRows("9007199254740993", "en", rowTranslator("en"))?.secondary, "≈9,007,199,254,740,993 rows")
  assert.equal(estimatedRows(null, "ru", rowTranslator("ru")), null)
})

test("estimated row exact labels use bigint-safe EN and RU plurals", () => {
  for (const [value, suffix] of [[0, "rows"], [1, "row"], [2, "rows"]] as const) {
    assert.equal(estimatedRows(value, "en", rowTranslator("en"))?.secondary, `≈${value} ${suffix}`)
  }
  for (const [value, suffix] of [[11, "строк"], [12, "строк"], [14, "строк"], [21, "строка"], [22, "строки"], [25, "строк"]] as const) {
    assert.equal(estimatedRows(value, "ru", rowTranslator("ru"))?.secondary, `≈${value} ${suffix}`)
  }
})

test("browser-local labels retain UTC context without a UTC duplicate", () => {
  const instant = Date.UTC(2026, 7, 14, 5, 30, 0, 123) * 1_000
  assert.deepEqual(localTimePair(instant, "en", "UTC"), { primary: "05:30:00.123 UTC", secondary: null })
  assert.deepEqual(localTimePair(instant, "en", "America/New_York"), { primary: "01:30:00.123 EDT", secondary: "05:30:00.123 UTC" })
})

test("local hour endpoints survive DST folds, skips, and date boundaries", () => {
  assert.deepEqual(localHourPair(Date.UTC(2026, 2, 8, 6) * 1_000, "en", "America/New_York"), {
    date: "Mar 08, 2026", primary: "01:00 EST–03:00 EDT", secondary: "06:00–07:00 UTC",
  })
  assert.deepEqual(localHourPair(Date.UTC(2026, 10, 1, 5) * 1_000, "en", "America/New_York"), {
    date: "Nov 01, 2026", primary: "01:00 EDT–01:00 EST", secondary: "05:00–06:00 UTC",
  })
  assert.deepEqual(localHourPair(Date.UTC(2026, 7, 14, 3) * 1_000, "en", "America/New_York"), {
    date: "Aug 13, 2026–Aug 14, 2026", primary: "23:00–00:00 EDT", secondary: "Aug 14, 2026 · 03:00–04:00 UTC",
  })
  assert.match(localHourPair(Date.UTC(2026, 10, 1, 5) * 1_000, "ru", "America/New_York").primary, /^01:00 GMT-4–01:00 GMT-5$/)
})
