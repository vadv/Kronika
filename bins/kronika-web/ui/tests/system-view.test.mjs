import assert from "node:assert/strict"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

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
    contents: 'export { currentValue, fallbackMetric, hasMetric } from "../src/system-view.tsx"',
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
      os_vmstat: [{ logicalName: "os_vmstat", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1106001", values: { oom_kill: 0 } }],
    },
  }
  assert.equal(helpers.currentValue(production, { ...spec, derive: "cpu_busy", series: "missing" }, 200, "en"), "20")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "mem_available_percent", series: "missing" }, 200, "en"), "25")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "filesystem_free_min", series: "missing" }, 200, "en"), "20")
  assert.equal(helpers.currentValue(production, { ...spec, field: "oom_kill", section: "os_vmstat", series: "missing" }, 200, "en"), "0")
})
