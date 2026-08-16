import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { gunzipSync } from "node:zlib"

import { importModule, registryPlugin } from "./import-module.mjs"
import { parseDictionary, validateDictionaries } from "../scripts/i18n.mjs"

const helpers = await importModule(
  'export { effectiveCpuCapacity, cgroupSnapshotPlan, chartableEntityColumns, clearCgroupSnapshotRows, currentValue, entityHistoryRequest, fallbackMetric, hasMetric, metricChartUnit, metricChartValue, metricHistoryPoints, metricHistoryRequest, metricPoints, metricRequestKey, resourceBreakdownSeries, systemEntityRows, CGROUP_SNAPSHOT_REQUESTS, SYSTEM_ENTITIES, SYSTEM_METRICS, SYSTEM_REQUESTS } from "../src/system-view.tsx"; export { bundledFixtureHour } from "../src/fixture.ts"',
  { plugins: [registryPlugin([
    { typeId: "1108001", logicalName: "os_diskstats", identity: ["major", "minor"], columns: ["ts", "major", "minor", "device", "io_in_progress"] },
    { typeId: "1112001", logicalName: "os_mountinfo", identity: ["major", "minor"], columns: ["ts", "major", "minor", "mount_point", "source", "fstype", "free_bytes", "total_bytes", "is_k8s_infra"] },
  ])] },
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
  const semantic = { points: [{ segmentId: "a", series: "test", timestamp: 100, value: 0.099 }] }
  assert.equal(helpers.currentValue(semantic, { ...spec, unit: "%" }, 100, "en"), "<0.1%")
  assert.equal(helpers.metricPoints(semantic, spec)[0].value, 0.099)
  const core = { points: [{ segmentId: "a", series: "test", timestamp: 100, value: 0.00399 }] }
  assert.equal(helpers.currentValue(core, { ...spec, unit: " cores" }, 100, "en"), "0.004")
  assert.equal(helpers.currentValue(core, { ...spec, unit: " cores" }, 100, "ru"), "0,004")
  assert.equal(helpers.metricChartValue(0.00399, "ru", " cores"), "0,004")
  assert.equal(helpers.metricPoints(core, spec)[0].value, 0.00399)
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
    memory: [{ logicalName: "os_meminfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1104001", values: { anon_pages: 30, buffers: 5, cached: 20, mem_available: 25, mem_free: 10, mem_total: 100, s_reclaimable: 5, s_unreclaim: 3 } }],
    pressure: [],
    sections: {
      os_cpu: [cpu(100, 10, 10, 80), { ...cpu(100, 0, 0, 0), values: { cpu_id: 0, scope: 0 } }, cpu(200, 20, 20, 160), { ...cpu(200, 0, 0, 0), values: { cpu_id: 0, scope: 0 } }],
      os_meminfo: [{ logicalName: "os_meminfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1104001", values: { anon_pages: 30, buffers: 5, cached: 20, mem_available: 25, mem_free: 10, mem_total: 100, s_reclaimable: 5, s_unreclaim: 3 } }],
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
  assert.equal(helpers.currentValue(production, { ...spec, derive: "cpu_used_cores", series: "missing", unit: " cores" }, 200, "en"), "0.2")
  const unknownScope = {
    ...production,
    sections: {
      os_cpu: [
        { ...cpu(100, 10, 10, 80), values: { ...cpu(100, 10, 10, 80).values, scope: null } },
        { ...cpu(200, 20, 20, 160), values: { ...cpu(200, 20, 20, 160).values, scope: null } },
      ],
    },
  }
  assert.deepEqual(helpers.metricPoints(unknownScope, { ...spec, derive: "cpu_used_cores", series: "missing" }), [])
  assert.equal(helpers.currentValue(production, { ...spec, field: "mem_available", section: "os_meminfo", series: "missing", unit: " KiB" }, 200, "en"), "25 KiB")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "mem_file_cache", series: "missing", unit: " KiB" }, 200, "en"), "25 KiB")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "mem_other", series: "missing", unit: " KiB" }, 200, "en"), "27 KiB")
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

