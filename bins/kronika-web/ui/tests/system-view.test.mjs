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
    contents: 'export { currentValue, fallbackMetric, hasMetric, metricPoints, SYSTEM_METRICS } from "../src/system-view.tsx"; export { bundledFixtureHour } from "../src/fixture.ts"',
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

test("system groups use dense real process metrics with strict null handling", () => {
  const process = (timestamp, ordinal, values) => ({
    logicalName: "os_process", ordinal, segmentId: "a", timestamp, typeId: "1100001", values,
  })
  const processes = [
    process(100, "0", { blkdelay_ticks: 0, majflt: 0, minflt: 2, nivcsw: 3, num_threads: 2, nvcsw: 4, read_bytes: null, rmem_kb: 10, rundelay_ns: 5, state: "R", vmem_kb: 20, vswap_kb: 0, write_bytes: null }),
    process(100, "1", { blkdelay_ticks: 7, majflt: 1, minflt: 3, nivcsw: 1, num_threads: 3, nvcsw: 2, read_bytes: 9, rmem_kb: 30, rundelay_ns: 6, state: "D", vmem_kb: 40, vswap_kb: 4, write_bytes: 8 }),
    process(200, "2", { blkdelay_ticks: 0, majflt: 0, minflt: 0, nivcsw: 0, num_threads: 1, nvcsw: 0, read_bytes: 0, rmem_kb: 0, rundelay_ns: 0, state: "S", vmem_kb: 0, vswap_kb: 0, write_bytes: 0 }),
  ]
  const hour = { points: [], processes, sections: {} }
  const derived = (name) => helpers.metricPoints(hour, { derive: name, group: "cpu", help: "x.help", id: name, label: "x.label", series: "missing", unit: "" }).map((point) => point.value)

  assert.deepEqual(derived("process_count"), [2, 1])
  assert.deepEqual(derived("process_running"), [1, 0])
  assert.deepEqual(derived("process_blocked"), [1, 0])
  assert.deepEqual(derived("process_threads"), [5, 1])
  assert.deepEqual(derived("process_context_switches"), [10, 0])
  assert.deepEqual(derived("process_resident"), [40, 0])
  assert.deepEqual(derived("process_swap"), [4, 0])
  assert.deepEqual(derived("process_read"), [null, 0])

  const grouped = new Map(helpers.SYSTEM_METRICS.map((metric) => [metric.id, metric.group]))
  for (const id of ["process_count", "process_running", "process_threads", "process_context_switches", "process_run_delay"]) assert.equal(grouped.get(id), "cpu")
  for (const id of ["process_resident", "process_virtual", "process_swap", "process_major_faults"]) assert.equal(grouped.get(id), "memory")
  for (const id of ["device_count", "filesystem_count"]) assert.equal(grouped.get(id), "storage")
  for (const id of ["interface_count", "network_rx", "network_tx", "network_errors", "network_drops"]) assert.equal(grouped.get(id), "network")
})

test("the committed hour supplies a dense honest System set and complete selected histories", async () => {
  const encoded = await readFile(new URL("../fixtures/real-hour.json.gz", import.meta.url))
  const fixture = JSON.parse(gunzipSync(encoded).toString("utf8"))
  Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: fixture })
  const hourStart = Math.floor(Number(fixture.meta.captureFromUs) / 3_600_000_000) * 3_600_000_000
  const hour = helpers.bundledFixtureHour(hourStart)
  assert.notEqual(hour, null)

  const available = helpers.SYSTEM_METRICS.map((metric) => ({ metric, points: helpers.metricPoints(hour, metric) }))
    .filter(({ points }) => points.some((point) => point.value !== null && Number.isFinite(point.value)))
  assert.ok(available.length >= 20)
  assert.deepEqual([...new Set(available.map(({ metric }) => metric.group))], ["cpu", "load", "memory", "pressure", "storage"])

  const health = available.find(({ metric }) => metric.id === "health")?.points ?? []
  const expected = fixture.system.health.filter(([timestamp]) => Number(timestamp) >= hourStart && Number(timestamp) < hourStart + 3_600_000_000)
  assert.equal(health.length, expected.length)
  assert.equal(health[0]?.timestamp, Number(expected[0]?.[0]))
  assert.equal(health.at(-1)?.timestamp, Number(expected.at(-1)?.[0]))
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
})
