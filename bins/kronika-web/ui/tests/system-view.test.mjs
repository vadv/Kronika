import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { gunzipSync } from "node:zlib"

import { importModule, registryPlugin } from "./import-module.mjs"

const helpers = await importModule(
  'export { chartableEntityColumns, currentValue, entityHistoryRequest, fallbackMetric, hasMetric, metricChartUnit, metricHistoryPoints, metricHistoryRequest, metricPoints, metricRequestKey, SYSTEM_METRICS, SYSTEM_REQUESTS } from "../src/system-view.tsx"; export { bundledFixtureHour } from "../src/fixture.ts"',
  { plugins: [registryPlugin([{ typeId: "1108001", logicalName: "os_diskstats", identity: ["major", "minor"], columns: ["ts", "major", "minor", "device", "io_in_progress"] }])] },
)

const rateHelpers = await importModule(
  'export { metricChartUnit, metricHistoryPoints } from "../src/system-view.tsx"',
  { plugins: [registryPlugin([{ typeId: "1108001", logicalName: "os_diskstats", identity: ["major", "minor"], columns: ["ts", "major", "minor", "reads"], columnMetadata: [
    { name: "ts", type: "timestamp_us", class: "timestamp", unit: null },
    { name: "major", type: "i32", class: "label", unit: null },
    { name: "minor", type: "i32", class: "label", unit: null },
    { name: "reads", type: "u64", class: "cumulative", unit: "count" },
  ] }])] },
)

const data = {
  points: [
    { segmentId: "a", series: "test", timestamp: 100, value: 5 },
    { segmentId: "a", series: "test", timestamp: 200, value: null },
    { segmentId: "a", series: "test", timestamp: 300, value: 0 },
  ],
}
const spec = { group: "cpu", help: "x.help", id: "x", label: "x.label", series: "test", unit: "" }

test("system current values use the stored observation at or before the cursor", () => {
  assert.equal(helpers.currentValue(data, spec, 150, "en"), "5")
  assert.equal(helpers.currentValue(data, spec, 200, "en"), "—")
  assert.equal(helpers.currentValue(data, spec, 300, "en"), "0")
  assert.equal(helpers.hasMetric({ points: [{ segmentId: "a", series: "test", timestamp: 1, value: null }] }, spec), false)
  assert.equal(helpers.hasMetric(data, spec), true)
  assert.equal(helpers.fallbackMetric("os_mountinfo"), null)
  assert.equal(helpers.fallbackMetric("os_diskstats"), null)
})

test("system histories omit rows whose layout does not own the selected field", () => {
  const direct = {
    points: [],
    sections: {
      os_vmstat: [
        { segmentId: "a", timestamp: 1, values: { oom_kill: 1 } },
        { segmentId: "a", timestamp: 2, values: { other: null } },
        { segmentId: "b", timestamp: 3, values: { oom_kill: null } },
      ],
    },
  }
  assert.deepEqual(helpers.metricPoints(direct, { ...spec, field: "oom_kill", section: "os_vmstat", series: "missing" }).map((point) => [point.timestamp, point.value]), [[1, 1], [3, null]])
})

