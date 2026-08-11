import { Activity, Cpu, Database, Gauge, HardDrive, MemoryStick, Network } from "lucide-react"
import { useEffect, useMemo, useState, type ReactNode } from "react"

import { fieldNameForLocator, resolveLocator, type DataRow, type Finding, type HourData, type Point, type SectionRequest } from "./api"
import { EntityTable, type EntityColumn } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import { asNumber, measure, snapshot, stateText, value, type Locale, shownMoment } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

interface MetricSpec {
  readonly id: string
  readonly group: "cpu" | "load" | "memory" | "pressure" | "storage" | "network"
  readonly label: string
  readonly help: string
  readonly section?: string
  readonly field?: string
  readonly derive?:
    | "cpu_busy"
    | "mem_available_percent"
    | "filesystem_free_min"
    | "process_count"
    | "process_running"
    | "process_blocked"
    | "process_threads"
    | "process_context_switches"
    | "process_run_delay"
    | "process_resident"
    | "process_virtual"
    | "process_swap"
    | "process_minor_faults"
    | "process_major_faults"
    | "process_read"
    | "process_write"
    | "process_block_delay"
    | "device_count"
    | "device_active_io"
    | "filesystem_count"
    | "interface_count"
    | "network_rx"
    | "network_tx"
    | "network_errors"
    | "network_drops"
  readonly series?: string
  readonly resource?: number
  readonly unit: string
}

export const SYSTEM_METRICS: readonly MetricSpec[] = [
  metric("health", "cpu", "system.metric.health", "health", "os_health", "%"),
  derivedMetric("cpu_busy", "cpu", "system.metric.cpu_busy", "os_cpu_busy_percent", "cpu_busy", "%"),
  derivedMetric("process_count", "cpu", "system.metric.process_count", "os_process_count", "process_count", ""),
  derivedMetric("process_running", "cpu", "system.metric.process_running", "os_process_running", "process_running", ""),
  derivedMetric("process_blocked", "cpu", "system.metric.process_blocked", "os_process_blocked", "process_blocked", ""),
  derivedMetric("process_threads", "cpu", "system.metric.process_threads", "os_process_threads", "process_threads", ""),
  derivedMetric("process_context_switches", "cpu", "system.metric.process_context_switches", "os_process_context_switches", "process_context_switches", ""),
  derivedMetric("process_run_delay", "cpu", "system.metric.process_run_delay", "os_process_run_delay", "process_run_delay", " ns"),
  metric("procs_running", "cpu", "system.metric.procs_running", "os_stat", "procs_running", ""),
  metric("procs_blocked", "cpu", "system.metric.procs_blocked", "os_stat", "procs_blocked", ""),
  metric("context_switches", "cpu", "system.metric.context_switches", "os_stat", "ctxt", ""),
  metric("load1", "load", "system.metric.load1", "os_loadavg", "load1", ""),
  metric("load5", "load", "system.metric.load5", "os_loadavg", "load5", ""),
  metric("load15", "load", "system.metric.load15", "os_loadavg", "load15", ""),
  metric("runnable", "load", "system.metric.runnable", "os_loadavg", "running", ""),
  metric("tasks", "load", "system.metric.tasks", "os_loadavg", "total", ""),
  derivedMetric("mem_available_percent", "memory", "system.metric.mem_available_percent", "os_mem_available_percent", "mem_available_percent", "%"),
  metric("mem_available", "memory", "system.metric.mem_available", "os_meminfo", "mem_available", " KiB"),
  metric("mem_total", "memory", "system.metric.mem_total", "os_meminfo", "mem_total", " KiB"),
  metric("cached", "memory", "system.metric.cached", "os_meminfo", "cached", " KiB"),
  metric("swap_free", "memory", "system.metric.swap_free", "os_meminfo", "swap_free", " KiB"),
  metric("swap_total", "memory", "system.metric.swap_total", "os_meminfo", "swap_total", " KiB"),
  derivedMetric("process_resident", "memory", "system.metric.process_resident", "os_process_resident", "process_resident", " KiB"),
  derivedMetric("process_virtual", "memory", "system.metric.process_virtual", "os_process_virtual", "process_virtual", " KiB"),
  derivedMetric("process_swap", "memory", "system.metric.process_swap", "os_process_swap", "process_swap", " KiB"),
  derivedMetric("process_minor_faults", "memory", "system.metric.process_minor_faults", "os_process_minor_faults", "process_minor_faults", ""),
  derivedMetric("process_major_faults", "memory", "system.metric.process_major_faults", "os_process_major_faults", "process_major_faults", ""),
  seriesSectionMetric("oom_kill", "memory", "system.metric.oom_kill", "os_oom_kills", "os_vmstat", "oom_kill", ""),
  pressureMetric("cpu_pressure", "system.metric.cpu_pressure", 0),
  pressureMetric("memory_pressure", "system.metric.memory_pressure", 1),
  pressureMetric("io_pressure", "system.metric.io_pressure", 2),
  derivedMetric("filesystem_free_min", "storage", "system.metric.filesystem_free_min", "os_min_filesystem_free_percent", "filesystem_free_min", "%"),
  derivedMetric("process_read", "storage", "system.metric.process_read", "os_process_read", "process_read", " B"),
  derivedMetric("process_write", "storage", "system.metric.process_write", "os_process_write", "process_write", " B"),
  derivedMetric("process_block_delay", "storage", "system.metric.process_block_delay", "os_process_block_delay", "process_block_delay", " ticks"),
  derivedMetric("device_count", "storage", "system.metric.device_count", "os_device_count", "device_count", ""),
  derivedMetric("device_active_io", "storage", "system.metric.device_active_io", "os_device_active_io", "device_active_io", ""),
  derivedMetric("filesystem_count", "storage", "system.metric.filesystem_count", "os_filesystem_count", "filesystem_count", ""),
  derivedMetric("interface_count", "network", "system.metric.interface_count", "os_interface_count", "interface_count", ""),
  derivedMetric("network_rx", "network", "system.metric.network_rx", "os_network_rx", "network_rx", " B"),
  derivedMetric("network_tx", "network", "system.metric.network_tx", "os_network_tx", "network_tx", " B"),
  derivedMetric("network_errors", "network", "system.metric.network_errors", "os_network_errors", "network_errors", ""),
  derivedMetric("network_drops", "network", "system.metric.network_drops", "os_network_drops", "network_drops", ""),
]

