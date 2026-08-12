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
    columns: ["ts", "load1", "load5"],
  },
  {
    typeId: "1005001",
    logicalName: "pg_stat_database",
    columns: ["ts", "datid", "datname", "xact_commit"],
  },
  {
    typeId: "2001001",
    logicalName: "pg_log_errors",
    columns: ["ts", "message", "category"],
  },
  {
    typeId: "1002001",
    logicalName: "pg_stat_statements",
    identity: ["queryid", "userid", "dbid"],
    columns: ["ts", "queryid", "userid", "dbid", "query", "calls", "total_time", "rows"],
  },
  {
    typeId: "1002002",
    logicalName: "pg_stat_statements",
    identity: ["queryid", "userid", "dbid"],
    columns: ["ts", "queryid", "userid", "dbid", "query", "calls", "total_exec_time", "rows", "wal_bytes"],
  },
  {
    typeId: "1003001",
    logicalName: "pg_store_plans",
    identity: ["userid", "dbid", "queryid", "planid"],
    columns: ["ts", "userid", "dbid", "queryid", "planid", "plan", "calls", "total_time"],
  },
]

test("snapshot rows use each physical layout's positional columns", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.pathname, "/api/segments/77/snapshot")
    assert.deepEqual(url.searchParams.getAll("section"), ["os_loadavg"])
    return ndjson([
      {
        record: "layout",
        layout: {
          type_id: "1105001",
          logical_name: "os_loadavg",
          columns: [{ name: "load1" }, { name: "load5" }],
        },
      },
      {
        record: "layout",
        layout: {
          type_id: "1105002",
          logical_name: "os_loadavg",
          columns: [{ name: "load5" }, { name: "load1" }],
        },
      },
      {
        record: "row", type_id: "1105001", ordinal: "4",
        timestamp: String(START + 1), values: [1, 5],
      },
      {
        record: "row", type_id: "1105002", ordinal: "8",
        timestamp: String(START + 2), values: [50, 10],
      },
    ])
  }

  try {
    const hour = await api.loadSnapshot("77", START, ["os_loadavg"], new AbortController().signal)
    assert.equal(hour.load.length, 2)
    assert.deepEqual(hour.load[0]?.values, { load1: 1, load5: 5 })
    assert.deepEqual(hour.load[1]?.values, { load5: 50, load1: 10 })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a curated snapshot follows the registry layout and physical order", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const statement = {
    section: "pg_stat_statements",
    typeIds: ["1002001", "1002002"],
    top: 200,
    defaultOrder: ["total_time", "total_exec_time"],
    order: { wal_demand: ["wal_bytes"] },
    fallbackOrder: ["calls"],
    fieldsByType: {
      "1002001": ["queryid", "userid", "dbid", "query", "calls", "total_time", "wal_bytes"],
      "1002002": ["queryid", "userid", "dbid", "query", "calls", "total_exec_time", "wal_bytes"],
    },
  }
  const segment = {
    id: "77", minTs: START, maxTs: START + 10,
    sections: [{ logicalName: "pg_stat_statements", typeId: "1002001" }],
  }
  const requests = api.requestsForSegment([statement], segment)
  assert.equal(requests.length, 1)
  assert.equal(requests[0]?.typeId, "1002001")
  assert.deepEqual(requests[0]?.fields, ["queryid", "userid", "dbid", "query", "calls", "total_time"])
  assert.deepEqual(requests[0]?.defaultOrder, ["total_time"])
  assert.deepEqual(requests[0]?.order, { wal_demand: [] })
  assert.deepEqual(requests[0]?.fallbackOrder, ["calls"])

  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.deepEqual(url.searchParams.getAll("section"), ["pg_stat_statements"])
    assert.deepEqual(url.searchParams.getAll("field"), ["queryid", "userid", "dbid", "query", "calls", "total_time"])
    assert.deepEqual(url.searchParams.getAll("by"), ["calls"])
    assert.equal(url.searchParams.get("top"), "200")
    assert.equal(url.searchParams.get("type_id"), "1002001")
    assert.equal(url.searchParams.get("text"), "160")
    assert.equal(url.searchParams.has("plan"), false)
    return ndjson([
      {
        record: "layout", rates: ["calls", "total_time"],
        layout: {
          type_id: "1002001", logical_name: "pg_stat_statements",
          columns: ["queryid", "userid", "dbid", "query", "calls", "total_time"].map((name) => ({ name })),
        },
      },
      {
        record: "row", type_id: "1002001", ordinal: "3", timestamp: String(START),
        values: ["41", "10", "20", { stored_text: "select 1", original_length: 8 }, 2, 3],
      },
    ])
  }
  try {
    const hour = await api.loadSnapshot("77", START, requests, new AbortController().signal, {
      column: "wal_demand", descending: true,
    })
    assert.equal(hour.pgStatements.length, 1)
    assert.deepEqual(hour.rateColumns.pg_stat_statements, ["calls", "total_time"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("physical execution aliases are selected before the calls fallback", async () => {
  const api = await bundledApi()
  const statement = {
    section: "pg_stat_statements",
    top: 200,
    defaultOrder: ["total_time", "total_exec_time"],
    fallbackOrder: ["calls"],
    fieldsByType: {
      "1002001": ["queryid", "calls", "total_time"],
      "1002002": ["queryid", "calls", "total_exec_time"],
    },
  }
  const v1 = api.requestsForSegment([statement], {
    id: "v1", minTs: START, maxTs: START,
    sections: [{ logicalName: "pg_stat_statements", typeId: "1002001" }],
  })
  const v2 = api.requestsForSegment([statement], {
    id: "v2", minTs: START, maxTs: START,
    sections: [{ logicalName: "pg_stat_statements", typeId: "1002002" }],
  })
  assert.deepEqual(v1[0]?.defaultOrder, ["total_time"])
  assert.deepEqual(v2[0]?.defaultOrder, ["total_exec_time"])

  const seen: URL[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    seen.push(new URL(String(input), "http://kronika.invalid"))
    return ndjson([])
  }
  try {
    await api.loadSnapshot("v1", START, v1, new AbortController().signal)
    await api.loadSnapshot("v2", START, v2, new AbortController().signal)
    await api.loadSnapshot("v2", START, v2, new AbortController().signal, {
      column: "total_exec_time", descending: true,
    })
    assert.deepEqual(seen[0]?.searchParams.getAll("by"), ["total_time"])
    assert.deepEqual(seen[1]?.searchParams.getAll("by"), ["total_exec_time"])
    assert.deepEqual(seen[2]?.searchParams.getAll("by"), ["total_exec_time"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("exact V1 and V2 query text requests carry only query and exact identity", async () => {
  const api = await bundledApi()
  const originalFetch = globalThis.fetch
  const seen: URL[] = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(url)
    const typeId = url.searchParams.get("type_id")
    return ndjson([
      {
        record: "layout",
        layout: { type_id: typeId, logical_name: "pg_stat_statements", columns: [{ name: "query" }] },
      },
      { record: "row", type_id: typeId, ordinal: "1", timestamp: String(START), values: ["select exact"] },
    ])
  }
  try {
    for (const typeId of ["1002001", "1002002"]) {
      const hour = await api.loadSnapshot(
        "77",
        START,
        [{ section: "pg_stat_statements", fields: ["query"], typeId }],
        new AbortController().signal,
        undefined,
        { filters: { queryid: "9007199254740999", userid: "10", dbid: "20" }, typeId, fullText: true },
      )
      assert.equal(hour.pgStatements[0]?.values.query, "select exact")
    }
    for (const [index, typeId] of ["1002001", "1002002"].entries()) {
      const url = seen[index]
      assert.equal(url?.searchParams.get("type_id"), typeId)
      assert.deepEqual(url?.searchParams.getAll("field"), ["query"])
      assert.equal(url?.searchParams.has("text"), false)
      assert.equal(url?.searchParams.get("where.queryid"), "9007199254740999")
      assert.equal(url?.searchParams.get("where.userid"), "10")
      assert.equal(url?.searchParams.get("where.dbid"), "20")
      assert.equal(url?.searchParams.has("where.toplevel"), false)
    }
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("an exact locator uses the generic projected snapshot contract", async () => {
  const api = await bundledApi()
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.deepEqual(url.searchParams.getAll("field"), ["queryid", "query"])
    assert.equal(url.searchParams.get("type_id"), "1002002")
    assert.equal(url.searchParams.get("row_ordinal"), "9223372036854775807")
    assert.equal(url.searchParams.get("text"), "160")
    assert.equal(url.searchParams.has("top"), false)
    assert.equal(url.searchParams.has("by"), false)
    return ndjson([
      {
        record: "layout",
        layout: {
          type_id: "1002002", logical_name: "pg_stat_statements",
          columns: [{ name: "queryid" }, { name: "query" }],
        },
      },
      {
        record: "row", type_id: "1002002", ordinal: "9223372036854775807",
        timestamp: String(START), values: ["9", "select located"],
      },
    ])
  }
  try {
    const hour = await api.loadSnapshot(
      "77",
      START,
      [{ section: "pg_stat_statements", typeId: "1002002", fields: ["queryid", "query"] }],
      new AbortController().signal,
      undefined,
      { rowOrdinal: "9223372036854775807" },
    )
    assert.equal(hour.pgStatements[0]?.ordinal, "9223372036854775807")
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

function ndjson(records: readonly unknown[]): Response {
  return new Response(`${records.map((record) => JSON.stringify(record)).join("\n")}\n`, { status: 200 })
}

test("the bundled review fixture answers detail reads without HTTP", async () => {
  const api = await bundledApi()
  Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: apiFixture() })
  const originalFetch = globalThis.fetch
  let fetches = 0
  globalThis.fetch = async () => {
    fetches += 1
    throw new Error("the bundled fixture reached HTTP")
  }
  try {
    const timeline = await api.loadTimeline(START, new AbortController().signal)
    assert.deepEqual(
      [...new Set(timeline.lanePoints.map((point) => point.lane))].sort(),
      ["cpu_busy", "memory", "pg_oldest_xact", "pg_running", "pg_waiting"],
    )
    assert.equal(timeline.lanePoints.find((point) => point.lane === "memory")?.value, 40)
    const series = await api.loadSeries(
      START,
      "os_process",
      { pid: "41", starttime: "99" },
      ["read_bytes"],
      new AbortController().signal,
    )
    assert.equal(series.length, 2)
    assert.deepEqual(series[1]?.values, { read_bytes: 160, pid: 41, starttime: "99" })
    const snapshot = await api.loadSnapshot(
      "segment-a",
      START + 2,
      [{ section: "pg_stat_activity", fields: ["query"], typeId: "1001003" }],
      new AbortController().signal,
      undefined,
      { filters: { pid: "7" }, typeId: "1001003", fullText: true },
    )
    assert.deepEqual(snapshot.activities[0]?.values, { query: "select exact", pid: 7 })
    const beforeFirstSample = await api.loadSnapshot(
      "segment-a",
      START,
      ["pg_stat_activity"],
      new AbortController().signal,
    )
    assert.equal(beforeFirstSample.activities.length, 0)
    const repeated = await api.loadSnapshot(
      "segment-a",
      START + 2,
      [
        { section: "pg_stat_activity", fields: ["pid"], typeId: "1001003" },
        { section: "pg_stat_activity", fields: ["query"], typeId: "1001003" },
      ],
      new AbortController().signal,
    )
    assert.deepEqual(repeated.activities.map((row) => row.values), [{ pid: 7 }, { query: "select exact" }])
    const boundary = apiFixture()
    const secondHour = START + 3_600_000_000
    boundary.meta.captureToUs = String(secondHour + 2)
    boundary.os.snapshots.push({
      segment_id: "segment-a",
      ts: String(secondHour + 1),
      type_id: "1100001",
      rows: [[String(secondHour + 1), "6", 42, "100", 170]],
    })
    boundary.system.cpuBusy.push([String(secondHour + 1), 35, "segment-a"])
    Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: boundary })
    const selectedHour = await api.loadTimeline(secondHour, new AbortController().signal)
    assert.equal(selectedHour.hour, secondHour)
    assert.ok(selectedHour.lanes.os_process?.every((row) =>
      row.timestamp >= secondHour && row.timestamp < secondHour + 3_600_000_000))
    assert.ok(selectedHour.points.every((point) =>
      point.timestamp >= secondHour && point.timestamp < secondHour + 3_600_000_000))
    assert.equal(fetches, 0)
  } finally {
    globalThis.fetch = originalFetch
    Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  }
})

function apiFixture() {
  const point = (offset: number, value: number | null) => [String(START + offset), value, "segment-a"]
  return {
    meta: {
      captureFromUs: String(START),
      captureToUs: String(START + 2),
      segments: 1,
    },
    findings: [],
    os: {
      columns: ["ts", "ordinal", "pid", "starttime", "read_bytes"],
      snapshots: [
        {
          segment_id: "segment-a",
          ts: String(START + 1),
          type_id: "1100001",
          rows: [[String(START + 1), "4", 41, "99", 100]],
        },
        {
          segment_id: "segment-a",
          ts: String(START + 2),
          type_id: "1100001",
          rows: [[String(START + 2), "5", 41, "99", 160]],
        },
      ],
    },
    pg: {
      columns: [
        "ts", "ordinal", "pid", "leader_pid", "backend_type", "state",
        "wait_event_type", "xact_start", "query",
      ],
      snapshots: [{
        segment_id: "segment-a",
        ts: String(START + 1),
        type_id: "1001003",
        rows: [[String(START + 1), "8", 7, null, "client backend", "active", null, String(START), "select exact"]],
      }],
    },
    system: {
      cpuBusy: [point(1, 25)],
      health: [point(1, 75)],
      load1: [point(1, 1)],
      memAvailable: [point(1, 60)],
      minFsFree: [point(1, 80)],
      oom: [point(1, 0)],
      psi: {},
    },
  }
}

test("a snapshot is keyed on the stored sample the cursor rests on", async () => {
  const api = await bundledApi()
  const line = [START + 3_000_000, START + 6_000_000, START + 9_000_000].map((timestamp, index) => ({
    segmentId: "7", logicalName: "health", typeId: "0", ordinal: String(index), timestamp, values: {},
  }))
  assert.equal(api.sampleAt(line, START + 7_500_000), START + 6_000_000, "between samples the earlier one answers")
  assert.equal(api.sampleAt(line, START + 6_000_000), START + 6_000_000, "on a sample it is itself")
  assert.equal(api.sampleAt(line, START + 1_000_000), START + 3_000_000, "before the first, the first")
  assert.equal(api.sampleAt([], START), null)
})

test("a snapshot replaces the section it carries instead of piling moments up", async () => {
  const api = await bundledApi()
  const row = (timestamp: number) => ({
    segmentId: "7", logicalName: "os_process", typeId: "1100001", ordinal: "0", timestamp, values: { pid: 1 },
  })
  const before = api.hourOf({
    hour: START, availableHours: [START], segments: [], lanes: { os_process: [row(START + 1)] }, health: [],
    points: [], lanePoints: [], findings: [], sourceFamilies: [], availableSections: ["os_process"],
  })
  const after = api.replaceSections(before, {
    sections: { os_process: [row(START + 2)] }, availableSections: ["os_process"],
    points: [], lanePoints: [], findings: [], sourceFamilies: [], segmentCount: 1,
  })
  assert.equal(after.processes.length, 1)
  assert.equal(after.processes[0].timestamp, START + 2)
})

test("section findings replace only their section and keep a stable locator order", async () => {
  const api = await bundledApi()
  const finding = (logicalName: string, segmentId: string, rowOrdinal: string, fieldOrdinal: number) => ({
    segmentId, logicalName, kind: "spike" as const, typeId: "1002002", timestamp: START + 1,
    category: null, rowOrdinal, fieldOrdinal,
  })
  const before = api.hourOf({
    hour: START, availableHours: [START], segments: [], lanes: {}, health: [], points: [], lanePoints: [],
    findings: [finding("health", "a", "0", 1), finding("pg_stat_statements", "old", "9", 10)],
    sourceFamilies: [], availableSections: ["pg_stat_statements"],
  })
  const after = api.replaceFindings(before, "pg_stat_statements", [
    finding("pg_stat_statements", "b", "2", 11),
    finding("pg_stat_statements", "a", "2", 10),
  ])
  assert.deepEqual(after.findings.map((item) => [item.logicalName, item.segmentId, item.fieldOrdinal]), [
    ["health", "a", 1],
    ["pg_stat_statements", "a", 10],
    ["pg_stat_statements", "b", 11],
  ])
})

test("a table loads its sparse finding indexes from each matching segment", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  const seen: string[] = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(url.pathname)
    const segmentId = url.pathname.split("/")[3]
    return ndjson([
      { record: "findings", type_id: "1002002", total_hits: "1", truncated: false },
      {
        record: "finding", kind: "spike", type_id: "1002002", field_ordinal: 10,
        row_ordinal: segmentId === "7" ? "5" : "3", ts: String(START + (segmentId === "7" ? 2 : 1)),
      },
    ])
  }
  try {
    const findings = await api.loadSectionFindings([
      { id: "7", minTs: START, maxTs: START + 10, sections: [{ logicalName: "pg_stat_statements", typeId: "1002002" }] },
      { id: "8", minTs: START + 11, maxTs: START + 20, sections: [{ logicalName: "pg_stat_statements", typeId: "1002002" }] },
      { id: "9", minTs: START + 21, maxTs: START + 30, sections: [{ logicalName: "os_process", typeId: "1100001" }] },
    ], "pg_stat_statements", new AbortController().signal)
    assert.deepEqual(seen, [
      "/api/segments/7/sections/pg_stat_statements/index",
      "/api/segments/8/sections/pg_stat_statements/index",
    ])
    assert.deepEqual(findings.map((item) => [item.segmentId, item.logicalName, item.rowOrdinal]), [
      ["8", "pg_stat_statements", "3"],
      ["7", "pg_stat_statements", "5"],
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("timeline lanes retain their segment and a recorded null", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async () => ndjson([
    { record: "hour", from: String(START), to: String(START + 3_600_000_000 - 1), available_hours: [String(START)] },
    {
      record: "finished_segment", id: "segment-a", min_ts: String(START), max_ts: String(START + 10),
      sections: [{ logical_name: "pg_stat_statements", type_id: "1002002" }],
    },
    { record: "index", segment: { id: "segment-a" }, logical_name: "health", checksum: null },
    { record: "lane", segment_id: "segment-a", lane: "cpu_busy", ts: String(START + 1), value: null },
  ])
  try {
    const timeline = await api.loadTimeline(START, new AbortController().signal)
    assert.deepEqual(timeline.lanePoints, [{ segmentId: "segment-a", lane: "cpu_busy", timestamp: START + 1, value: null }])
    assert.deepEqual(timeline.segments[0]?.sections, [{ logicalName: "pg_stat_statements", typeId: "1002002" }])
    assert.equal(api.fieldNameForLocator({ typeId: "0", fieldOrdinal: 0 }), "os_health")
    assert.equal(api.fieldNameForLocator({ typeId: "0", fieldOrdinal: 1 }), "overall_health")
    assert.equal(api.fieldNameForLocator({ typeId: "0", fieldOrdinal: 2 }), null)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("snapshot segment selection never jumps from blank early time to the final segment", async () => {
  const api = await bundledApi()
  const segments = [
    { id: "first", minTs: 100, maxTs: 200, sections: [] },
    { id: "second", minTs: 300, maxTs: 400, sections: [] },
  ]
  assert.equal(api.segmentBoundAt(segments, 50), null)
  assert.equal(api.segmentBoundAt(segments, 150)?.id, "first")
  assert.equal(api.segmentBoundAt(segments, 250)?.id, "first")
  assert.equal(api.segmentBoundAt(segments, 350)?.id, "second")
})

test("projected history retains segment provenance and exact type", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.pathname, "/api/hour")
    assert.deepEqual(url.searchParams.getAll("field"), ["calls", "total_exec_time"])
    assert.equal(url.searchParams.get("type_id"), "1002002")
    assert.equal(url.searchParams.get("where.queryid"), "41")
    return ndjson([
      { record: "series_segment", segment: { id: "segment-a" } },
      {
        record: "layout",
        layout: {
          type_id: "1002002", logical_name: "pg_stat_statements",
          columns: [{ name: "calls" }, { name: "total_exec_time" }],
        },
      },
      { record: "row", type_id: "1002002", ordinal: "2", timestamp: String(START + 1), values: [2, 8] },
      { record: "series_segment", segment: { id: "segment-b" } },
      { record: "row", type_id: "1002002", ordinal: "3", timestamp: String(START + 2), values: [3, 12] },
    ])
  }
  try {
    const rows = await api.loadSeries(
      START,
      "pg_stat_statements",
      { queryid: "41" },
      ["calls", "total_exec_time"],
      new AbortController().signal,
      "1002002",
    )
    assert.deepEqual(rows.map((row) => [row.segmentId, row.typeId]), [
      ["segment-a", "1002002"],
      ["segment-b", "1002002"],
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})
