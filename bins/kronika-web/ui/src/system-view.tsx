import { Activity, Cpu, Database, Gauge, HardDrive, MemoryStick, Network } from "lucide-react"
import { useEffect, useMemo, useState, type ReactNode } from "react"

import { fieldNameForLocator, resolveLocator, type DataRow, type Finding, type HourData, type Point } from "./api"
import { EntityTable, type EntityColumn } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import { asNumber, measure, snapshot, value, type Locale } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

interface MetricSpec {
  readonly id: string
  readonly group: "cpu" | "load" | "memory" | "pressure" | "storage"
  readonly label: string
  readonly help: string
  readonly section?: string
  readonly field?: string
  readonly derive?: "cpu_busy" | "mem_available_percent" | "filesystem_free_min"
  readonly series?: string
  readonly resource?: number
  readonly unit: string
}

const METRICS: readonly MetricSpec[] = [
  metric("health", "cpu", "system.metric.health", "health", "os_health", "%"),
  derivedMetric("cpu_busy", "cpu", "system.metric.cpu_busy", "os_cpu_busy_percent", "cpu_busy", "%"),
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
  seriesSectionMetric("oom_kill", "memory", "system.metric.oom_kill", "os_oom_kills", "os_vmstat", "oom_kill", ""),
  pressureMetric("cpu_pressure", "system.metric.cpu_pressure", 0),
  pressureMetric("memory_pressure", "system.metric.memory_pressure", 1),
  pressureMetric("io_pressure", "system.metric.io_pressure", 2),
  derivedMetric("filesystem_free_min", "storage", "system.metric.filesystem_free_min", "os_min_filesystem_free_percent", "filesystem_free_min", "%"),
]

const GROUPS: readonly { readonly id: MetricSpec["group"]; readonly icon: ReactNode; readonly label: string }[] = [
  { id: "cpu", icon: <Cpu size={14} />, label: "system.group.cpu" },
  { id: "load", icon: <Gauge size={14} />, label: "system.group.load" },
  { id: "memory", icon: <MemoryStick size={14} />, label: "system.group.memory" },
  { id: "pressure", icon: <Activity size={14} />, label: "system.group.pressure" },
  { id: "storage", icon: <HardDrive size={14} />, label: "system.group.storage" },
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
  const available = useMemo(() => METRICS.filter((spec) => hasMetric(data, spec)), [data])
  const [selected, setSelected] = useState(available[0]?.id ?? "")
  useEffect(() => {
    if (available.some((spec) => spec.id === selected)) return
    setSelected(available[0]?.id ?? "")
  }, [available, selected])
  useEffect(() => {
    if (focus === null) return
    const field = fieldNameForLocator(focus)
    const focusedRow = resolveLocator(data, focus)?.row ?? null
    const resource = asNumber(value(focusedRow, "resource"))
    const fallback = fallbackMetric(focus.logicalName)
    const match = available.find((spec) => spec.section === focus.logicalName && spec.field === field
      && (spec.resource === undefined || spec.resource === resource))
      ?? (fallback === null ? undefined : available.find((spec) => spec.id === fallback))
    if (match !== undefined) setSelected(match.id)
  }, [available, data, focus])
  const selectedMetric = available.find((spec) => spec.id === selected) ?? available[0]
  const selectedPoints = selectedMetric === undefined ? [] : metricPoints(data, selectedMetric)
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={data.load} memory={data.memory} onCursor={onCursor} onFinding={onFinding} pressure={data.pressure} t={t} />
    <section className="system-console">
      <div className="metric-groups">
        {GROUPS.map((group) => {
          const metrics = available.filter((spec) => spec.group === group.id)
          if (metrics.length === 0) return null
          return <section className="metric-group" key={group.id}>
            <h2>{group.icon}<span>{t(group.label)}</span></h2>
            <div className="metric-grid">
              {metrics.map((spec) => <button aria-pressed={selectedMetric?.id === spec.id} key={spec.id} onClick={() => setSelected(spec.id)} type="button">
                <LabelHelp helpKey={spec.help} labelKey={spec.label} t={t} />
                <strong>{currentValue(data, spec, cursor, locale)}</strong>
              </button>)}
            </div>
          </section>
        })}
      </div>
      <section className="metric-history">
        <header><span>{t("system.history")}</span><strong>{selectedMetric === undefined ? "—" : t(selectedMetric.label)}</strong></header>
        {selectedMetric === undefined
          ? <p className="table-empty">{t("system.no_metrics")}</p>
          : <SeriesChart hour={hour} label={t(selectedMetric.label)} locale={locale} points={selectedPoints} unit={selectedMetric.unit} />}
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

function metricPoints(data: HourData, spec: MetricSpec): readonly ChartPoint[] {
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
  const bySnapshot = new Map<string, { readonly segmentId: string; readonly timestamp: number; readonly values: number[] }>()
  for (const row of sectionRows(data, "os_mountinfo")) {
    const total = asNumber(value(row, "total_bytes"))
    const free = asNumber(value(row, "free_bytes"))
    if (total === null || free === null || total <= 0) continue
    const key = `${row.segmentId}:${row.timestamp}`
    const stored = bySnapshot.get(key) ?? { segmentId: row.segmentId, timestamp: row.timestamp, values: [] }
    stored.values.push(free / total * 100)
    bySnapshot.set(key, stored)
  }
  return [...bySnapshot.values()].map((stored) => ({ segmentId: stored.segmentId, timestamp: stored.timestamp, value: Math.min(...stored.values) }))
}

function cpuBusyPoints(rows: readonly DataRow[]): readonly ChartPoint[] {
  const fields = ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"] as const
  const bySegment = new Map<string, DataRow[]>()
  for (const row of rows) {
    if (asNumber(value(row, "cpu_id")) !== -1 || (asNumber(value(row, "scope")) ?? 0) !== 0) continue
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
  const points = metricPoints(data, spec)
  let nearest: ChartPoint | null = null
  for (const candidate of points) {
    if (nearest === null || Math.abs(candidate.timestamp - cursor) < Math.abs(nearest.timestamp - cursor)
      || (Math.abs(candidate.timestamp - cursor) === Math.abs(nearest.timestamp - cursor) && candidate.timestamp < nearest.timestamp)) nearest = candidate
  }
  return nearest === null ? "—" : measure(nearest.value, locale, spec.unit)
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