const GROUPS: readonly { readonly id: MetricSpec["group"]; readonly icon: ReactNode; readonly label: string }[] = [
  { id: "cpu", icon: <Cpu size={14} />, label: "system.group.cpu" },
  { id: "load", icon: <Gauge size={14} />, label: "system.group.load" },
  { id: "memory", icon: <MemoryStick size={14} />, label: "system.group.memory" },
  { id: "pressure", icon: <Activity size={14} />, label: "system.group.pressure" },
  { id: "storage", icon: <HardDrive size={14} />, label: "system.group.storage" },
  { id: "network", icon: <Network size={14} />, label: "system.group.network" },
]

/** What each derived metric reads. Kept beside the code that derives it, so a
 *  new metric cannot quietly ask the loader for a section it never declared. */
const DERIVE_INPUTS: Readonly<Record<NonNullable<MetricSpec["derive"]>, readonly [string, readonly string[]]>> = {
  cpu_busy: ["os_cpu", ["cpu_id", "scope", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"]],
  mem_available_percent: ["os_meminfo", ["mem_total", "mem_available"]],
  filesystem_free_min: ["os_mountinfo", ["total_bytes", "free_bytes"]],
  process_count: ["os_process", []],
  process_running: ["os_process", ["state"]],
  process_blocked: ["os_process", ["state"]],
  process_threads: ["os_process", ["num_threads"]],
  process_context_switches: ["os_process", ["nvcsw", "nivcsw"]],
  process_run_delay: ["os_process", ["rundelay_ns"]],
  process_resident: ["os_process", ["rmem_kb"]],
  process_virtual: ["os_process", ["vmem_kb"]],
  process_swap: ["os_process", ["vswap_kb"]],
  process_minor_faults: ["os_process", ["minflt"]],
  process_major_faults: ["os_process", ["majflt"]],
  process_read: ["os_process", ["read_bytes"]],
  process_write: ["os_process", ["write_bytes"]],
  process_block_delay: ["os_process", ["blkdelay_ticks"]],
  device_count: ["os_diskstats", []],
  device_active_io: ["os_diskstats", ["io_in_progress"]],
  filesystem_count: ["os_mountinfo", []],
  interface_count: ["os_netdev", []],
  network_rx: ["os_netdev", ["rx_bytes"]],
  network_tx: ["os_netdev", ["tx_bytes"]],
  network_errors: ["os_netdev", ["rx_errs", "tx_errs"]],
  network_drops: ["os_netdev", ["rx_drop", "tx_drop"]],
}


const GROUP_COLUMNS: readonly (readonly MetricSpec["group"][])[] = [
  ["cpu", "memory"],
  ["load", "pressure", "storage", "network"],
]

const ENTITIES: readonly {
  readonly section: string
  readonly label: string
  readonly icon: ReactNode
  readonly columns: readonly EntityColumn[]
}[] = [
  {
    section: "os_diskstats", label: "system.entities.devices", icon: <HardDrive size={14} />,
    columns: [text("device", 150, true), number("io_in_progress"), number("reads"), number("writes"), number("read_sectors"), number("write_sectors"), milliseconds("read_time_ms"), milliseconds("write_time_ms"), number("discards"), number("flushes")],
  },
  {
    section: "os_mountinfo", label: "system.entities.mounts", icon: <Database size={14} />,
    columns: [text("mount_point", 240, true), text("fstype", 120), text("source", 180), id("major"), id("minor"), bytes("total_bytes"), bytes("free_bytes"), boolean("is_k8s_infra")],
  },
  {
    section: "os_netdev", label: "system.entities.network", icon: <Network size={14} />,
    columns: [text("iface", 150, true), bytes("rx_bytes"), number("rx_packets"), number("rx_errs"), number("rx_drop"), bytes("tx_bytes"), number("tx_packets"), number("tx_errs"), number("tx_drop"), number("speed_mbit"), id("duplex")],
  },
  {
    section: "os_topology", label: "system.entities.topology", icon: <Cpu size={14} />,
    columns: [id("cpu_id", 90, true), text("model_name", 300), number("mhz_max"), id("core_id"), id("socket_id"), id("numa_node")],
  },
]

/** The sections and fields this view reads, and nothing more. A process row
 *  carries a command line; the charts here only sum numbers off it.
 *
 *  Split in two: an hour of per-process rows is megabytes per segment, and the
 *  cards that sum them are a minority of the screen. The rest draws first and
 *  they fill in. */
function systemRequests(): readonly SectionRequest[] {
  const wanted = new Map<string, Set<string>>()
  const need = (section: string, fields: readonly string[]) => {
    const stored = wanted.get(section) ?? new Set<string>()
    for (const field of fields) stored.add(field)
    wanted.set(section, stored)
  }
  for (const spec of SYSTEM_METRICS) {
    if (spec.derive !== undefined) {
      const [section, fields] = DERIVE_INPUTS[spec.derive]
      need(section, fields)
    } else if (spec.section !== undefined && spec.field !== undefined) {
      need(spec.section, spec.resource === undefined ? [spec.field] : [spec.field, "resource"])
    }
  }
  for (const panel of ENTITIES) need(panel.section, panel.columns.map((column: EntityColumn) => column.field))
  return [...wanted].map(([section, fields]) => ({ section, fields: [...fields] }))
}

const ALL_SYSTEM_REQUESTS = systemRequests()
export const SYSTEM_REQUESTS = ALL_SYSTEM_REQUESTS.filter((request) => request.section !== "os_process")
export const SYSTEM_DEFERRED_REQUESTS = ALL_SYSTEM_REQUESTS.filter((request) => request.section === "os_process")

export function SystemView({
  cursor,
  data,
  focus,
  hour,
  locale,
  onCursor,
  onFinding,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly focus: Finding | null
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly t: Translate
}) {
  const available = useMemo(() => SYSTEM_METRICS.map((spec) => ({ points: metricPoints(data, spec), spec }))
    .filter(({ points }) => points.some((point) => point.value !== null && Number.isFinite(point.value))), [data])
  const [selected, setSelected] = useState(available[0]?.spec.id ?? "")
  useEffect(() => {
    if (available.some(({ spec }) => spec.id === selected)) return
    setSelected(available[0]?.spec.id ?? "")
  }, [available, selected])
  useEffect(() => {
    if (focus === null) return
    const field = fieldNameForLocator(focus)
    const focusedRow = resolveLocator(data, focus)?.row ?? null
    const resource = asNumber(value(focusedRow, "resource"))
    const fallback = fallbackMetric(focus.logicalName)
    const match = available.find(({ spec }) => spec.section === focus.logicalName && spec.field === field
      && (spec.resource === undefined || spec.resource === resource))
      ?? (fallback === null ? undefined : available.find(({ spec }) => spec.id === fallback))
    if (match !== undefined) setSelected(match.spec.id)
  }, [available, data, focus])
  const selectedMetric = available.find(({ spec }) => spec.id === selected) ?? available[0]
  const selectedPoints = selectedMetric?.points ?? []
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} onCursor={onCursor} onFinding={onFinding} shownAt={shownAt} t={t} />
    <section className="system-console">
      <div className="metric-groups">
        {GROUP_COLUMNS.map((column, index) => <div className="metric-column" key={index}>
          {column.map((id) => {
            const group = GROUPS.find((candidate) => candidate.id === id)
            const metrics = available.filter(({ spec }) => spec.group === id)
            if (group === undefined || metrics.length === 0) return null
            return <section className="metric-group" data-testid={`system-group-${group.id}`} key={group.id}>
              <h2>{group.icon}<span>{t(group.label)}</span></h2>
              <div className="metric-grid">
                {metrics.map(({ points, spec }) => {
                  const output = currentPointValue(points, cursor, locale, spec.unit)
                  return <button aria-pressed={selectedMetric?.spec.id === spec.id} data-testid={`system-metric-${spec.id}`} key={spec.id} onClick={() => setSelected(spec.id)} type="button">
                    <LabelHelp helpKey={spec.help} labelKey={spec.label} t={t} />
                    <strong title={output}>{output}</strong>
                  </button>
                })}
              </div>
            </section>
          })}
        </div>)}
      </div>
      <section className="metric-history">
        <header><span>{t("system.history")}</span><strong>{selectedMetric === undefined ? "—" : t(selectedMetric.spec.label)}</strong></header>
        {selectedMetric === undefined
          ? <p className="table-empty">{t("system.no_metrics")}</p>
          : <SeriesChart hour={hour} label={t(selectedMetric.spec.label)} locale={locale} points={selectedPoints} />}
      </section>
    </section>
    <section className="entity-panels">
      {ENTITIES.map((entity) => {
        const rows = snapshot(sectionRows(data, entity.section), cursor)
        if (rows.length === 0) return null
        const resolved = focus?.logicalName === entity.section ? resolveLocator(data, focus)?.row ?? null : null
        return <section className="entity-panel" key={entity.section}>
          <h2>{entity.icon}<span>{t(entity.label)}</span></h2>
          <EntityTable columns={entity.columns} empty={t("table.no_rows")} label={t(entity.label)} locale={locale} rows={rows} selectedKey={resolved === null ? null : rowKey(resolved)} t={t} testId={`system-${entity.section}`} />
        </section>
      })}
    </section>
  </>
}

export function metricPoints(data: HourData, spec: MetricSpec): readonly ChartPoint[] {
  if (spec.series !== undefined) {
    const stored = data.points.filter((point) => point.series === spec.series).map(point)
    if (stored.length !== 0) return stored
  }
  if (spec.derive !== undefined) return derivedPoints(data, spec.derive)
  if (spec.section === undefined || spec.field === undefined) return []
  const field = spec.field
  return sectionRows(data, spec.section).flatMap((row) => {
    if (spec.resource !== undefined && asNumber(value(row, "resource")) !== spec.resource) return []
    const number = asNumber(value(row, field))
    return [{ segmentId: row.segmentId, timestamp: row.timestamp, value: number }]
  })
}

export function hasMetric(data: HourData, spec: MetricSpec): boolean {
  return metricPoints(data, spec).some((point) => point.value !== null && Number.isFinite(point.value))
}

function derivedPoints(data: HourData, derive: NonNullable<MetricSpec["derive"]>): readonly ChartPoint[] {
  if (derive === "cpu_busy") return cpuBusyPoints(sectionRows(data, "os_cpu"))
  if (derive === "mem_available_percent") return sectionRows(data, "os_meminfo").map((row) => {
    const available = asNumber(value(row, "mem_available"))
    const total = asNumber(value(row, "mem_total"))
    return { segmentId: row.segmentId, timestamp: row.timestamp, value: available === null || total === null || total <= 0 ? null : available / total * 100 }
  })
  if (derive === "filesystem_free_min") return aggregateRows(sectionRows(data, "os_mountinfo"), (rows) => {
    const percentages = rows.map((row) => {
      const total = asNumber(value(row, "total_bytes"))
      const free = asNumber(value(row, "free_bytes"))
      return total === null || free === null || total <= 0 ? null : free / total * 100
    })
    return percentages.some((number) => number === null) ? null : Math.min(...percentages as number[])
  })
  if (derive === "process_count") return aggregateRows(data.processes, (rows) => rows.length)
  if (derive === "process_running") return aggregateRows(data.processes, (rows) => countState(rows, "R"))
  if (derive === "process_blocked") return aggregateRows(data.processes, (rows) => countState(rows, "D"))
  if (derive === "process_threads") return aggregateRows(data.processes, (rows) => sumFields(rows, ["num_threads"]))
  if (derive === "process_context_switches") return aggregateRows(data.processes, (rows) => sumFields(rows, ["nvcsw", "nivcsw"]))
  if (derive === "process_run_delay") return aggregateRows(data.processes, (rows) => sumFields(rows, ["rundelay_ns"]))
  if (derive === "process_resident") return aggregateRows(data.processes, (rows) => sumFields(rows, ["rmem_kb"]))
  if (derive === "process_virtual") return aggregateRows(data.processes, (rows) => sumFields(rows, ["vmem_kb"]))
  if (derive === "process_swap") return aggregateRows(data.processes, (rows) => sumFields(rows, ["vswap_kb"]))
  if (derive === "process_minor_faults") return aggregateRows(data.processes, (rows) => sumFields(rows, ["minflt"]))
  if (derive === "process_major_faults") return aggregateRows(data.processes, (rows) => sumFields(rows, ["majflt"]))
  if (derive === "process_read") return aggregateRows(data.processes, (rows) => sumFields(rows, ["read_bytes"]))
  if (derive === "process_write") return aggregateRows(data.processes, (rows) => sumFields(rows, ["write_bytes"]))
  if (derive === "process_block_delay") return aggregateRows(data.processes, (rows) => sumFields(rows, ["blkdelay_ticks"]))
  if (derive === "device_count") return aggregateRows(sectionRows(data, "os_diskstats"), (rows) => rows.length)
  if (derive === "device_active_io") return aggregateRows(sectionRows(data, "os_diskstats"), (rows) => sumFields(rows, ["io_in_progress"]))
  if (derive === "filesystem_count") return aggregateRows(sectionRows(data, "os_mountinfo"), (rows) => rows.length)
  if (derive === "interface_count") return aggregateRows(sectionRows(data, "os_netdev"), (rows) => rows.length)
  if (derive === "network_rx") return aggregateRows(sectionRows(data, "os_netdev"), (rows) => sumFields(rows, ["rx_bytes"]))
  if (derive === "network_tx") return aggregateRows(sectionRows(data, "os_netdev"), (rows) => sumFields(rows, ["tx_bytes"]))
  if (derive === "network_errors") return aggregateRows(sectionRows(data, "os_netdev"), (rows) => sumFields(rows, ["rx_errs", "tx_errs"]))
  return aggregateRows(sectionRows(data, "os_netdev"), (rows) => sumFields(rows, ["rx_drop", "tx_drop"]))
}

function aggregateRows(rows: readonly DataRow[], aggregate: (rows: readonly DataRow[]) => number | null): readonly ChartPoint[] {
  const groups = new Map<string, { readonly rows: DataRow[]; readonly segmentId: string; readonly timestamp: number }>()
  for (const row of rows) {
    const key = `${row.segmentId}:${row.timestamp}`
    const stored = groups.get(key) ?? { rows: [], segmentId: row.segmentId, timestamp: row.timestamp }
    stored.rows.push(row)
    groups.set(key, stored)
  }
  return [...groups.values()]
    .sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId))
    .map((stored) => ({ segmentId: stored.segmentId, timestamp: stored.timestamp, value: aggregate(stored.rows) }))
}

