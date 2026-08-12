import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { gunzipSync } from "node:zlib"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  format: "esm",
  platform: "node",
  plugins: [{
    name: "registry",
    setup(context) {
      context.onResolve({ filter: /^kronika:registry$/ }, () => ({ namespace: "registry", path: "registry" }))
      context.onLoad({ filter: /.*/, namespace: "registry" }, () => ({ contents: "export const registry = []" }))
    },
  }],
  stdin: {
    contents: 'export { currentValue, fallbackMetric, hasMetric, metricPoints, SYSTEM_METRICS, SYSTEM_REQUESTS } from "../src/system-view.tsx"; export { bundledFixtureHour } from "../src/fixture.ts"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

const data = {
  points: [
    { segmentId: "a", series: "test", timestamp: 100, value: 5 },
    { segmentId: "a", series: "test", timestamp: 200, value: null },
    { segmentId: "a", series: "test", timestamp: 300, value: 0 },
  ],
}
const spec = { group: "cpu", help: "x.help", id: "x", label: "x.label", series: "test", unit: "" }

test("system current values preserve null and zero at the nearest observation", () => {
  assert.equal(helpers.currentValue(data, spec, 200, "en"), "—")
  assert.equal(helpers.currentValue(data, spec, 300, "en"), "0")
  assert.equal(helpers.hasMetric({ points: [{ segmentId: "a", series: "test", timestamp: 1, value: null }] }, spec), false)
  assert.equal(helpers.hasMetric(data, spec), true)
  assert.equal(helpers.fallbackMetric("os_mountinfo"), null)
  assert.equal(helpers.fallbackMetric("os_diskstats"), null)
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
  assert.deepEqual(derived("network_rx"), [30])
  assert.deepEqual(derived("network_tx"), [null])
  assert.deepEqual(derived("network_errors"), [1])
  assert.deepEqual(derived("network_drops"), [5])
})

test("System never depends on process rows loaded by another view", () => {
  assert.equal(helpers.SYSTEM_REQUESTS.some(({ section }) => section === "os_process"), false)
  assert.equal(helpers.SYSTEM_METRICS.some(({ id }) => id.startsWith("process_")), false)

  const grouped = new Map(helpers.SYSTEM_METRICS.map((metric) => [metric.id, metric.group]))
  for (const id of ["device_count", "filesystem_count"]) assert.equal(grouped.get(id), "storage")
  for (const id of ["interface_count", "network_rx", "network_tx", "network_errors", "network_drops"]) assert.equal(grouped.get(id), "network")
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
