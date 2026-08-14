import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const stylesheet = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
const slowQueryColumns = ["ts", "pattern", "sample", "count", "max_duration_ms", "total_duration_ms"]
const presentation = await importModule(
  'export { findingDetailFields } from "../src/finding-presentation.ts"',
  { plugins: [registryPlugin([{
    columns: slowQueryColumns,
    identity: [],
    logicalName: "pg_log_slow_queries",
    typeId: "2004001",
  }])] },
)

function blockAfter(marker, source = stylesheet) {
  const start = source.indexOf(marker)
  assert.notEqual(start, -1, `missing ${marker}`)
  const opening = source.indexOf("{", start)
  let depth = 0
  for (let index = opening; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1
    if (source[index] === "}") depth -= 1
    if (depth === 0) return source.slice(opening + 1, index)
  }
  assert.fail(`unterminated ${marker}`)
}

test("all detail key/value rows share a readable label track and bounded value track", () => {
  const row = blockAfter(".detail-list > div, .pg-detail dl > div, .event-detail dl > div")
  assert.match(row, /grid-template-columns:\s*minmax\(10rem, 30%\) minmax\(0, 1fr\)/)

  const label = blockAfter(".detail-list dt, .pg-detail dt, .event-detail dt")
  assert.match(label, /hyphens:\s*none/)
  assert.match(label, /overflow:\s*hidden/)
  assert.match(label, /overflow-wrap:\s*normal/)
  assert.match(label, /word-break:\s*normal/)

  const value = blockAfter(".detail-list dd, .pg-detail dd, .event-detail dd")
  assert.match(value, /min-width:\s*0/)
  assert.match(value, /overflow-wrap:\s*anywhere/)
  assert.match(stylesheet, /\.pg-detail dd, \.event-detail dd \{[^}]*font-variant-numeric:\s*tabular-nums;[^}]*text-align:\s*right;/)
})

test("narrow detail rows stack labels above values", () => {
  const narrow = blockAfter("@media (max-width: 520px)")
  const row = blockAfter(".detail-list > div, .pg-detail dl > div, .event-detail dl > div", narrow)
  assert.match(row, /align-items:\s*start/)
  assert.match(row, /grid-template-columns:\s*minmax\(0, 1fr\)/)
  assert.match(narrow, /\.detail-list dd, \.pg-detail dd, \.event-detail dd \{ text-align: left; \}/)
})

test("slow-query detail keeps text followed by compact numeric values", () => {
  const row = {
    logicalName: "pg_log_slow_queries",
    ordinal: "3",
    segmentId: "segment-a",
    timestamp: 10,
    typeId: "2004001",
    values: {
      ts: 10,
      pattern: "select * from orders where id = ?",
      sample: "select * from orders where id = 42",
      count: 3,
      max_duration_ms: 3_831,
      total_duration_ms: 7_662,
    },
  }
  const finding = {
    category: null,
    fieldOrdinal: 0,
    kind: "event",
    logicalName: "pg_log_slow_queries",
    rowOrdinal: "3",
    segmentId: "segment-a",
    timestamp: 10,
    typeId: "2004001",
  }

  assert.deepEqual(presentation.findingDetailFields(row, finding), [
    ["pattern", row.values.pattern],
    ["sample", row.values.sample],
    ["count", 3],
    ["max_duration_ms", 3_831],
    ["total_duration_ms", 7_662],
  ])
})