test("host CPU categories form a truthful total and keep host capacity separate", () => {
  const aggregate = (timestamp, values) => ({
    logicalName: "os_cpu", ordinal: String(timestamp), segmentId: "s", timestamp, typeId: "1102001",
    values: { cpu_id: -1, scope: 0, ...values },
  })
  const core = (timestamp, id) => ({ logicalName: "os_cpu", ordinal: `${timestamp}-${id}`, segmentId: "s", timestamp, typeId: "1102001", values: { cpu_id: id, scope: 0 } })
  const baseline = { user: 100, nice: 10, system: 50, idle: 500, iowait: 20, irq: 4, softirq: 6, steal: 10 }
  const next = { user: 120, nice: 15, system: 60, idle: 550, iowait: 25, irq: 6, softirq: 9, steal: 15 }
  const rows = [aggregate(1_000_000, baseline), core(1_000_000, 0), core(1_000_000, 1), aggregate(2_000_000, next), core(2_000_000, 0), core(2_000_000, 1)]
  const point = (derive) => helpers.metricHistoryPoints({ ...spec, derive, series: "missing" }, rows).at(-1).value
  assert.deepEqual([point("cpu_user"), point("cpu_system"), point("cpu_irq"), point("cpu_iowait"), point("cpu_steal"), point("cpu_idle")], [25, 10, 5, 5, 5, 50])
  assert.equal(point("cpu_used_cores"), 0.9)
  assert.deepEqual(helpers.metricHistoryPoints({ ...spec, derive: "cpu_capacity", series: "missing" }, rows).map(({ value }) => value), [2, 2])

  const current = { ...data, rateColumns: { os_cpu: ["user"] }, sections: { os_cpu: [aggregate(3_000_000, { user: 20, nice: 5, system: 10, idle: 50, iowait: 5, irq: 2, softirq: 3, steal: 5 }), core(3_000_000, 0), core(3_000_000, 1)] } }
  assert.equal(helpers.metricPoints(current, { ...spec, derive: "cpu_user", series: "missing" })[0].value, 25)
  const reset = [...rows, aggregate(3_000_000, baseline), core(3_000_000, 0), core(3_000_000, 1)]
  assert.equal(helpers.metricHistoryPoints({ ...spec, derive: "cpu_user", series: "missing" }, reset).at(-1).value, null)
})

test("host memory categories do not overlap and available remains a separate estimate", () => {
  const row = { logicalName: "os_meminfo", ordinal: "1", segmentId: "s", timestamp: 1, typeId: "1104001", values: {
    mem_total: 1000, mem_free: 100, mem_available: 450, anon_pages: 300, cached: 200, buffers: 50, s_reclaimable: 50, s_unreclaim: 25,
  } }
  const source = { ...data, sections: { os_meminfo: [row] }, memory: [row] }
  const reading = (derive) => helpers.metricPoints(source, { ...spec, derive, series: "missing" })[0].value
  assert.equal(reading("mem_file_cache"), 250)
  assert.equal(reading("mem_other"), 275)
  assert.equal(300 + 250 + 50 + 25 + 100 + reading("mem_other"), 1000)
  assert.equal(row.values.mem_available, 450)
  const invalid = { ...row, values: { ...row.values, mem_total: 10 } }
  assert.equal(helpers.metricPoints({ ...source, memory: [invalid], sections: { os_meminfo: [invalid] } }, { ...spec, derive: "mem_other", series: "missing" })[0].value, null)
})