function sumFields(rows: readonly DataRow[], fields: readonly string[]): number | null {
  let total = 0
  for (const row of rows) {
    for (const field of fields) {
      const number = asNumber(value(row, field))
      if (number === null) return null
      total += number
    }
  }
  return total
}

function countState(rows: readonly DataRow[], selected: string): number | null {
  let count = 0
  for (const row of rows) {
    const state = stateText(value(row, "state"))
    if (state === "—") return null
    if (state === selected) count += 1
  }
  return count
}

function cpuBusyPoints(rows: readonly DataRow[]): readonly ChartPoint[] {
  const fields = ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"] as const
  const bySegment = new Map<string, DataRow[]>()
  for (const row of rows) {
    if (asNumber(value(row, "cpu_id")) !== -1 || asNumber(value(row, "scope")) !== 0) continue
    const stored = bySegment.get(row.segmentId) ?? []
    stored.push(row)
    bySegment.set(row.segmentId, stored)
  }
  const points: ChartPoint[] = []
  for (const [segmentId, stored] of bySegment) {
    let previous: readonly number[] | null = null
    for (const row of stored.slice().sort((left, right) => left.timestamp - right.timestamp)) {
      const counters = fields.map((field) => asNumber(value(row, field)))
      let output: number | null = null
      if (previous !== null && counters.every((counter): counter is number => counter !== null)) {
        const deltas = counters.map((counter, index) => counter - (previous?.[index] ?? counter))
        const total = deltas.reduce((sum, delta) => sum + delta, 0)
        const idle = (deltas[3] ?? 0) + (deltas[4] ?? 0)
        if (total > 0 && deltas.every((delta) => delta >= 0)) output = (total - idle) / total * 100
      }
      points.push({ segmentId, timestamp: row.timestamp, value: output })
      previous = counters.every((counter): counter is number => counter !== null) ? counters : null
    }
  }
  return points
}

