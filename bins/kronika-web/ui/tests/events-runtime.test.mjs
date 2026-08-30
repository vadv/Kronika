import assert from "node:assert/strict"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

import { registryPlugin } from "./import-module.mjs"

const directory = dirname(fileURLToPath(import.meta.url))
const databaseColumns = [
  ...Array.from({ length: 16 }, (_, index) => `pad${index}`), "deadlocks",
  "pad17", "pad18", "pad19", "frozen_xid_age", "min_mxid_age",
  "pad22", "pad23", "pad24", "checksum_failures",
  ...Array.from({ length: 6 }, (_, index) => `pad${26 + index}`), "sessions_fatal", "sessions_killed",
]
const compiled = await build({
  bundle: true,
  external: ["react", "react-dom", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  plugins: [registryPlugin([{
    columns: databaseColumns,
    identity: ["datid"],
    logicalName: "pg_stat_database",
    typeId: "1005004",
  }])],
  stdin: {
    contents: 'export { entryInScope, entryOf, groupMarks, MarkGroupRow } from "../src/events-view.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const events = loaded.exports

const translations = {
  "events.boundary.increased": "any increase from the previous value",
  "events.boundary.wraparound": "1,600,000,000 transactions or more",
  "events.source.database": "Databases",
  "pg.field.checksum_failures.help": "Checksum help text.",
  "pg.field.checksum_failures.label": "Checksum failures",
  "pg.field.min_mxid_age.help": "MultiXact help text.",
  "pg.field.min_mxid_age.label": "MultiXact age",
}
const t = (key) => translations[key] ?? key
const hour = Date.UTC(2026, 7, 23, 9) * 1_000

function finding(fieldOrdinal, rowOrdinal = "7") {
  return {
    category: null,
    fieldOrdinal,
    kind: "known_bad",
    logicalName: "pg_stat_database",
    rowOrdinal,
    segmentId: "segment-a",
    timestamp: hour + 120_000_000,
    typeId: "1005004",
  }
}

test("Events renders each finding metric label, help and boundary from findingMetric", () => {
  for (const expected of [
    { boundary: "any increase from the previous value", fieldOrdinal: 25, help: "Checksum help text.", label: "Checksum failures" },
    { boundary: "1,600,000,000 transactions or more", fieldOrdinal: 21, help: "MultiXact help text.", label: "MultiXact age" },
  ]) {
    const [group] = events.groupMarks([finding(expected.fieldOrdinal)], hour, t)
    assert.ok(group)
    const markup = renderToStaticMarkup(createElement(events.MarkGroupRow, {
      expanded: true,
      group,
      hour,
      locale: "en",
      onCursor() {},
      onFinding() {},
      onToggle() {},
      t,
    }))
    assert.match(markup, new RegExp(`data-testid="event-mark-label">${expected.label}<`))
    assert.match(markup, new RegExp(`data-testid="event-mark-boundary">${expected.boundary}<`))
    assert.match(markup, new RegExp(`data-testid="event-mark-help">${expected.help}<`))
    assert.match(markup, />Databases</)
  }
})

test("Events groups repeated crossings before rendering metric metadata", () => {
  const later = { ...finding(25, "8"), timestamp: hour + 180_000_000 }
  const [group] = events.groupMarks([later, finding(25)], hour, t)
  assert.ok(group)
  assert.equal(group.findings.length, 2)
  assert.equal(group.metric.label, "Checksum failures")
  assert.equal(group.metric.helpKey, "pg.field.checksum_failures.help")
  assert.equal(group.metric.boundary, "any increase from the previous value")
})

test("Events scopes compact groups by source without treating one locator as a member list", () => {
  const entry = {
    section: "pg_log_errors",
    detailLocator: {
      segment_id: "segment-a",
      type_id: "2001001",
      row_ordinal: "1",
    },
  }
  const representative = {
    category: null,
    fieldOrdinal: 0,
    kind: "event",
    logicalName: "pg_log_errors",
    rowOrdinal: "1",
    segmentId: "segment-a",
    timestamp: hour,
    typeId: "2001001",
  }
  const otherMember = { ...representative, rowOrdinal: "2", timestamp: hour + 1 }
  const otherSource = { ...otherMember, logicalName: "pg_log_checkpoints", typeId: "2002001" }
  const sameSourceThreshold = { ...otherMember, kind: "known_bad" }
  const threshold = { ...sameSourceThreshold, logicalName: "os_cpu", typeId: "1102001" }

  assert.equal(events.entryInScope(entry, [otherMember]), true)
  assert.equal(events.entryInScope(entry, [sameSourceThreshold, otherSource, threshold]), false)
  assert.equal(events.entryOf([entry], representative), entry)
  assert.equal(events.entryOf([entry], otherMember), null)
})