test("CPU and memory histories expose their complete operator breakdowns", () => {
  const cpu = (timestamp, id, values = {}) => ({
    logicalName: "os_cpu", ordinal: `${timestamp}-${id}`, segmentId: "s", timestamp, typeId: "1102001",
    values: { cpu_id: id, scope: 0, ...values },
  })
  const cpuRows = [
    cpu(1_000_000, -1, { user: 10, nice: 0, system: 5, idle: 80, iowait: 2, irq: 1, softirq: 1, steal: 1 }),
    cpu(1_000_000, 0), cpu(1_000_000, 1),
    cpu(2_000_000, -1, { user: 30, nice: 5, system: 15, idle: 130, iowait: 7, irq: 3, softirq: 4, steal: 6 }),
    cpu(2_000_000, 0), cpu(2_000_000, 1),
  ]
  const t = (key) => key
  const cpuSeries = helpers.resourceBreakdownSeries("cpu_user", cpuRows, false, "en", t)
  assert.deepEqual(cpuSeries.map(({ id, unit }) => [id, unit]), [
    ["cpu_used_cores", "cores"], ["cpu_capacity", "cores"], ["cpu_user", "%"], ["cpu_system", "%"],
    ["cpu_irq", "%"], ["cpu_iowait", "%"], ["cpu_steal", "%"], ["cpu_idle", "%"],
  ])
  assert.equal(cpuSeries.find(({ id }) => id === "cpu_capacity").points.at(-1).value, 2)

  const memory = [{ logicalName: "os_meminfo", ordinal: "1", segmentId: "s", timestamp: 1, typeId: "1104001", values: {
    mem_total: 1000, mem_available: 450, mem_free: 100, anon_pages: 300, cached: 200, buffers: 50, s_reclaimable: 50, s_unreclaim: 25,
  } }]
  const memorySeries = helpers.resourceBreakdownSeries("mem_anon", memory, false, "en", t)
  assert.deepEqual(memorySeries.map(({ id }) => id), ["mem_total", "mem_available", "mem_anon", "mem_file_cache", "mem_s_reclaimable", "mem_s_unreclaim", "mem_free", "mem_other"])
  assert.equal(memorySeries.find(({ id }) => id === "mem_other").points[0].value, 275)
})

test("device latency uses exact reset-safe counter operands and a stable major:minor identity", () => {
  const disk = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_diskstats")
  const readLatency = disk.columns.find(({ field }) => field === "read_latency_ms")
  const diskRequest = helpers.SYSTEM_REQUESTS.find(({ section }) => section === "os_diskstats")
  assert.equal(diskRequest.fields.includes("io_weighted_time_ms"), true)
  assert.equal(diskRequest.fields.includes("weighted_time_ms"), false)
  const row = (timestamp, reads, readTime) => ({ logicalName: "os_diskstats", ordinal: String(timestamp), segmentId: "s", timestamp, typeId: "1108001", values: { major: 8, minor: 1, reads, read_time_ms: readTime } })
  const points = readLatency.points([
    row(1_000_000, "9007199254740993", "9007199254741000"),
    row(2_000_000, "9007199254740995", "9007199254741010"),
    row(3_000_000, "9007199254740995", "9007199254741020"),
    row(4_000_000, "1", "2"),
  ])
  assert.deepEqual(points.map(({ value }) => value), [null, 5, null, null])
  assert.deepEqual(helpers.entityHistoryRequest(row(2_000_000, "3", "9"), readLatency), {
    fields: ["reads", "read_time_ms", "major", "minor"],
    key: '["1108001",[["major","8"],["minor","1"]],"read_latency_ms"]',
    section: "os_diskstats", typeId: "1108001", where: { major: "8", minor: "1" },
  })
  assert.equal(
    helpers.entityHistoryRequest({ ...row(2_000_000, "3", "9"), segmentId: "next" }, readLatency).key,
    helpers.entityHistoryRequest(row(2_000_000, "3", "9"), readLatency).key,
  )
  const current = helpers.systemEntityRows({ ...data, sections: { os_diskstats: [{ ...row(5_000_000, 2, 10), values: { ...row(5_000_000, 2, 10).values, device: "sda", read_sectors: 3, write_sectors: 4, writes: 0, write_time_ms: 1, io_time_ms: 200, io_weighted_time_ms: 500, io_in_progress: 2 } }] } }, "os_diskstats", 5_000_000)[0]
  assert.equal(current.values.device_id, "8:1")
  assert.equal(current.values.read_latency_ms, 5)
  assert.equal(current.values.write_latency_ms, null)
  assert.equal(current.values.read_bytes, 1536)
  assert.equal(current.values.device_busy, 20)
  assert.equal(current.values.average_queue, 0.5)
})