export function currentValue(data: HourData, spec: MetricSpec, cursor: number, locale: Locale): string {
  return currentPointValue(metricPoints(data, spec), cursor, locale, spec.unit)
}

function currentPointValue(points: readonly ChartPoint[], cursor: number, locale: Locale, unit: string): string {
  let nearest: ChartPoint | null = null
  for (const candidate of points) {
    if (nearest === null || Math.abs(candidate.timestamp - cursor) < Math.abs(nearest.timestamp - cursor)
      || (Math.abs(candidate.timestamp - cursor) === Math.abs(nearest.timestamp - cursor) && candidate.timestamp < nearest.timestamp)) nearest = candidate
  }
  return nearest === null ? "—" : measure(nearest.value, locale, unit)
}

export function fallbackMetric(logicalName: string): string | null {
  if (logicalName === "os_cpu" || logicalName === "os_stat") return "cpu_busy"
  if (logicalName === "os_loadavg") return "load1"
  if (logicalName === "os_meminfo") return "mem_available_percent"
  if (logicalName === "os_vmstat") return "oom_kill"
  return logicalName === "health" ? "health" : null
}

function rowKey(row: DataRow): string { return `${row.segmentId}:${row.typeId}:${row.ordinal}` }

function sectionRows(data: HourData, section: string): readonly DataRow[] {
  if (section === "health") return data.health
  if (section === "os_loadavg") return data.load
  if (section === "os_meminfo") return data.memory
  if (section === "os_psi") return data.pressure
  return data.sections[section] ?? []
}

