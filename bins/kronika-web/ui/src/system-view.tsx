import { Activity, Cpu, Database, Gauge, HardDrive, MemoryStick, Network } from "lucide-react"
import { useEffect, useMemo, useState, type ReactNode } from "react"

import { fieldNameForLocator, type DataRow, type Finding, type HourData, type Point } from "./api"
import { EntityTable, type EntityColumn } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import { asNumber, measure, snapshot, value, type Locale } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

interface MetricSpec {
  readonly id: string
  readonly group: "cpu" | "load" | "memory" | "pressure"
  readonly label: string
  readonly help: string
  readonly section?: string
  readonly field?: string
  readonly series?: string
  readonly resource?: number
  readonly unit: string
}

const METRICS: readonly MetricSpec[] = [
  metric("health", "cpu", "system.metric.health", "health", "os_health", "%"),
  seriesMetric("cpu_busy", "cpu", "system.metric.cpu_busy", "os_cpu_busy_percent", "%"),
  metric("procs_running", "cpu", "system.metric.procs_running", "os_stat", "procs_running", ""),
  metric("procs_blocked", "cpu", "system.metric.procs_blocked", "os_stat", "procs_blocked", ""),
  metric("context_switches", "cpu", "system.metric.context_switches", "os_stat", "ctxt", ""),
  metric("load1", "load", "system.metric.load1", "os_loadavg", "load1", ""),
  metric("load5", "load", "system.metric.load5", "os_loadavg", "load5", ""),
  metric("load15", "load", "system.metric.load15", "os_loadavg", "load15", ""),
  metric("runnable", "load", "system.metric.runnable", "os_loadavg", "running", ""),
  metric("tasks", "load", "system.metric.tasks", "os_loadavg", "total", ""),
  metric("mem_available_percent", "memory", "system.metric.mem_available_percent", "os_meminfo", "mem_available_percent", "%"),
  metric("mem_available", "memory", "system.metric.mem_available", "os_meminfo", "mem_available", " KiB"),
  metric("mem_total", "memory", "system.metric.mem_total", "os_meminfo", "mem_total", " KiB"),
  metric("cached", "memory", "system.metric.cached", "os_meminfo", "cached", " KiB"),
  metric("swap_free", "memory", "system.metric.swap_free", "os_meminfo", "swap_free", " KiB"),
  metric("swap_total", "memory", "system.metric.swap_total", "os_meminfo", "swap_total", " KiB"),
  metric("oom_kill", "memory", "system.metric.oom_kill", "os_vmstat", "oom_kill", ""),
  pressureMetric("cpu_pressure", "system.metric.cpu_pressure", 0),
  pressureMetric("memory_pressure", "system.metric.memory_pressure", 1),
  pressureMetric("io_pressure", "system.metric.io_pressure", 2),
]

const GROUPS: readonly { readonly id: MetricSpec["group"]; readonly icon: ReactNode; readonly label: string }[] = [
  { id: "cpu", icon: <Cpu size={14} />, label: "system.group.cpu" },
  { id: "load", icon: <Gauge size={14} />, label: "system.group.load" },
  { id: "memory", icon: <MemoryStick size={14} />, label: "system.group.memory" },
  { id: "pressure", icon: <Activity size={14} />, label: "system.group.pressure" },
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
  const available = useMemo(() => METRICS.filter((spec) => metricPoints(data, spec).length !== 0), [data])
  const [selected, setSelected] = useState(available[0]?.id ?? "")
  useEffect(() => {
    if (available.some((spec) => spec.id === selected)) return
    setSelected(available[0]?.id ?? "")
  }, [available, selected])
  useEffect(() => {
    if (focus === null) return
    const field = fieldNameForLocator(focus)
    const match = available.find((spec) => spec.section === focus.logicalName && spec.field === field)
    if (match !== undefined) setSelected(match.id)
  }, [available, focus])
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
        return <section className="entity-panel" key={entity.section}>
          <h2>{entity.icon}<span>{t(entity.label)}</span></h2>
          <EntityTable columns={entity.columns} empty={t("table.no_rows")} label={t(entity.label)} locale={locale} rows={rows} testId={`system-${entity.section}`} />
        </section>
      })}
    </section>
  </>
}

function metricPoints(data: HourData, spec: MetricSpec): readonly ChartPoint[] {
  if (spec.series !== undefined) return data.points.filter((point) => point.series === spec.series).map(point)
  if (spec.section === undefined || spec.field === undefined) return []
  const field = spec.field
  return sectionRows(data, spec.section).flatMap((row) => {
    if (spec.resource !== undefined && asNumber(value(row, "resource")) !== spec.resource) return []
    const number = asNumber(value(row, field))
    return [{ segmentId: row.segmentId, timestamp: row.timestamp, value: number }]
  })
}

function currentValue(data: HourData, spec: MetricSpec, cursor: number, locale: Locale): string {
  const points = metricPoints(data, spec)
  let nearest: ChartPoint | null = null
  for (const candidate of points) {
    if (candidate.value === null) continue
    if (nearest === null || Math.abs(candidate.timestamp - cursor) < Math.abs(nearest.timestamp - cursor)
      || (Math.abs(candidate.timestamp - cursor) === Math.abs(nearest.timestamp - cursor) && candidate.timestamp < nearest.timestamp)) nearest = candidate
  }
  return nearest === null ? "—" : measure(nearest.value, locale, spec.unit)
}

function sectionRows(data: HourData, section: string): readonly DataRow[] {
  if (section === "health") return data.health
  if (section === "os_loadavg") return data.load
  if (section === "os_meminfo") return data.memory
  if (section === "os_psi") return data.pressure
  return data.sections[section] ?? []
}

function metric(id: string, group: MetricSpec["group"], label: string, section: string, field: string, unit: string): MetricSpec {
  return { id, group, label, help: `${label}.help`, section, field, unit }
}

function seriesMetric(id: string, group: MetricSpec["group"], label: string, series: string, unit: string): MetricSpec {
  return { id, group, label, help: `${label}.help`, series, unit }
}

function pressureMetric(id: string, label: string, resource: number): MetricSpec {
  return { id, group: "pressure", label, help: `${label}.help`, section: "os_psi", field: "some_avg10", resource, unit: "%" }
}

function point(source: Point): ChartPoint { return source }
function text(field: string, width = 130, sticky = false): EntityColumn { return { field, label: field, kind: "text", width, sticky } }
function number(field: string, width = 126): EntityColumn { return { field, label: field, kind: "number", width } }
function id(field: string, width = 110, sticky = false): EntityColumn { return { field, label: field, kind: "id", width, sticky } }
function bytes(field: string, width = 145): EntityColumn { return { field, label: field, kind: "bytes", width } }
function milliseconds(field: string, width = 145): EntityColumn { return { field, label: field, kind: "milliseconds", width } }
function boolean(field: string, width = 130): EntityColumn { return { field, label: field, kind: "boolean", width } }