test("collector cgroup rows keep leaf settings factual and use effective hierarchy capacities", () => {
  const row = (logicalName, path, values, ordinal = path) => ({ logicalName, ordinal, segmentId: "s", timestamp: 10, typeId: logicalName === "os_cgroup_cpu" ? "1201001" : logicalName === "os_cgroup_memory" ? "1202001" : "1203001", values: { cgroup_path: path, scope: 3, ...values } })
  const context = row("os_cgroup_context", "/ignored", {
    cpu_path: "/mine", memory_path: "/mine", io_path: "/mine", cpuset_cpus: 1,
    effective_cpu_quota_usec: 50_000, effective_cpu_period_usec: 100_000, effective_memory_max: 1500,
  }, "context")
  const source = { ...data, sections: {
    os_cgroup_context: [context],
    os_cgroup_cpu: [row("os_cgroup_cpu", "/other", { usage_usec: 9 }), row("os_cgroup_cpu", "/mine", { usage_usec: 1_500_000, user_usec: 1_000_000, system_usec: 400_000, quota_usec: 200_000, period_usec: 100_000 })],
    os_cgroup_memory: [row("os_cgroup_memory", "/mine", { current: 1000, max: 2000, anon: 400, file: 300, kernel: 200, slab: 50 })],
    os_cgroup_io: [row("os_cgroup_io", "/mine", { major: 8, minor: 0, rbytes: 10, wbytes: 20, rios: 1, wios: 2 })],
  } }
  const cpu = helpers.systemEntityRows(source, "os_cgroup_cpu", 10)
  assert.equal(cpu.length, 1)
  assert.equal(cpu[0].values.cgroup_used_cores, 1.5)
  assert.equal(cpu[0].values.cgroup_other_cores, 0.1)
  assert.equal(cpu[0].values.cgroup_quota, 2)
  assert.equal(cpu[0].values.cgroup_capacity, 0.5)
  const columns = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_cgroup_cpu").columns
  for (const field of ["cgroup_used_cores", "cgroup_user_cores", "cgroup_system_cores", "cgroup_other_cores", "cgroup_capacity", "cgroup_quota"]) {
    assert.equal(columns.find((column) => column.field === field).kind, "cores", field)
  }
  const memory = helpers.systemEntityRows(source, "os_cgroup_memory", 10)[0]
  assert.equal(memory.values.effective_memory_max, 1500)
  assert.equal(memory.values.max, 2000)
  assert.equal(memory.values.kernel_other, 150)
  assert.equal(memory.values.memory_unclassified, 100)
  assert.equal(helpers.systemEntityRows(source, "os_cgroup_io", 10)[0].values.device_id, "8:0")
  assert.equal(helpers.effectiveCpuCapacity(null, null, 2), null)
  assert.equal(helpers.effectiveCpuCapacity(-1, null, 2), 2)
  assert.equal(helpers.effectiveCpuCapacity(-1, 100_000, null), null)
  assert.equal(helpers.effectiveCpuCapacity(50_000, 100_000, null), 0.5)
  assert.equal(helpers.effectiveCpuCapacity(300_000, 100_000, 2), 2)
  assert.equal(helpers.effectiveCpuCapacity(50_000, 0, 2), null)
  const malformedContext = { ...context, values: { ...context.values, effective_memory_max: -1 } }
  const malformedMemory = helpers.systemEntityRows({ ...source, sections: { ...source.sections, os_cgroup_context: [malformedContext] } }, "os_cgroup_memory", 10)[0]
  assert.equal(malformedMemory.values.effective_memory_max, null)
})