function metric(id: string, group: MetricSpec["group"], label: string, section: string, field: string, unit: string): MetricSpec {
  return { id, group, label: `${label}.label`, help: `${label}.help`, section, field, unit }
}

function seriesSectionMetric(id: string, group: MetricSpec["group"], label: string, series: string, section: string, field: string, unit: string): MetricSpec {
  return { id, group, label: `${label}.label`, help: `${label}.help`, series, section, field, unit }
}

function derivedMetric(id: string, group: MetricSpec["group"], label: string, series: string, derive: NonNullable<MetricSpec["derive"]>, unit: string): MetricSpec {
  return { id, group, label: `${label}.label`, help: `${label}.help`, series, derive, unit }
}

function pressureMetric(id: string, label: string, resource: number): MetricSpec {
  return { id, group: "pressure", label: `${label}.label`, help: `${label}.help`, section: "os_psi", field: "some_avg10", resource, unit: "%" }
}

function point(source: Point): ChartPoint { return source }
function systemColumn(field: string, kind: NonNullable<EntityColumn["kind"]>, width: number, sticky = false): EntityColumn {
  return { field, label: `system.field.${field}.label`, help: `system.field.${field}.help`, kind, width, sticky }
}
function text(field: string, width = 130, sticky = false): EntityColumn { return systemColumn(field, "text", width, sticky) }
function number(field: string, width = 126): EntityColumn { return systemColumn(field, "number", width) }
function id(field: string, width = 110, sticky = false): EntityColumn { return systemColumn(field, "id", width, sticky) }
function bytes(field: string, width = 145): EntityColumn { return systemColumn(field, "bytes", width) }
function milliseconds(field: string, width = 145): EntityColumn { return systemColumn(field, "milliseconds", width) }
function boolean(field: string, width = 130): EntityColumn { return systemColumn(field, "boolean", width) }