test("system cards derive production values when fixture-only series are absent", () => {
  const cpu = (timestamp, user, system, idle) => ({
    logicalName: "os_cpu", ordinal: String(timestamp), segmentId: "a", timestamp, typeId: "1102001",
    values: { cpu_id: -1, idle, iowait: 0, irq: 0, nice: 0, scope: 0, softirq: 0, steal: 0, system, user },
  })
  const production = {
    points: [],
    health: [],
    load: [],
    memory: [{ logicalName: "os_meminfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1104001", values: { mem_available: 25, mem_total: 100 } }],
    pressure: [],
    sections: {
      os_cpu: [cpu(100, 10, 10, 80), cpu(200, 20, 20, 160)],
      os_meminfo: [{ logicalName: "os_meminfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1104001", values: { mem_available: 25, mem_total: 100 } }],
      os_mountinfo: [
        { logicalName: "os_mountinfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1112001", values: { free_bytes: 50, total_bytes: 100 } },
        { logicalName: "os_mountinfo", ordinal: "1", segmentId: "a", timestamp: 200, typeId: "1112001", values: { free_bytes: 20, total_bytes: 100 } },
      ],
      os_diskstats: [
        { logicalName: "os_diskstats", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1110001", values: { io_in_progress: 2 } },
        { logicalName: "os_diskstats", ordinal: "1", segmentId: "a", timestamp: 200, typeId: "1110001", values: { io_in_progress: 1 } },
      ],
      os_netdev: [
        { logicalName: "os_netdev", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1111001", values: { rx_bytes: 10, rx_drop: 0, rx_errs: 1, tx_bytes: 40, tx_drop: 2, tx_errs: 0 } },
        { logicalName: "os_netdev", ordinal: "1", segmentId: "a", timestamp: 200, typeId: "1111001", values: { rx_bytes: 20, rx_drop: 3, rx_errs: 0, tx_bytes: null, tx_drop: 0, tx_errs: 0 } },
      ],
      os_vmstat: [{ logicalName: "os_vmstat", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1106001", values: { oom_kill: 0 } }],
    },
  }
  const derived = (name) => helpers.metricPoints(production, { ...spec, derive: name, series: "missing" }).map((point) => point.value)
  assert.equal(helpers.currentValue(production, { ...spec, derive: "cpu_busy", series: "missing" }, 200, "en"), "20")
  const unknownScope = {
    ...production,
    sections: {
      os_cpu: [
        { ...cpu(100, 10, 10, 80), values: { ...cpu(100, 10, 10, 80).values, scope: null } },
        { ...cpu(200, 20, 20, 160), values: { ...cpu(200, 20, 20, 160).values, scope: null } },
      ],
    },
  }
  assert.deepEqual(helpers.metricPoints(unknownScope, { ...spec, derive: "cpu_busy", series: "missing" }), [])
  assert.equal(helpers.currentValue(production, { ...spec, derive: "mem_available_percent", series: "missing" }, 200, "en"), "25")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "filesystem_free_min", series: "missing" }, 200, "en"), "20")
  assert.equal(helpers.currentValue(production, { ...spec, field: "oom_kill", section: "os_vmstat", series: "missing" }, 200, "en"), "0")
  assert.deepEqual(derived("device_count"), [2])
  assert.deepEqual(derived("device_active_io"), [3])
  assert.deepEqual(derived("filesystem_count"), [2])
  assert.deepEqual(derived("interface_count"), [2])
  assert.deepEqual(derived("network_rx"), [null])
  assert.deepEqual(derived("network_tx"), [null])
  assert.deepEqual(derived("network_errors"), [null])
  assert.deepEqual(derived("network_drops"), [null])
})

test("System never depends on process rows loaded by another view", async () => {
  assert.equal(helpers.SYSTEM_REQUESTS.some(({ section }) => section === "os_process"), false)
  assert.equal(helpers.SYSTEM_METRICS.some(({ id }) => id.startsWith("process_")), false)

  const grouped = new Map(helpers.SYSTEM_METRICS.map((metric) => [metric.id, metric.group]))
  for (const id of ["device_count", "filesystem_count"]) assert.equal(grouped.get(id), "storage")
  for (const id of ["interface_count", "network_rx", "network_tx", "network_errors", "network_drops"]) assert.equal(grouped.get(id), "network")
  const disk = helpers.SYSTEM_REQUESTS.find(({ section }) => section === "os_diskstats")
  assert.ok(disk.fields.includes("major"))
  assert.ok(disk.fields.includes("minor"))
  const source = await readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8")
  assert.match(source, /rows\.length === 0 && activeContext === null/)
})

test("System history requests are selected-metric keys with exact physical inputs", () => {
  const direct = helpers.metricHistoryRequest({ ...spec, field: "oom_kill", section: "os_vmstat", series: undefined })
  assert.deepEqual(direct, { fields: ["oom_kill"], section: "os_vmstat", where: {} })
  const pressure = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_pressure")
  const pressureRequest = helpers.metricHistoryRequest(pressure)
  assert.deepEqual(pressureRequest.where, { resource: "0" })
  assert.ok(pressureRequest.fields.includes("some_avg10"))
  assert.ok(pressureRequest.fields.includes("resource"))
  const cpu = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_busy")
  const cpuRequest = helpers.metricHistoryRequest(cpu)
  for (const field of ["cpu_id", "scope", "user", "idle", "iowait"]) assert.ok(cpuRequest.fields.includes(field))
  assert.equal(helpers.metricRequestKey(100, cpu, cpuRequest), helpers.metricRequestKey(100, cpu, cpuRequest))
  assert.notEqual(helpers.metricRequestKey(100, cpu, cpuRequest), helpers.metricRequestKey(200, cpu, cpuRequest))
  assert.equal(helpers.metricChartUnit({ ...spec, unit: " KiB" }, "en"), "KiB")
  assert.equal(helpers.metricChartUnit({ ...spec, unit: " B" }, "en"), "bytes/s")
  assert.equal(helpers.metricChartUnit({ ...spec, unit: " B" }, "ru"), "байты/с")
  assert.equal(helpers.metricChartUnit({ ...spec, id: "network_errors" }, "en"), "1/s")
  assert.equal(helpers.metricChartUnit({ ...spec, id: "network_drops" }, "ru"), "1/с")
})

test("System entity charts include numeric measurements and exclude identities and categories", () => {
  const columns = [
    { field: "major", kind: "id" },
    { field: "device", kind: "text" },
    { field: "is_k8s_infra", kind: "boolean" },
    { field: "captured_at", kind: "timestamp" },
    { field: "reads", kind: "number" },
    { field: "read_time_ms", kind: "milliseconds" },
    { field: "total_bytes", kind: "bytes" },
  ]
  assert.deepEqual(helpers.chartableEntityColumns(columns).map(({ field }) => field), ["reads", "read_time_ms", "total_bytes"])
  const row = {
    logicalName: "os_diskstats", ordinal: "4", segmentId: "s", timestamp: 12, typeId: "1108001",
    values: { major: 8, minor: 0, reads: 0 },
  }
  const request = helpers.entityHistoryRequest(row, columns[4])
  assert.deepEqual(request.where, { major: "8", minor: "0" })
  assert.deepEqual(request.fields, ["reads", "major", "minor"])
  assert.equal(request.section, "os_diskstats")
  assert.equal(request.typeId, "1108001")
  assert.equal(helpers.entityHistoryRequest(row, columns[0]), null)
  assert.deepEqual(helpers.metricHistoryPoints({ ...spec, field: "reads", section: "os_diskstats", series: undefined }, [
    { ...row, timestamp: 1, values: { ...row.values, reads: 0 } },
    { ...row, timestamp: 2, values: { major: 8, minor: 0 } },
    { ...row, timestamp: 3, values: { ...row.values, reads: null } },
  ]).map(({ timestamp, value }) => [timestamp, value]), [[1, 0], [3, null]])
})

test("registry cumulative fields become reset-safe rates across storage segments", () => {
  const spec = { field: "reads", group: "storage", help: "x", id: "reads", label: "x", section: "os_diskstats", unit: "" }
  const row = (segmentId, timestamp, reads) => ({ logicalName: "os_diskstats", ordinal: String(timestamp), segmentId, timestamp, typeId: "1108001", values: { major: 8, minor: 0, reads } })
  assert.deepEqual(rateHelpers.metricHistoryPoints(spec, [
    row("a", 1_000_000, 10), row("a", 2_000_000, 14), row("b", 3_000_000, 20), row("b", 4_000_000, null), row("b", 5_000_000, 1), row("b", 6_000_000, 3),
  ]).map(({ value }) => value), [null, 4, 6, null, null, 2])
  assert.equal(rateHelpers.metricChartUnit(spec, "en"), "1/s")
})

test("the committed hour supplies only honest System metrics with complete histories", async () => {
  const encoded = await readFile(new URL("../fixtures/real-hour.json.gz", import.meta.url))
  const fixture = JSON.parse(gunzipSync(encoded).toString("utf8"))
  Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: fixture })
  const hourStart = Math.floor(Number(fixture.meta.captureFromUs) / 3_600_000_000) * 3_600_000_000
  const hour = helpers.bundledFixtureHour(hourStart)
  assert.notEqual(hour, null)

  const available = helpers.SYSTEM_METRICS.map((metric) => ({ metric, points: helpers.metricPoints(hour, metric) }))
    .filter(({ points }) => points.some((point) => point.value !== null && Number.isFinite(point.value)))
  assert.ok(available.length >= 9)
  assert.deepEqual([...new Set(available.map(({ metric }) => metric.group))], ["cpu", "load", "memory", "pressure", "storage"])

  const health = available.find(({ metric }) => metric.id === "health")?.points ?? []
  const expected = fixture.system.health.filter(([timestamp]) => Number(timestamp) >= hourStart && Number(timestamp) < hourStart + 3_600_000_000)
  assert.equal(health.length, expected.length)
  assert.equal(health[0]?.timestamp, Number(expected[0]?.[0]))
  assert.equal(health.at(-1)?.timestamp, Number(expected.at(-1)?.[0]))
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
})

test("System uses the audited balanced groups without a forced console floor", async () => {
  const [source, styles] = await Promise.all([
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.match(source, /\["cpu", "memory", "pressure"\]/)
  assert.match(source, /\["load", "storage", "network"\]/)
  assert.match(styles, /\.system-console \{[^}]*align-items: start;/)
  assert.doesNotMatch(styles, /\.system-console \{[^}]*min-height:/)
})