test("System loads cgroup entities only through exact controller-specific plans", () => {
  for (const section of ["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"]) {
    assert.equal(helpers.SYSTEM_REQUESTS.some((request) => request.section === section), false)
    const request = helpers.CGROUP_SNAPSHOT_REQUESTS.find((candidate) => candidate.section === section)
    assert.ok(request.fields.includes("cgroup_path"), section)
    assert.ok(request.fields.includes("scope"), section)
  }
  assert.equal(helpers.SYSTEM_REQUESTS.some(({ section }) => section === "os_cgroup_context"), true)

  const context = {
    logicalName: "os_cgroup_context", ordinal: "context", segmentId: "segment-a", timestamp: 10, typeId: "1205001",
    values: { cpu_path: "/cpu/collector", memory_path: "/memory/collector", io_path: "/io/collector", scope: 3 },
  }
  const source = { sections: { os_cgroup_context: [context] } }
  const plan = helpers.cgroupSnapshotPlan("segment-a", 12, source)
  assert.equal(plan.key, '["segment-a",12,"/cpu/collector","/memory/collector","/io/collector","3"]')
  assert.deepEqual(Object.fromEntries(plan.loads.map(({ filters, request }) => [request.section, filters])), {
    os_cgroup_cpu: { cgroup_path: "/cpu/collector", scope: "3" },
    os_cgroup_memory: { cgroup_path: "/memory/collector", scope: "3" },
    os_cgroup_io: { cgroup_path: "/io/collector", scope: "3" },
  })

  const wrongSegment = helpers.cgroupSnapshotPlan("segment-b", 12, source)
  assert.deepEqual(wrongSegment.loads, [])
  const unusableScope = helpers.cgroupSnapshotPlan("segment-a", 12, {
    sections: { os_cgroup_context: [{ ...context, values: { ...context.values, scope: null } }] },
  })
  assert.deepEqual(unusableScope.loads, [])
})

