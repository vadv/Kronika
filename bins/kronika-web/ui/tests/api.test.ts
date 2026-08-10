import assert from "node:assert/strict"
import { Buffer } from "node:buffer"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"

import { parseNdjson } from "../src/wire.ts"

test("a streamed error record rejects an otherwise successful response", () => {
  assert.throws(
    () => parseNdjson('{"record":"rows"}\n{"record":"error","error":"unreadable"}\n', "/api/example"),
    /unreadable.*\/api\/example/,
  )
})

const START = 1_800_000_000_000_000
const TEST_REGISTRY = [
  {
    typeId: "1105001",
    logicalName: "os_loadavg",
    columns: [{ name: "ts" }, { name: "load1" }, { name: "load5" }],
  },
  {
    typeId: "1005001",
    logicalName: "pg_stat_database",
    columns: [{ name: "ts" }, { name: "datid" }, { name: "datname" }, { name: "xact_commit" }],
  },
  {
    typeId: "2001001",
    logicalName: "pg_log_errors",
    columns: [{ name: "ts" }, { name: "message" }, { name: "category" }],
  },
]

test("the hour loader uses registry fields, bounded requests and exact locators", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  let active = 0
  let maximumActive = 0
  const requested = new Set<string>()
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    requested.add(`${url.pathname}?${url.searchParams.toString()}`)
    active += 1
    maximumActive = Math.max(maximumActive, active)
    await new Promise((resolve) => setImmediate(resolve))
    active -= 1
    if (url.pathname === "/api/catalog") {
      return ndjson([
        {
          record: "catalog",
          source_families: [
            { name: "os", configured: true, present: true },
            { name: "postgresql", configured: true, present: true },
          ],
        },
        {
          record: "finished_segment",
          id: "77",
          min_ts: String(START),
          max_ts: String(START + 9_000_000),
          sections: TEST_REGISTRY.map((layout) => ({ logical_name: layout.logicalName })),
        },
      ])
    }
    const match = /^\/api\/segments\/77\/sections\/([^/]+)\/(history|index)$/.exec(url.pathname)
    assert.notEqual(match, null)
    const logicalName = match?.[1] ?? ""
    if (match?.[2] === "index") return indexResponse(logicalName)
    return historyResponse(logicalName, url.searchParams.getAll("field"))
  }

  try {
    const hour = await api.loadHour(START, new AbortController().signal)
    assert.ok(maximumActive > 1)
    assert.ok(maximumActive <= 4)
    assert.deepEqual(hour.availableSections, [
      "os_loadavg", "pg_stat_database", "pg_log_errors", "health",
    ])
    assert.equal(hour.sections.os_loadavg?.[0]?.values.load1, 0)
    assert.equal(hour.sections.os_loadavg?.[0]?.values.load5, null)
    assert.equal(hour.pgDatabases[0]?.values.xact_commit, 0)
    assert.equal(hour.points[0]?.identity.datid, 7)
    const finding = hour.findings.find((candidate) => candidate.typeId === "2001001")
    assert.notEqual(finding, undefined)
    assert.equal(finding?.rowOrdinal, "12")
    assert.equal(finding?.fieldOrdinal, 2)
    const resolved = finding === undefined ? null : api.resolveLocator(hour, finding)
    assert.equal(resolved?.logicalName, "pg_log_errors")
    assert.equal(resolved?.fieldName, "category")
    assert.equal(resolved?.row.ordinal, "12")
    assert.equal(api.logicalNameForTypeId("1005001"), "pg_stat_database")
    assert.ok([...requested].some((path) =>
      path.includes("/os_loadavg/history?") && path.includes("field=load1") && path.includes("field=load5"),
    ))
  } finally {
    globalThis.fetch = originalFetch
  }
})

async function bundledApi() {
  const result = await build({
    entryPoints: [fileURLToPath(new URL("../src/api.ts", import.meta.url))],
    bundle: true,
    format: "esm",
    platform: "node",
    write: false,
    plugins: [{
      name: "test-registry",
      setup(context) {
        context.onResolve({ filter: /^kronika:registry$/ }, () => ({ path: "registry", namespace: "test" }))
        context.onLoad({ filter: /.*/, namespace: "test" }, () => ({
          contents: `export const registry=${JSON.stringify(TEST_REGISTRY)}`,
          loader: "js",
        }))
      },
    }],
  })
  const output = result.outputFiles[0]
  assert.notEqual(output, undefined)
  const source = Buffer.from(output?.contents ?? []).toString("base64")
  return import(`data:text/javascript;base64,${source}`)
}

function historyResponse(logicalName: string, fields: readonly string[]): Response {
  if (logicalName === "health") {
    return ndjson([
      { record: "layout", layout: { type_id: "0", columns: [{ name: "os_health" }] } },
      { record: "row", type_id: "0", ordinal: "3", timestamp: String(START + 1), values: [0] },
    ])
  }
  const layout = TEST_REGISTRY.find((candidate) => candidate.logicalName === logicalName)
  assert.notEqual(layout, undefined)
  const values = fields.map((field) => {
    if (field === "load1" || field === "xact_commit" || field === "category") return 0
    if (field === "datid") return 7
    return null
  })
  return ndjson([
    { record: "layout", layout: { type_id: layout?.typeId, columns: fields.map((name) => ({ name })) } },
    {
      record: "row",
      type_id: layout?.typeId,
      ordinal: logicalName === "pg_log_errors" ? "12" : "4",
      timestamp: String(START + 1),
      values,
    },
  ])
}

function indexResponse(logicalName: string): Response {
  if (logicalName === "pg_stat_database") {
    return ndjson([{
      record: "point",
      type_id: "1005001",
      series: "transactions_per_second",
      ts: String(START + 1),
      identity: { datid: 7 },
      value: 0,
    }])
  }
  if (logicalName === "pg_log_errors") {
    return ndjson([{
      record: "finding",
      kind: "event",
      type_id: "2001001",
      field_ordinal: 2,
      row_ordinal: 12,
      ts: String(START + 1),
      category: 0,
    }])
  }
  return ndjson([])
}

function ndjson(records: readonly unknown[]): Response {
  return new Response(`${records.map((record) => JSON.stringify(record)).join("\n")}\n`, { status: 200 })
}
