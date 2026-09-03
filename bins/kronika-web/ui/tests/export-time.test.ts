import assert from "node:assert/strict"
import test from "node:test"

import { exportRangeDefaults, formatExportSecond, parseExportRange } from "../src/export-time.ts"

test("UTC defaults cover the selected hour through its inclusive last second", () => {
  const hour = Date.UTC(2026, 7, 14, 23) * 1_000
  const defaults = exportRangeDefaults(hour, "utc")
  assert.deepEqual(defaults, {
    from: "2026-08-14T23:00:00",
    fromSecond: 1_786_748_400,
    to: "2026-08-14T23:59:59",
    toSecond: 1_786_751_999,
  })
  assert.deepEqual(parseExportRange(defaults.from, defaults.to, "utc"), {
    ok: true,
    from: defaults.fromSecond,
    to: defaults.toSecond,
  })
})

test("whole-second UTC values round-trip across days and before the Unix epoch", () => {
  assert.equal(formatExportSecond(-1, "utc"), "1969-12-31T23:59:59")
  assert.deepEqual(parseExportRange("1969-12-31T23:59:59", "1970-01-01T00:00:01", "utc"), {
    ok: true,
    from: -1,
    to: 1,
  })
  assert.deepEqual(parseExportRange("1970-01-01T00:00", "1970-01-01T00:01", "utc"), {
    ok: true,
    from: 0,
    to: 60,
  })
  assert.deepEqual(parseExportRange("2026-08-14T23:59:59", "2026-08-16T00:00:00", "utc"), {
    ok: true,
    from: Date.UTC(2026, 7, 14, 23, 59, 59) / 1_000,
    to: Date.UTC(2026, 7, 16) / 1_000,
  })
})

test("Browser mode parses and formats the browser's civil time", () => {
  const previous = process.env.TZ
  process.env.TZ = "Europe/Moscow"
  try {
    const second = Date.UTC(2026, 7, 14, 5, 30, 45) / 1_000
    assert.equal(formatExportSecond(second, "browser"), "2026-08-14T08:30:45")
    assert.deepEqual(parseExportRange("2026-08-14T08:30:45", "2026-08-14T08:31:00", "browser"), {
      ok: true,
      from: second,
      to: second + 15,
    })
  } finally {
    if (previous === undefined) delete process.env.TZ
    else process.env.TZ = previous
  }
})

test("civil round-trip validation rejects gaps, normalization and subseconds", () => {
  const previous = process.env.TZ
  process.env.TZ = "America/New_York"
  try {
    for (const [from, to, error] of [
      ["", "2026-01-01T00:00:00", "required"],
      ["2026-02-30T00:00:00", "2026-03-01T00:00:00", "invalid"],
      ["2026-01-01T00:00:00.500", "2026-01-01T00:00:01", "invalid"],
      ["2026-03-08T02:30:00", "2026-03-08T03:30:00", "invalid"],
      ["2026-01-02T00:00:00", "2026-01-01T23:59:59", "order"],
    ] as const) assert.deepEqual(parseExportRange(from, to, "browser"), { ok: false, error })
  } finally {
    if (previous === undefined) delete process.env.TZ
    else process.env.TZ = previous
  }
})

test("a repeated Browser hour retains the exact selected-hour defaults", () => {
  const previous = process.env.TZ
  process.env.TZ = "America/New_York"
  try {
    const hour = Date.UTC(2026, 10, 1, 6) * 1_000
    const defaults = exportRangeDefaults(hour, "browser")
    assert.equal(defaults.from, "2026-11-01T01:00:00")
    assert.equal(defaults.to, "2026-11-01T01:59:59")
    assert.deepEqual(parseExportRange(defaults.from, defaults.to, "browser", {
      from: defaults.fromSecond,
      to: defaults.toSecond,
    }), {
      ok: true,
      from: Date.UTC(2026, 10, 1, 6) / 1_000,
      to: Date.UTC(2026, 10, 1, 6, 59, 59) / 1_000,
    })
  } finally {
    if (previous === undefined) delete process.env.TZ
    else process.env.TZ = previous
  }
})