test("changing a cgroup snapshot key removes every prior exact entity row", () => {
  const preserved = [{ logicalName: "os_cpu", ordinal: "cpu", segmentId: "s", timestamp: 1, typeId: "1102001", values: {} }]
  const cleared = helpers.clearCgroupSnapshotRows({
    sections: {
      os_cpu: preserved,
      os_cgroup_context: [{ logicalName: "os_cgroup_context", ordinal: "context", segmentId: "s", timestamp: 1, typeId: "1205001", values: {} }],
      os_cgroup_cpu: [{ logicalName: "os_cgroup_cpu", ordinal: "cpu", segmentId: "s", timestamp: 1, typeId: "1201001", values: {} }],
      os_cgroup_memory: [{ logicalName: "os_cgroup_memory", ordinal: "memory", segmentId: "s", timestamp: 1, typeId: "1202001", values: {} }],
      os_cgroup_io: [{ logicalName: "os_cgroup_io", ordinal: "io", segmentId: "s", timestamp: 1, typeId: "1203001", values: {} }],
    },
  })
  assert.equal(cleared.sections.os_cpu, preserved)
  assert.equal(cleared.sections.os_cgroup_context.length, 1)
  assert.equal(cleared.sections.os_cgroup_cpu, undefined)
  assert.equal(cleared.sections.os_cgroup_memory, undefined)
  assert.equal(cleared.sections.os_cgroup_io, undefined)
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

test("System entity tables keep exact meaning-first orders and rate presentation", () => {
  const fields = Object.fromEntries(helpers.SYSTEM_ENTITIES.map(({ section, columns }) => [section, columns.map(({ field }) => field)]))
  assert.deepEqual(fields.os_diskstats, ["device", "device_id", "reads", "writes", "read_bytes", "write_bytes", "read_latency_ms", "write_latency_ms", "device_busy", "average_queue", "io_in_progress"])
  assert.deepEqual(fields.os_cgroup_cpu, ["cgroup_path", "cgroup_used_cores", "cgroup_user_cores", "cgroup_system_cores", "cgroup_other_cores", "cgroup_capacity", "cgroup_quota", "cpuset_cpus"])
  assert.deepEqual(fields.os_cgroup_memory, ["cgroup_path", "current", "effective_memory_max", "max", "anon", "file", "slab", "kernel_other", "memory_unclassified"])
  assert.deepEqual(fields.os_cgroup_io, ["cgroup_path", "device_id", "rbytes", "wbytes", "rios", "wios"])
  assert.deepEqual(fields.os_mountinfo, ["mount_point", "source", "fstype", "free_bytes", "total_bytes", "is_k8s_infra"])
  assert.deepEqual(fields.os_netdev, ["iface", "rx_bytes", "tx_bytes", "rx_packets", "tx_packets", "rx_errs", "tx_errs", "rx_drop", "tx_drop", "speed_mbit", "duplex"])
  assert.deepEqual(fields.os_topology, ["cpu_id", "socket_id", "core_id", "numa_node", "model_name", "mhz_max"])

  const disk = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_diskstats")
  for (const field of ["reads", "writes", "read_bytes", "write_bytes"]) assert.equal(disk.columns.find((column) => column.field === field).rate, true)
  for (const field of ["read_latency_ms", "write_latency_ms", "device_busy", "average_queue"]) assert.notEqual(disk.columns.find((column) => column.field === field).historyFields, undefined)
})

test("hidden mount device IDs remain exact request and history identity", () => {
  const mount = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_mountinfo")
  const request = helpers.SYSTEM_REQUESTS.find(({ section }) => section === "os_mountinfo")
  assert.ok(request.fields.includes("major"))
  assert.ok(request.fields.includes("minor"))
  assert.equal(mount.columns.some(({ field }) => field === "major" || field === "minor"), false)

  const row = { logicalName: "os_mountinfo", ordinal: "0", segmentId: "s", timestamp: 12, typeId: "1112001", values: { major: 8, minor: 1, free_bytes: 4 } }
  assert.deepEqual(helpers.entityHistoryRequest(row, mount.columns.find(({ field }) => field === "free_bytes")), {
    fields: ["free_bytes", "major", "minor"],
    key: '["1112001",[["major","8"],["minor","1"]],"free_bytes"]',
    section: "os_mountinfo",
    typeId: "1112001",
    where: { major: "8", minor: "1" },
  })
})

test("System entity headers have exact EN/RU help without obvious or orphan entries", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  const obvious = new Set(["device", "device_id", "cgroup_path", "mount_point", "fstype", "source", "iface", "cpu_id", "model_name"])
  const usedHelp = new Set()
  for (const { columns, section } of helpers.SYSTEM_ENTITIES) for (const column of columns) {
    assert.equal(column.help === undefined, obvious.has(column.field), `${section}/${column.field}`)
    if (column.help === undefined) continue
    usedHelp.add(column.help)
    assert.equal(Object.hasOwn(english, column.help), true, column.help)
    assert.equal(Object.hasOwn(russian, column.help), true, column.help)
  }
  const dictionaryHelp = Object.keys(english).filter((key) => /^system\.field\.[^.]+\.help$/.test(key)).sort()
  assert.deepEqual([...usedHelp].sort(), dictionaryHelp)
})

test("System history requests are selected-metric keys with exact physical inputs", () => {
  const direct = helpers.metricHistoryRequest({ ...spec, field: "oom_kill", section: "os_vmstat", series: undefined })
  assert.deepEqual(direct, { fields: ["oom_kill"], section: "os_vmstat", where: {} })
  const pressure = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_pressure")
  const pressureRequest = helpers.metricHistoryRequest(pressure)
  assert.deepEqual(pressureRequest.where, { resource: "0" })
  assert.ok(pressureRequest.fields.includes("some_avg10"))
  assert.ok(pressureRequest.fields.includes("resource"))
  const cpu = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_user")
  const cpuRequest = helpers.metricHistoryRequest(cpu)
  for (const field of ["cpu_id", "scope", "user", "idle", "iowait"]) assert.ok(cpuRequest.fields.includes(field))
  const memory = helpers.SYSTEM_METRICS.find(({ id }) => id === "mem_anon")
  const memoryRequest = helpers.metricHistoryRequest(memory)
  for (const field of ["mem_total", "mem_available", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim"]) assert.ok(memoryRequest.fields.includes(field))
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

test("a cumulative metric stays absent until its section announces rate columns", async () => {
  const spec = { field: "reads", group: "storage", help: "x", id: "reads", label: "x", section: "os_diskstats", unit: "" }
  const row = { logicalName: "os_diskstats", ordinal: "0", segmentId: "a", timestamp: 1, typeId: "1108001", values: { major: 8, minor: 0, reads: 10 } }
  const registry = [
    { typeId: "1108001", logicalName: "os_diskstats", identity: ["major", "minor"], columns: ["ts", "major", "minor", "reads"], columnMetadata: [
      { name: "ts", type: "timestamp_us", class: "timestamp", unit: null },
      { name: "major", type: "i32", class: "label", unit: null },
      { name: "minor", type: "i32", class: "label", unit: null },
      { name: "reads", type: "u64", class: "cumulative", unit: "count" },
    ] },
  ]
  const cumulative = await importModule('export { hasMetric } from "../src/system-view.tsx"', { plugins: [registryPlugin(registry)] })
  assert.equal(cumulative.hasMetric({ points: [], sections: { os_diskstats: [row] }, rateColumns: {} }, spec), false)
  assert.equal(cumulative.hasMetric({ points: [], sections: { os_diskstats: [row] }, rateColumns: { os_diskstats: ["reads"] } }, spec), true)
})

test("the storage rollups peak across devices and honor pre-computed rates", () => {
  const row = (timestamp, major, ioTime) => ({ logicalName: "os_diskstats", ordinal: `${major}:${timestamp}`, segmentId: "a", timestamp, typeId: "1108001", values: { major, minor: 0, io_time_ms: ioTime } })
  const busy = helpers.SYSTEM_METRICS.find(({ id }) => id === "device_busy")
  const counter = helpers.metricPoints({ points: [], sections: { os_diskstats: [
    row(1_000_000, 7, 100), row(1_000_000, 8, 300),
    row(2_000_000, 7, 200), row(2_000_000, 8, 700),
  ] }, rateColumns: { os_diskstats: ["io_time_ms"] } }, busy).map(({ value }) => value)
  assert.deepEqual(counter, [30, 70])
  const storedRates = helpers.metricPoints({ points: [], sections: { os_diskstats: [row(1_000_000, 7, 0.2), row(1_000_000, 8, 0.7)] }, rateColumns: { os_diskstats: ["io_time_ms"] } }, busy)
  assert.ok(Math.abs((storedRates[0]?.value ?? 0) - 0.07) < 1e-9)
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
  assert.ok(available.length >= 7)
  assert.deepEqual([...new Set(available.map(({ metric }) => metric.group))], ["host", "load", "memory", "pressure", "storage"])

  const health = available.find(({ metric }) => metric.id === "health")?.points ?? []
  const expected = fixture.system.health.filter(([timestamp]) => Number(timestamp) >= hourStart && Number(timestamp) < hourStart + 3_600_000_000)
  assert.equal(health.length, expected.length)
  assert.equal(health[0]?.timestamp, Number(expected[0]?.[0]))
  assert.equal(health.at(-1)?.timestamp, Number(expected.at(-1)?.[0]))
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
})

test("System keeps the audited balanced groups and opens charts only inside the dock", async () => {
  const [source, styles] = await Promise.all([
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.match(source, /\["host", "cpu", "memory", "pressure"\]/)
  assert.match(source, /\["load", "storage", "network"\]/)
  // The dock, not a standing console chart: closed by default, opened by a Use
  // row or a metric chip, dismissed like the PostgreSQL detail panel.
  assert.match(source, /useState\(false\)/)
  assert.match(source, /dockShown && selectedMetric !== undefined && <SystemDock/)
  assert.match(source, /data-testid="system-dock"/)
  assert.match(source, /useDetailDismiss\(onClose, `system:\$\{group\}`\)/)
  assert.match(source, /chartsVisible && dockOpen/)
  assert.doesNotMatch(source, /metric-history|system-console/)
  assert.match(styles, /\.system-layout \{[^}]*align-items: start;/)
  assert.match(styles, /\.system-layout \{[^}]*clamp\(460px, 32vw, 600px\)/)
  assert.doesNotMatch(styles, /\.system-layout \{[^}]*min-height:/)
  assert.doesNotMatch(styles, /\.metric-history|\.system-console/)
})
