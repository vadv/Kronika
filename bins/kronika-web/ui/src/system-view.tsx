import { registry } from "kronika:registry"
import { X } from "lucide-react"
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { HostSection } from "./address"
import { fieldNameForLocator, loadSeries, resolveLocator, type Cell, type DataRow, type Finding, type HourData, type Point, type SectionRequest } from "./api"
import { buildMetricSamples } from "./chart"
import { ChartOnly, useChartsVisible } from "./chart-visibility"
import { useDetailDismiss } from "./detail-dismiss"
import { contextualRows, type EntityContext } from "./entity-context"
import { EntityTable, type EntityColumn } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import { useHistoryRequest } from "./history-request"
import { asNumber, humanBytes, humanCores, humanPercent, measure, rawText, shownMoment, snapshot, value, type Locale } from "./model"
import { readingAt, SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"
import { UPlotChart, type RecordedSeries } from "./uplot-chart"
import { UseTable, type UseResourceKey } from "./use-table"

interface MetricSpec {
  readonly id: string
  readonly group: HostSection
  readonly label: string
  readonly help: string
  readonly section?: string
  readonly field?: string
  readonly derive?:
    | "cpu_user"
    | "cpu_system"
    | "cpu_irq"
    | "cpu_iowait"
    | "cpu_steal"
    | "cpu_idle"
    | "cpu_used_cores"
    | "cpu_capacity"
    | "mem_file_cache"
    | "mem_other"
    | "filesystem_free_min"
    | "device_busy"
    | "device_average_queue"
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

interface SystemEntityColumn extends EntityColumn {
  readonly chartable?: boolean
  readonly historyFields?: readonly string[]
  readonly points?: (rows: readonly DataRow[]) => readonly ChartPoint[]
}

interface RegistryColumn {
  readonly class: "cumulative" | "gauge" | "label" | "timestamp"
  readonly name: string
  readonly type: string
  readonly unit: string | null
}

export function metricChartUnit(spec: MetricSpec, locale: Locale): string {
  if (spec.unit === "%") return "%"
  if (spec.unit === " KiB") return "B"
  if (spec.unit === " cores") return "cores"
  if (spec.unit === " B") return locale === "ru" ? "байты/с" : "bytes/s"
  if (spec.id === "network_errors" || spec.id === "network_drops") return locale === "ru" ? "1/с" : "1/s"
  if (metricClass(spec) === "cumulative") return locale === "ru" ? "1/с" : "1/s"
  return locale === "ru" ? "количество" : "count"
}

export interface MetricHistoryRequest {
  readonly fields: readonly string[]
  readonly section: string
  readonly where: Readonly<Record<string, string>>
}

export interface EntityHistoryRequest extends MetricHistoryRequest {
  readonly key: string
  readonly typeId: string
}

export interface CgroupSnapshotLoad {
  readonly filters: Readonly<Record<string, string>>
  readonly request: SectionRequest
}

export interface CgroupSnapshotPlan {
  readonly key: string
  readonly loads: readonly CgroupSnapshotLoad[]
}

export const SYSTEM_METRICS: readonly MetricSpec[] = [
  derivedMetric("cpu_used_cores", "cpu", "system.metric.cpu_used_cores", "cpu_used_cores", "cpu_used_cores", " cores"),
  derivedMetric("cpu_capacity", "cpu", "system.metric.cpu_capacity", "cpu_capacity", "cpu_capacity", " cores"),
  derivedMetric("cpu_user", "cpu", "system.metric.cpu_user", "cpu_user", "cpu_user", "%"),
  derivedMetric("cpu_system", "cpu", "system.metric.cpu_system", "cpu_system", "cpu_system", "%"),
  derivedMetric("cpu_irq", "cpu", "system.metric.cpu_irq", "cpu_irq", "cpu_irq", "%"),
  derivedMetric("cpu_iowait", "cpu", "system.metric.cpu_iowait", "cpu_iowait", "cpu_iowait", "%"),
  derivedMetric("cpu_steal", "cpu", "system.metric.cpu_steal", "cpu_steal", "cpu_steal", "%"),
  derivedMetric("cpu_idle", "cpu", "system.metric.cpu_idle", "cpu_idle", "cpu_idle", "%"),
  metric("procs_running", "cpu", "system.metric.procs_running", "os_stat", "procs_running", ""),
  metric("procs_blocked", "cpu", "system.metric.procs_blocked", "os_stat", "procs_blocked", ""),
  metric("context_switches", "cpu", "system.metric.context_switches", "os_stat", "ctxt", ""),
  metric("load1", "cpu", "system.metric.load1", "os_loadavg", "load1", ""),
  metric("load5", "cpu", "system.metric.load5", "os_loadavg", "load5", ""),
  metric("load15", "cpu", "system.metric.load15", "os_loadavg", "load15", ""),
  metric("runnable", "cpu", "system.metric.runnable", "os_loadavg", "running", ""),
  metric("tasks", "cpu", "system.metric.tasks", "os_loadavg", "total", ""),
  metric("mem_available", "memory", "system.metric.mem_available", "os_meminfo", "mem_available", " KiB"),
  metric("mem_total", "memory", "system.metric.mem_total", "os_meminfo", "mem_total", " KiB"),
  metric("mem_anon", "memory", "system.metric.mem_anon", "os_meminfo", "anon_pages", " KiB"),
  derivedMetric("mem_file_cache", "memory", "system.metric.mem_file_cache", "mem_file_cache", "mem_file_cache", " KiB"),
  metric("mem_s_reclaimable", "memory", "system.metric.mem_s_reclaimable", "os_meminfo", "s_reclaimable", " KiB"),
  metric("mem_s_unreclaim", "memory", "system.metric.mem_s_unreclaim", "os_meminfo", "s_unreclaim", " KiB"),
  metric("mem_free", "memory", "system.metric.mem_free", "os_meminfo", "mem_free", " KiB"),
  derivedMetric("mem_other", "memory", "system.metric.mem_other", "mem_other", "mem_other", " KiB"),
  metric("swap_free", "memory", "system.metric.swap_free", "os_meminfo", "swap_free", " KiB"),
  metric("swap_total", "memory", "system.metric.swap_total", "os_meminfo", "swap_total", " KiB"),
  seriesSectionMetric("oom_kill", "memory", "system.metric.oom_kill", "os_oom_kills", "os_vmstat", "oom_kill", ""),
  pressureMetric("cpu_pressure", "cpu", "system.metric.cpu_pressure", 0),
  pressureMetric("memory_pressure", "memory", "system.metric.memory_pressure", 1),
  pressureMetric("io_pressure", "storage", "system.metric.io_pressure", 2),
  derivedMetric("device_busy", "storage", "system.metric.device_busy", "os_device_busy", "device_busy", "%"),
  derivedMetric("device_average_queue", "storage", "system.metric.device_average_queue", "os_device_average_queue", "device_average_queue", ""),
  derivedMetric("filesystem_free_min", "storage", "system.metric.filesystem_free_min", "os_min_filesystem_free_percent", "filesystem_free_min", "%"),
  derivedMetric("device_count", "storage", "system.metric.device_count", "os_device_count", "device_count", ""),
  derivedMetric("device_active_io", "storage", "system.metric.device_active_io", "os_device_active_io", "device_active_io", ""),
  derivedMetric("filesystem_count", "storage", "system.metric.filesystem_count", "os_filesystem_count", "filesystem_count", ""),
  derivedMetric("interface_count", "network", "system.metric.interface_count", "os_interface_count", "interface_count", ""),
  derivedMetric("network_rx", "network", "system.metric.network_rx", "os_network_rx", "network_rx", " B"),
  derivedMetric("network_tx", "network", "system.metric.network_tx", "os_network_tx", "network_tx", " B"),
  derivedMetric("network_errors", "network", "system.metric.network_errors", "os_network_errors", "network_errors", ""),
  derivedMetric("network_drops", "network", "system.metric.network_drops", "os_network_drops", "network_drops", ""),
]

const CPU_FIELDS = ["cpu_id", "scope", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"] as const
const MEMORY_FIELDS = ["mem_total", "mem_available", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim"] as const
const CPU_BREAKDOWN_IDS = ["cpu_used_cores", "cpu_capacity", "cpu_user", "cpu_system", "cpu_irq", "cpu_iowait", "cpu_steal", "cpu_idle"] as const
const MEMORY_BREAKDOWN_IDS = ["mem_total", "mem_available", "mem_anon", "mem_file_cache", "mem_s_reclaimable", "mem_s_unreclaim", "mem_free", "mem_other"] as const
const BREAKDOWN_COLORS: readonly RecordedSeries["color"][] = ["cyan", "green", "blue", "amber", "violet", "red", "gray", "rose"]

// The mount history request fetches both sides of the pair at once.
const MOUNT_PAIR_COLUMN: SystemEntityColumn = { ...bytes("free_bytes"), historyFields: ["free_bytes", "total_bytes"] }

// A section owns its entity panels: what the resource is made of.
const SECTION_ENTITIES: Readonly<Record<HostSection, readonly string[]>> = {
  overview: ["os_topology"],
  cpu: ["os_cgroup_cpu"],
  memory: ["os_cgroup_memory"],
  storage: ["os_diskstats", "os_mountinfo", "os_cgroup_io"],
  network: ["os_netdev"],
}

const DERIVE_INPUTS: Readonly<Record<NonNullable<MetricSpec["derive"]>, readonly [string, readonly string[]]>> = {
  cpu_user: ["os_cpu", CPU_FIELDS],
  cpu_system: ["os_cpu", CPU_FIELDS],
  cpu_irq: ["os_cpu", CPU_FIELDS],
  cpu_iowait: ["os_cpu", CPU_FIELDS],
  cpu_steal: ["os_cpu", CPU_FIELDS],
  cpu_idle: ["os_cpu", CPU_FIELDS],
  cpu_used_cores: ["os_cpu", CPU_FIELDS],
  cpu_capacity: ["os_cpu", CPU_FIELDS],
  mem_file_cache: ["os_meminfo", ["cached", "buffers"]],
  mem_other: ["os_meminfo", ["mem_total", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim"]],
  filesystem_free_min: ["os_mountinfo", ["total_bytes", "free_bytes"]],
  device_busy: ["os_diskstats", ["io_time_ms", "device"]],
  device_average_queue: ["os_diskstats", ["io_weighted_time_ms", "device"]],
  device_count: ["os_diskstats", []],
  device_active_io: ["os_diskstats", ["io_in_progress"]],
  filesystem_count: ["os_mountinfo", []],
  interface_count: ["os_netdev", []],
  network_rx: ["os_netdev", ["rx_bytes"]],
  network_tx: ["os_netdev", ["tx_bytes"]],
  network_errors: ["os_netdev", ["rx_errs", "tx_errs"]],
  network_drops: ["os_netdev", ["rx_drop", "tx_drop"]],
}

// A UseTable row picks the whole group; the lane the row reports is the metric
// the detail chart opens on. Grouped metrics keep their own lanes.
const RESOURCE_GROUP: Readonly<Record<UseResourceKey, MetricSpec["group"]>> = {
  cpu: "cpu",
  memory: "memory",
  disk: "storage",
  network: "network",
}

const RESOURCE_LANE: Readonly<Record<UseResourceKey, string>> = {
  cpu: "cpu_used_cores",
  memory: "mem_available",
  disk: "device_busy",
  network: "network_rx",
}

// A UseTable row picks the whole group and opens the metric the row
// reports: the lane the table already shows, if any, else the first in the group.
function resourceSelection(available: readonly { readonly points: readonly ChartPoint[]; readonly spec: MetricSpec }[], resource: UseResourceKey): string | null {
  const lane = RESOURCE_LANE[resource]
  const target = available.find(({ spec }) => spec.id === lane) ?? available.find(({ spec }) => spec.group === RESOURCE_GROUP[resource])
  return target?.spec.id ?? null
}

// The group of every metric a resource row opens: both the lane the row leads
// with and the submetrics its chips offer.
function metricResource(spec: MetricSpec): UseResourceKey | null {
  return (Object.keys(RESOURCE_GROUP) as UseResourceKey[]).find((key) => RESOURCE_GROUP[key] === spec.group) ?? null
}

// The lane the detail chart leads with when a group is picked without a row:
// the resource lane when the group belongs to one, otherwise the health track.
function groupLane(group: MetricSpec["group"]): string {
  const match = (Object.keys(RESOURCE_GROUP) as UseResourceKey[]).find((key) => RESOURCE_GROUP[key] === group)
  return match === undefined ? "health" : RESOURCE_LANE[match]
}

// A metric's own lane: the metric's exact id when the timeline carries it, the
// mapped lane for normalized metrics, otherwise the resource or health lane.
function metricLane(spec: MetricSpec): string {
  if (normalizedMetricLanes(spec).some(([lane]) => lane === spec.id)) return spec.id
  const lane = timelineLane(spec.id)
  if (lane !== "health" || spec.id === "health") return lane
  return groupLane(spec.group)
}

export const SYSTEM_ENTITIES: readonly {
  readonly section: string
  readonly label: string
  readonly columns: readonly SystemEntityColumn[]
}[] = [
  {
    section: "os_diskstats", label: "system.entities.devices",
    columns: [
      text("device", 150, true), virtualText("device_id", ["major", "minor"]), rateNumber("reads"), rateNumber("writes"),
      derivedRateBytes("read_bytes", ["read_sectors"], (rows) => exactCounterRatePoints(rows, "read_sectors", 512)),
      derivedRateBytes("write_bytes", ["write_sectors"], (rows) => exactCounterRatePoints(rows, "write_sectors", 512)),
      latency("read_latency_ms", "reads", "read_time_ms"), latency("write_latency_ms", "writes", "write_time_ms"),
      derivedPercent("device_busy", ["io_time_ms"], (rows) => exactCounterRatePoints(rows, "io_time_ms", 0.1)),
      derivedNumber("average_queue", ["io_weighted_time_ms"], (rows) => exactCounterRatePoints(rows, "io_weighted_time_ms", 0.001)),
      number("io_in_progress"),
    ],
  },
  {
    section: "os_cgroup_cpu", label: "system.entities.cgroup_cpu",
    columns: [
      text("cgroup_path", 240, true),
      derivedCores("cgroup_used_cores", ["usage_usec"], (rows) => cgroupCpuPoints(rows, "usage_usec")),
      derivedCores("cgroup_user_cores", ["user_usec"], (rows) => cgroupCpuPoints(rows, "user_usec")),
      derivedCores("cgroup_system_cores", ["system_usec"], (rows) => cgroupCpuPoints(rows, "system_usec")),
      derivedCores("cgroup_other_cores", ["usage_usec", "user_usec", "system_usec"], cgroupOtherCpuPoints),
      nonChartCores("cgroup_capacity", []), nonChartCores("cgroup_quota", ["quota_usec", "period_usec"]), nonChartNumber("cpuset_cpus", []),
    ],
  },
  {
    section: "os_cgroup_memory", label: "system.entities.cgroup_memory",
    columns: [
      text("cgroup_path", 240, true), bytes("current"), nonChartBytes("effective_memory_max", []), bytes("max"), bytes("anon"), bytes("file"), bytes("slab"),
      derivedBytes("kernel_other", ["kernel", "slab"], (rows) => differencePoints(rows, "kernel", ["slab"])),
      derivedBytes("memory_unclassified", ["current", "anon", "file", "kernel"], (rows) => differencePoints(rows, "current", ["anon", "file", "kernel"])),
    ],
  },
  {
    section: "os_cgroup_io", label: "system.entities.cgroup_io",
    columns: [text("cgroup_path", 240, true), virtualText("device_id", ["major", "minor"]), rateBytes("rbytes"), rateBytes("wbytes"), rateNumber("rios"), rateNumber("wios")],
  },
  {
    section: "os_mountinfo", label: "system.entities.mounts",
    columns: [text("mount_point", 240, true), text("source", 180), text("fstype", 120), bytes("free_bytes"), bytes("total_bytes"), boolean("is_k8s_infra")],
  },
  {
    section: "os_netdev", label: "system.entities.network",
    columns: [text("iface", 150, true), rateBytes("rx_bytes"), rateBytes("tx_bytes"), rateNumber("rx_packets"), rateNumber("tx_packets"), rateNumber("rx_errs"), rateNumber("tx_errs"), rateNumber("rx_drop"), rateNumber("tx_drop"), number("speed_mbit"), id("duplex")],
  },
  {
    section: "os_topology", label: "system.entities.topology",
    columns: [id("cpu_id", 90, true), id("socket_id"), id("core_id"), id("numa_node"), text("model_name", 300), number("mhz_max")],
  },
]

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
  for (const panel of SYSTEM_ENTITIES) need(panel.section, [
    ...panel.columns.flatMap((column) => column.historyFields ?? [column.field]),
    ...registry.filter((layout) => layout.logicalName === panel.section).flatMap((layout) => layout.identity),
  ])
  for (const section of ["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"]) need(section, ["cgroup_path", "scope"])
  need("os_cgroup_context", [
    "cpu_path", "memory_path", "io_path", "cpuset_cpus", "effective_cpu_quota_usec",
    "effective_cpu_period_usec", "effective_memory_max", "cgroup_version", "scope",
  ])
  return [...wanted].map(([section, fields]) => ({ section, fields: [...fields] }))
}

const CGROUP_PATH_FIELDS = {
  os_cgroup_cpu: "cpu_path",
  os_cgroup_memory: "memory_path",
  os_cgroup_io: "io_path",
} as const
const ALL_SYSTEM_REQUESTS = systemRequests()
const CGROUP_SECTIONS = new Set(Object.keys(CGROUP_PATH_FIELDS))

export const CGROUP_SNAPSHOT_REQUESTS = ALL_SYSTEM_REQUESTS.filter(({ section }) => CGROUP_SECTIONS.has(section))
export const SYSTEM_REQUESTS = ALL_SYSTEM_REQUESTS.filter(({ section }) => !CGROUP_SECTIONS.has(section))

export function cgroupSnapshotPlan(
  segmentId: string,
  cursor: number,
  data: Pick<HourData, "sections">,
  requests: readonly SectionRequest[] = CGROUP_SNAPSHOT_REQUESTS,
): CgroupSnapshotPlan {
  const context = snapshot((data.sections.os_cgroup_context ?? []).filter((row) => row.segmentId === segmentId), cursor)[0] ?? null
  const cpuPath = rawText(value(context, "cpu_path"))
  const memoryPath = rawText(value(context, "memory_path"))
  const ioPath = rawText(value(context, "io_path"))
  const storedScope = rawText(value(context, "scope"))
  const scope = storedScope !== null && /^(?:0|[1-9]\d*)$/.test(storedScope) ? storedScope : null
  const paths = { cpu_path: cpuPath, memory_path: memoryPath, io_path: ioPath }
  const key = JSON.stringify([segmentId, cursor, cpuPath, memoryPath, ioPath, scope])
  const loads = requests.flatMap((request) => {
    const pathField = CGROUP_PATH_FIELDS[request.section as keyof typeof CGROUP_PATH_FIELDS]
    const path = pathField === undefined ? null : paths[pathField]
    return path === null || scope === null
      ? []
      : [{ request, filters: { cgroup_path: path, scope } }]
  })
  return { key, loads }
}

export function clearCgroupSnapshotRows(data: HourData): HourData {
  const sections: Record<string, readonly DataRow[]> = { ...data.sections }
  for (const section of CGROUP_SECTIONS) delete sections[section]
  return { ...data, sections }
}

export function SystemView({
  context,
  contextRow,
  cursor,
  data,
  focus,
  section,
  historyRevision,
  hour,
  locale,
  onCursor,
  onContextClear,
  onFinding,
  tablesLoading = false,
  t,
}: {
  readonly context: EntityContext | null
  readonly contextRow: DataRow | null
  readonly cursor: number
  readonly data: HourData
  readonly focus: Finding | null
  readonly section: HostSection
  readonly historyRevision: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onContextClear: () => void
  readonly onFinding: (finding: Finding) => void
  readonly tablesLoading?: boolean | undefined
  readonly t: Translate
}) {
  const chartsVisible = useChartsVisible()
  const available = useMemo(() => SYSTEM_METRICS.map((spec) => ({ points: metricPoints(data, spec), spec }))
    .filter(({ points }) => points.some((point) => point.value !== null && Number.isFinite(point.value))), [data])
  const sectionMetrics = useMemo(() => available.filter(({ spec }) => spec.group === section), [available, section])
  const [selected, setSelected] = useState(available[0]?.spec.id ?? "")
  const [dockOpen, setDockOpen] = useState(false)
  // Only an empty startup selection is auto-resolved. A chosen metric stays
  // chosen across refresh swaps that briefly hide its section — the dock must
  // not abandon the resource mid-read.
  useEffect(() => {
    const first = available[0]
    if (selected !== "" || first === undefined) return
    setSelected(first.spec.id)
  }, [available, selected])
  const openMetric = (id: string) => {
    setSelected(id)
    setDockOpen(true)
  }
  const appliedFocus = useRef<Finding | null>(null)
  useEffect(() => {
    if (focus === null) {
      appliedFocus.current = null
      return
    }
    if (focus === appliedFocus.current) return
    const field = fieldNameForLocator(focus)
    const focusedRow = resolveLocator(data, focus)?.row ?? null
    const resource = asNumber(value(focusedRow, "resource"))
    const fallback = fallbackMetric(focus.logicalName)
    const match = available.find(({ spec }) => spec.section === focus.logicalName && spec.field === field
      && (spec.resource === undefined || spec.resource === resource))
      ?? (fallback === null ? undefined : available.find(({ spec }) => spec.id === fallback))
    if (match !== undefined) {
      appliedFocus.current = focus
      setSelected(match.spec.id)
      setDockOpen(true)
    }
  }, [available, data, focus])
  const selectedSpec = SYSTEM_METRICS.find((spec) => spec.id === selected) ?? available[0]?.spec
  // The spec comes from the static catalog so the dock keeps its frame while
  // its section is mid-reload; the points honestly empty out for that window.
  const selectedMetric = selectedSpec === undefined ? undefined : {
    points: available.find(({ spec }) => spec.id === selectedSpec.id)?.points ?? [],
    spec: selectedSpec,
  }
  const selectedResource = selectedMetric === undefined ? null : metricResource(selectedMetric.spec)
  const dockShown = chartsVisible && dockOpen && selectedMetric !== undefined
  const dockMeta = useMemo(() => {
    if (selectedMetric === undefined) return { chips: [] as readonly MetricSpec[], chartChip: (id: string) => id }
    const group = selectedMetric.spec.group
    const lane = (Object.keys(RESOURCE_GROUP) as UseResourceKey[]).find((key) => RESOURCE_GROUP[key] === group)
    return dockGroupMetrics(available.filter(({ spec }) => spec.group === group).map(({ spec }) => spec), lane === undefined ? undefined : RESOURCE_LANE[lane])
  }, [available, selectedMetric])
  const fallbackPoints = selectedMetric?.points ?? []
  const request = useMemo(() => selectedMetric === undefined ? null : metricHistoryRequest(selectedMetric.spec), [selectedMetric])
  const requestKey = request === null || selectedMetric === undefined ? null : metricRequestKey(hour, selectedMetric.spec, request)
  const needsHistory = dockShown && request !== null && requestKey !== null && distinctTimes(fallbackPoints) <= 1
  const loadedHistory = useHistoryRequest(needsHistory ? requestKey : null, historyRevision,
    !needsHistory || request === null ? null : (signal) => loadSeries(hour, request.section, request.where, request.fields, signal))
  const selectedPoints = useMemo(() => {
    if (selectedMetric === undefined || loadedHistory.value === null) return fallbackPoints
    const loadedPoints = metricHistoryPoints(selectedMetric.spec, loadedHistory.value)
    return loadedPoints.length === 0 ? fallbackPoints : loadedPoints
  }, [fallbackPoints, loadedHistory.value, selectedMetric])
  const historyRows = loadedHistory.value !== null && loadedHistory.value.length !== 0
    ? loadedHistory.value
    : request === null ? [] : sectionRows(data, request.section)
  const historyUsesRates = loadedHistory.value === null
    && request !== null
    && (data.rateColumns?.[request.section] ?? []).length !== 0
  const breakdown = useMemo(() => selectedMetric === undefined ? [] : resourceBreakdownSeries(
    selectedMetric.spec.id,
    historyRows,
    historyUsesRates,
    locale,
    t,
  ), [historyRows, historyUsesRates, locale, selectedMetric, t])
  const secondLane = selectedMetric === undefined ? null : secondMetricLane(selectedMetric.spec)
  const secondPoints = useMemo(() => secondLane === null ? undefined : laneChartPoints(data, secondLane), [data, secondLane])
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  return <>
    <ChartOnly><Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={onCursor} onFinding={onFinding} primaryLane={selectedMetric === undefined ? "health" : metricLane(selectedMetric.spec)} shownAt={shownAt} t={t} /></ChartOnly>
    <div className="system-main mt-2 min-w-0 [&>.use-table]:mt-0 [&>.metric-groups]:mt-2 [&>.table-empty]:mt-2">
        {section === "overview" && <UseTable canOpen={(resource) => resourceSelection(available, resource) !== null} cursor={cursor} lanePoints={data.lanePoints} locale={locale} onSelect={(resource) => {
          const target = resourceSelection(available, resource)
          if (target !== null) openMetric(target)
        }} selected={dockShown ? selectedResource : null} t={t} />}
      {dockShown && selectedMetric !== undefined && <SystemDock
        chart={breakdown.length === 0
          ? <SeriesChart cursor={cursor} empty={t("history.empty")} format={(reading, place) => metricChartValue(reading, place, selectedMetric.spec.unit)} helpKey={selectedMetric.spec.help} hour={hour} labelKey={selectedMetric.spec.label} locale={locale} onCursor={onCursor} points={selectedPoints} scale={selectedMetric.spec.unit === "%" ? "percent" : "nonnegative"} second={secondPoints} secondHelpKey={secondLane === null ? undefined : "system.metric.network_tx.help"} secondLabelKey={secondLane === null ? undefined : "system.metric.network_tx.label"} stats status={needsHistory ? loadedHistory.status : "ready"} t={t} unit={metricChartUnit(selectedMetric.spec, locale)} />
          : <div className="series-chart"><UPlotChart cursor={cursor} hour={hour} isolate={{ anchor: selectedMetric.spec.id }} locale={locale} onCursor={onCursor} reading={currentPointValue(selectedPoints, cursor, locale, selectedMetric.spec.unit)} series={breakdown} stats status={!needsHistory || loadedHistory.status === "ready" ? undefined : <p className={`series-status series-status-${loadedHistory.status}`} role={loadedHistory.status === "error" ? "alert" : "status"}>{t(`history.${loadedHistory.status}`)}</p>} t={t} testId={`system-${selectedMetric.spec.group}-composition`} /></div>}
        group={selectedMetric.spec.group}
        label={`section.${selectedMetric.spec.group}`}
        metrics={dockMeta.chips}
        onClose={() => setDockOpen(false)}
        onSelect={setSelected}
        selected={dockMeta.chartChip(selectedMetric.spec.id)}
        t={t}
      />}
        {available.length === 0 && <p className="table-empty">{t("system.no_metrics")}</p>}
        {sectionMetrics.length > 0
          && <div className="metric-groups grid grid-cols-1 gap-[7px]">
                <section className="metric-group panel" data-testid={`system-group-${section}`}>
                  <h2 className="panel-head"><span>{t(`section.${section}`)}</span></h2>
                  <div className="metric-grid grid-cols-4 max-[1000px]:grid-cols-2">
                    {sectionMetrics.map(({ points, spec }) => {
                      const output = currentPointValue(points, cursor, locale, spec.unit)
                      return <div className="metric-choice [.metric-groups_&>button]:after:absolute [.metric-groups_&>button]:after:bottom-[5px] [.metric-groups_&>button]:after:right-[7px] [.metric-groups_&>button]:after:text-[11px] [.metric-groups_&>button]:after:text-fg4 [.metric-groups_&>button]:after:opacity-55 [.metric-groups_&>button]:after:content-['↗'] [.metric-groups_&>button:hover]:after:text-accent3 [.metric-groups_&>button:hover]:after:opacity-100 [.metric-groups_&>button:focus-visible]:after:text-accent3 [.metric-groups_&>button:focus-visible]:after:opacity-100 [.metric-groups_&>button_strong]:pr-3.5" key={spec.id}>
                        <button aria-pressed={dockShown && selectedMetric?.spec.id === spec.id} data-testid={`system-metric-${spec.id}`} onClick={() => openMetric(spec.id)} type="button">
                          <span>{t(spec.label)}</span>
                          <strong title={output}>{output}</strong>
                        </button>
                        <LabelHelp helpKey={spec.help} iconOnly labelKey={spec.label} t={t} testId={`system-metric-help-${spec.id}`} />
                      </div>
                    })}
                  </div>
                </section>
          </div>}
    </div>

    <section className="entity-panels mt-2 grid grid-cols-2 gap-2 max-[1000px]:grid-cols-1 charts-hidden:min-h-0 charts-hidden:flex-auto charts-hidden:auto-rows-fr">
      {SYSTEM_ENTITIES.filter((entity) => SECTION_ENTITIES[section].includes(entity.section)).map((entity) => {
        const allRows = systemEntityRows(data, entity.section, cursor)
        const activeContext = context?.logicalName === entity.section ? context : null
        const rows = contextualRows(allRows, activeContext, activeContext === null ? null : contextRow)
        // A section the hour carries is loading, not absent, while its
        // snapshot catches up; a section without rows stays honestly absent.
        if (rows.length === 0 && activeContext === null && !tablesLoading) return null
        if (rows.length === 0 && activeContext === null && !data.availableSections.includes(entity.section)) return null
        const finding = focus?.logicalName === entity.section ? focus : null
        return <SystemEntityPanel
          columns={entity.columns}
          contextLabel={activeContext?.label}
          cursor={cursor}
          finding={finding}
          historyRevision={historyRevision}
          hour={hour}
          key={entity.section}
          label={t(entity.label)}
          locale={locale}
          onContextClear={activeContext === null ? undefined : onContextClear}
          onCursor={onCursor}
          rows={rows}
          section={entity.section}
          tablesLoading={tablesLoading}
          t={t}
        />
      })}
    </section>
  </>
}

// The dock is the System counterpart of the PostgreSQL detail panel: a click
// on a Use row or a metric chip opens it on the resource's group, and the chart
// lives only inside it — nothing on the page silently swaps its content.
function SystemDock({
  chart,
  group,
  label,
  metrics,
  onClose,
  onSelect,
  selected,
  t,
}: {
  readonly chart: ReactNode
  readonly group: MetricSpec["group"]
  readonly label: string
  readonly metrics: readonly MetricSpec[]
  readonly onClose: () => void
  readonly onSelect: (id: string) => void
  readonly selected: string
  readonly t: Translate
}) {
  const detail = useDetailDismiss(onClose, `system:${group}`)
  // The click may have happened below the fold; bring the opened panel into
  // view once, minimally.
  useEffect(() => {
    detail.current?.scrollIntoView({ block: "nearest" })
  }, [])
  return <aside aria-label={t(label)} className="pg-detail system-dock mt-2 max-h-none overflow-visible border border-line3 max-[1000px]:static max-[1000px]:bottom-auto max-[1000px]:right-auto max-[1000px]:top-auto max-[1000px]:w-auto max-[1000px]:max-w-none max-[1000px]:max-h-none max-[1000px]:overflow-visible max-[1000px]:shadow-none" data-testid="system-dock" ref={detail}>
    <header className="pg-detail-head">
      <div><span>{t("system.history")}</span><h2>{t(label)}</h2></div>
      <button aria-label={t("common.close")} onClick={onClose} type="button"><X aria-hidden="true" size={14} /></button>
    </header>
    <section className="process-history mt-2.5 grid min-w-0 gap-[7px] border-t border-line3 pt-[7px]">
      <div aria-label={t(label)} className="dock-tabs history-selector flex max-w-full gap-[5px] overflow-x-auto p-px pb-[3px] [scrollbar-width:thin]" role="group">
        {metrics.map((spec) => <button aria-pressed={spec.id === selected} data-testid={`system-dock-metric-${spec.id}`} key={spec.id} onClick={() => onSelect(spec.id)} type="button">{t(spec.label)}</button>)}
      </div>
      {chart}
    </section>
  </aside>
}

// A mount's free and total bytes are one fact: how full the filesystem is.
// The pair charts together — used space against the total ceiling — instead of
// two switchable single-series charts that make the operator subtract in their
// head. Used is derived elementarily, total − free, the way df reports it.
export function mountPairSeries(rows: readonly DataRow[], t: Translate): readonly RecordedSeries[] {
  const total = buildMetricSamples(rows, (row) => storedNumber(row, "total_bytes"))
  const used = buildMetricSamples(rows, (row) => {
    const capacity = storedNumber(row, "total_bytes")
    const free = storedNumber(row, "free_bytes")
    if (capacity === undefined || free === undefined) return undefined
    return capacity === null || free === null || capacity < free ? null : capacity - free
  })
  if (!used.some((point) => point.value !== null) && !total.some((point) => point.value !== null)) return []
  const format = (reading: number, place: Locale) => humanBytes(reading, place)
  return [
    { color: "cyan", helpKey: "system.field.used_bytes.help", id: "used_bytes", label: t("system.field.used_bytes.label"), labelKey: "system.field.used_bytes.label", points: used, scale: "nonnegative", tick: format, unit: "B", value: format },
    { color: "gray", helpKey: "system.field.total_bytes.help", id: "total_bytes", label: t("system.field.total_bytes.label"), labelKey: "system.field.total_bytes.label", points: total, scale: "nonnegative", tick: format, unit: "B", value: format },
  ]
}

function SystemEntityPanel({
  columns,
  contextLabel,
  cursor,
  tablesLoading,
  finding,
  historyRevision,
  hour,
  label,
  locale,
  onContextClear,
  onCursor,
  rows,
  section,
  t,
}: {
  readonly columns: readonly SystemEntityColumn[]
  readonly contextLabel?: string | undefined
  readonly cursor: number
  readonly finding: Finding | null
  readonly historyRevision: number
  readonly hour: number
  readonly label: string
  readonly locale: Locale
  readonly onContextClear?: (() => void) | undefined
  readonly onCursor: (timestamp: number) => void
  readonly rows: readonly DataRow[]
  readonly tablesLoading?: boolean | undefined
  readonly section: string
  readonly t: Translate
}) {
  const chartsVisible = useChartsVisible()
  const metricColumns = useMemo(() => chartableEntityColumns(columns), [columns])
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const selectedRow = selectedKey === null ? null : rows.find((row) => entityRowKey(row) === selectedKey) ?? null
  const availableColumns = useMemo(() => selectedRow === null
    ? []
    : metricColumns.filter((column) => Object.hasOwn(selectedRow.values, physicalField(column, selectedRow.typeId))), [metricColumns, selectedRow])
  const [selectedField, setSelectedField] = useState("")
  useEffect(() => {
    if (selectedKey !== null && selectedRow === null) setSelectedKey(null)
  }, [selectedKey, selectedRow])
  useEffect(() => {
    if (availableColumns.some((column) => column.field === selectedField)) return
    setSelectedField(availableColumns[0]?.field ?? "")
  }, [availableColumns, selectedField])
  const selectedColumn = availableColumns.find((column) => column.field === selectedField) ?? availableColumns[0]
  const mountPair = section === "os_mountinfo" && selectedRow !== null
    && Object.hasOwn(selectedRow.values, "free_bytes") && Object.hasOwn(selectedRow.values, "total_bytes")
  const requestColumn = mountPair ? MOUNT_PAIR_COLUMN : selectedColumn
  const historyRequest = selectedRow === null || requestColumn === undefined ? null : entityHistoryRequest(selectedRow, requestColumn)
  const historyKey = historyRequest === null ? null : `${hour}:${historyRequest.key}`
  const requestFields = historyRequest === null ? "[]" : JSON.stringify(historyRequest.fields)
  const requestWhere = historyRequest === null ? "{}" : JSON.stringify(historyRequest.where)
  const requestSection = historyRequest?.section ?? ""
  const requestTypeId = historyRequest?.typeId
  const visibleHistoryKey = chartsVisible ? historyKey : null
  const history = useHistoryRequest(visibleHistoryKey, historyRevision,
    visibleHistoryKey === null || requestSection === "" || requestTypeId === undefined ? null : (signal) => {
    const fields = JSON.parse(requestFields) as readonly string[]
    const where = JSON.parse(requestWhere) as Readonly<Record<string, string>>
    return loadSeries(hour, requestSection, where, fields, signal, requestTypeId)
  })
  const chartRows = history.value?.length ? history.value : selectedRow === null ? [] : [selectedRow]
  const chartPoints = useMemo(() => selectedColumn === undefined ? [] : entityMetricPoints(chartRows, selectedColumn), [chartRows, selectedColumn])
  const pairSeries = useMemo(() => mountPair ? mountPairSeries(chartRows, t) : null, [chartRows, mountPair, t])
  const chartMetadata = selectedRow === null || selectedColumn === undefined || selectedColumn.historyFields !== undefined
    ? null : registryColumn(selectedRow.typeId, physicalField(selectedColumn, selectedRow.typeId))
  return <section className="entity-panel panel min-w-0 charts-hidden:flex charts-hidden:flex-col" data-testid={`system-panel-${section}`}>
    <h2 className="panel-head"><span>{label}</span></h2>
    <EntityTable
      columns={columns}
      contextLabel={contextLabel}
      empty={t("table.no_rows")}
      loading={tablesLoading && rows.length === 0}
      finding={finding}
      findingField={finding === null ? null : fieldNameForLocator(finding)}
      label={label}
      locale={locale}
      onContextClear={onContextClear}
      {...(metricColumns.length === 0 ? {} : { onSelect: (row: DataRow) => {
        const key = entityRowKey(row)
        setSelectedKey(key)
        if (key !== selectedKey) {
          const first = metricColumns.find((column) => Object.hasOwn(row.values, physicalField(column, row.typeId)))
          setSelectedField(first?.field ?? "")
        }
      } })}
      rowKey={entityRowKey}
      rows={rows}
      selectedKey={selectedKey}
      t={t}
      testId={`system-${section}`}
    />
    <ChartOnly>{selectedRow !== null && (mountPair || selectedColumn !== undefined) && <section className="system-entity-history min-w-0 border border-t-0 border-line2" data-testid={`system-${section}-history`}>
      <header className="flex items-start justify-between gap-1.5 px-[7px] pt-1.5">
        {mountPair
          ? <div className="system-history-selector flex max-w-[calc(100%-30px)] gap-1 overflow-x-auto pb-[3px] [scrollbar-width:thin] [&>button]:min-h-[27px] [&>button]:flex-none [&>button]:cursor-pointer [&>button]:border [&>button]:border-line3 [&>button]:bg-s2 [&>button]:px-[7px] [&>button]:py-1 [&>button]:text-xs [&>button]:text-fg2 [&>button[aria-pressed=true]]:border-accent [&>button[aria-pressed=true]]:bg-accent-soft [&>button[aria-pressed=true]]:text-fg" role="group" />
          : <div className="system-history-selector flex max-w-[calc(100%-30px)] gap-1 overflow-x-auto pb-[3px] [scrollbar-width:thin] [&>button]:min-h-[27px] [&>button]:flex-none [&>button]:cursor-pointer [&>button]:border [&>button]:border-line3 [&>button]:bg-s2 [&>button]:px-[7px] [&>button]:py-1 [&>button]:text-xs [&>button]:text-fg2 [&>button[aria-pressed=true]]:border-accent [&>button[aria-pressed=true]]:bg-accent-soft [&>button[aria-pressed=true]]:text-fg" role="group">
            {availableColumns.map((column) => <button aria-pressed={column.field === selectedColumn?.field} key={column.field} onClick={() => setSelectedField(column.field)} type="button">{t(column.label)}</button>)}
          </div>}
        <button aria-label={t("common.close")} className="min-h-[27px] min-w-[27px] flex-none cursor-pointer border border-line3 bg-s2 px-[5px] py-px text-md text-fg2" onClick={() => setSelectedKey(null)} type="button">×</button>
      </header>
      {mountPair
        ? pairSeries === null || (pairSeries.length === 0 && history.status === "ready")
          ? <p className="table-empty">{t("status.no_data")}</p>
          : <div className="series-chart"><UPlotChart
              cursor={cursor}
              hour={hour}
              locale={locale}
              onCursor={onCursor}
              reading={(() => { const stored = readingAt(pairSeries?.[0]?.points ?? [], cursor); return stored === null ? "—" : humanBytes(stored, locale) })()}
              series={pairSeries ?? []}
              status={history.status === "ready" ? undefined : <p className={`series-status series-status-${history.status}`} role={history.status === "error" ? "alert" : "status"}>{t(`history.${history.status}`)}</p>}
              t={t}
              testId={`system-${section}-pair`}
            /></div>
        : selectedColumn !== undefined && <SeriesChart
            cursor={cursor}
            empty={t("status.no_data")}
            format={(reading, place) => entityMetricValue(reading, place, selectedColumn, chartMetadata)}
            helpKey={selectedColumn.help ?? "chart.metric.help"}
            hour={hour}
            labelKey={selectedColumn.label}
            locale={locale}
            onCursor={onCursor}
            points={chartPoints}
            scale={selectedColumn.kind === "percent" ? "percent" : "nonnegative"}
            status={history.status}
            t={t}
            unit={entityMetricUnit(selectedColumn, locale, chartMetadata)}
          />}
    </section>}</ChartOnly>
  </section>
}

export function chartableEntityColumns(columns: readonly SystemEntityColumn[]): readonly SystemEntityColumn[] {
  return columns.filter((column) => column.chartable !== false && (column.kind === "number"
    || column.kind === "estimated_rows"
    || column.kind === "bytes"
    || column.kind === "kib"
    || column.kind === "milliseconds"
    || column.kind === "duration"
    || column.kind === "microseconds"
    || column.kind === "percent"
    || column.kind === "cores"))
}

export function entityHistoryRequest(row: DataRow, column: SystemEntityColumn): EntityHistoryRequest | null {
  if (!chartableEntityColumns([column]).includes(column)) return null
  const layout = registry.find((candidate) => candidate.typeId === row.typeId && candidate.logicalName === row.logicalName)
  if (layout === undefined || layout.identity.length === 0) return null
  const identities = layout.identity.map((field) => [field, rawText(value(row, field))] as const)
  if (identities.some(([, stored]) => stored === null)) return null
  const field = physicalField(column, row.typeId)
  const where = Object.fromEntries(identities) as Readonly<Record<string, string>>
  const fields = uniqueStrings([...(column.historyFields ?? [field]), ...layout.identity])
  return {
    fields,
    key: JSON.stringify([row.typeId, identities, field]),
    section: row.logicalName,
    typeId: row.typeId,
    where,
  }
}

function entityMetricPoints(rows: readonly DataRow[], column: SystemEntityColumn): readonly ChartPoint[] {
  if (column.points !== undefined) return column.points(rows)
  const points = buildMetricSamples(rows, (row) => {
    const field = physicalField(column, row.typeId)
    return Object.hasOwn(row.values, field) ? asNumber(value(row, field)) : undefined
  })
  const first = rows[0]
  return first !== undefined && registryColumn(first.typeId, physicalField(column, first.typeId))?.class === "cumulative"
    ? cumulativeRate(points)
    : points
}

function entityRowKey(row: DataRow): string {
  const layout = registry.find((candidate) => candidate.typeId === row.typeId && candidate.logicalName === row.logicalName)
  if (layout === undefined || layout.identity.length === 0) return rowKey(row)
  return JSON.stringify([row.segmentId, row.typeId, layout.identity.map((field) => rawText(value(row, field)))])
}

function physicalField(column: EntityColumn, typeId: string): string {
  if (typeof column.physicalField === "string") return column.physicalField
  return column.physicalField?.[typeId] ?? column.field
}

function entityMetricUnit(column: SystemEntityColumn, locale: Locale, metadata: RegistryColumn | null): string {
  const perSecond = metadata?.class === "cumulative" || column.rate === true ? "/s" : ""
  if (column.kind === "cores") return "cores"
  if (column.field === "speed_mbit") return "Mbit/s"
  if (column.field === "mhz_max") return "MHz"
  if (column.kind === "bytes" || column.kind === "kib") return `${locale === "ru" ? "байты" : "bytes"}${perSecond}`
  if (column.kind === "milliseconds" || column.kind === "duration") return `${locale === "ru" ? "мс" : "ms"}${perSecond}`
  if (column.kind === "microseconds") return `${locale === "ru" ? "мкс" : "µs"}${perSecond}`
  if (column.kind === "percent") return "%"
  if (metadata?.unit === "sectors") return `${locale === "ru" ? "секторы" : "sectors"}${perSecond}`
  return metadata?.class === "cumulative" ? (locale === "ru" ? "1/с" : "1/s") : (locale === "ru" ? "количество" : "count")
}

function entityMetricValue(reading: number, locale: Locale, column: SystemEntityColumn, metadata: RegistryColumn | null): string {
  const suffix = metadata?.class === "cumulative" || column.rate === true ? "/s" : ""
  if (column.kind === "cores") return humanCores(reading, locale)
  if (column.field === "speed_mbit") return measure(reading, locale, " Mbit/s")
  if (column.field === "mhz_max") return measure(reading, locale, " MHz")
  if (column.kind === "bytes") return humanBytes(reading, locale, suffix)
  if (column.kind === "kib") return humanBytes(reading * 1024, locale, suffix)
  if (column.kind === "milliseconds" || column.kind === "duration") return measure(reading, locale, `${locale === "ru" ? " мс" : " ms"}${suffix}`)
  if (column.kind === "microseconds") return measure(reading, locale, `${locale === "ru" ? " мкс" : " µs"}${suffix}`)
  if (column.kind === "percent") return humanPercent(reading, locale)
  return measure(reading, locale, suffix)
}

function timelineLane(metric: string | undefined): string {
  if (metric?.startsWith("cpu_") === true) return "cpu_busy"
  if (metric === "cpu_pressure") return "cpu_stall"
  if (metric === "io_pressure") return "io_stall"
  if (metric?.startsWith("mem_") === true) return "memory"
  return "health"
}

// The lanes a metric reports on the timeline, with the transform the stored
// lane value needs to match the metric's unit.
function normalizedMetricLanes(spec: MetricSpec): readonly (readonly [string, (value: number) => number])[] {
  const mapping: Readonly<Record<string, readonly (readonly [string, (value: number) => number])[]>> = {
    cpu_pressure: [["cpu_stall", (number) => number]],
    io_pressure: [["io_stall", (number) => number]],
    network_rx: [["net_rx", (number) => number], ["net_tx", (number) => number]],
    network_errors: [["net_errors", (number) => number]],
    network_drops: [["net_drop", (number) => number]],
    oom_kill: [["mem_oom", (number) => number]],
  }
  return mapping[spec.id] ?? []
}

// The second timeline lane a metric chart overlays, when the pair shares the
// metric's unit. Only the network throughput pair qualifies today.
function secondMetricLane(spec: MetricSpec): string | null {
  return spec.id === "network_rx" ? "net_tx" : null
}

function laneChartPoints(data: HourData, lane: string): readonly ChartPoint[] {
  return data.lanePoints.filter((point) => point.lane === lane).map((point) => ({
    segmentId: point.segmentId,
    timestamp: point.timestamp,
    value: point.value,
  }))
}

function normalizedMetricPoints(data: HourData, spec: MetricSpec): readonly ChartPoint[] {
  const selected = normalizedMetricLanes(spec)
  if (selected.length === 0) return []
  const [lane, transform] = selected[0]!
  return data.lanePoints.filter((point) => point.lane === lane).map((point) => ({
    segmentId: point.segmentId,
    timestamp: point.timestamp,
    value: point.value === null ? null : transform(point.value),
  }))
}

export function metricPoints(data: HourData, spec: MetricSpec): readonly ChartPoint[] {
  const normalized = normalizedMetricPoints(data, spec)
  if (normalized.length !== 0) return normalized
  if (spec.series !== undefined) {
    const stored = data.points.filter((point) => point.series === spec.series).map(point)
    if (stored.length !== 0) return stored
  }
  if (spec.derive !== undefined) return derivedPoints(data, spec.derive)
  if (spec.section === undefined || spec.field === undefined) return []
  // Counter rollups stay absent until the section layout announces its rate
  // columns; without the rates a raw counter reads as a climbing total, not a
  // per-second value.
  if (metricClass(spec) === "cumulative" && !(data.rateColumns?.[spec.section] ?? []).includes(spec.field)) return []
  const field = spec.field
  return buildMetricSamples(sectionRows(data, spec.section), (row) => {
    if (spec.resource !== undefined && asNumber(value(row, "resource")) !== spec.resource) return undefined
    return storedNumber(row, field)
  })
}

const BREAKDOWN_MEMBER_IDS: ReadonlySet<string> = new Set([...CPU_BREAKDOWN_IDS, ...MEMORY_BREAKDOWN_IDS])

// The composition chart already legends every breakdown member; the dock offers
// one chip for it — the group's lane metric — instead of a strip of chips that
// all draw the same picture.
export function dockGroupMetrics(
  groupMetrics: readonly MetricSpec[],
  laneId: string | undefined,
): { readonly chips: readonly MetricSpec[]; readonly chartChip: (id: string) => string } {
  const members = groupMetrics.filter((spec) => BREAKDOWN_MEMBER_IDS.has(spec.id))
  if (members.length === 0) return { chips: groupMetrics, chartChip: (id) => id }
  const anchor = members.find((spec) => spec.id === laneId) ?? members[0]!
  return {
    chips: groupMetrics.filter((spec) => !BREAKDOWN_MEMBER_IDS.has(spec.id) || spec.id === anchor.id),
    chartChip: (id) => BREAKDOWN_MEMBER_IDS.has(id) ? anchor.id : id,
  }
}

export function resourceBreakdownSeries(
  selectedId: string,
  rows: readonly DataRow[],
  rates: boolean,
  locale: Locale,
  t: Translate,
): readonly RecordedSeries[] {
  if (selectedId === "device_busy") return deviceBreakdownSeries(rows, "io_time_ms", 0.1, rates, locale)
  if (selectedId === "device_average_queue") return deviceBreakdownSeries(rows, "io_weighted_time_ms", 0.001, rates, locale)
  const ids: readonly string[] = CPU_BREAKDOWN_IDS.includes(selectedId as typeof CPU_BREAKDOWN_IDS[number])
    ? CPU_BREAKDOWN_IDS
    : MEMORY_BREAKDOWN_IDS.includes(selectedId as typeof MEMORY_BREAKDOWN_IDS[number]) ? MEMORY_BREAKDOWN_IDS : []
  return ids.flatMap((id, index) => {
    const spec = SYSTEM_METRICS.find((candidate) => candidate.id === id)
    const color = BREAKDOWN_COLORS[index]
    if (spec === undefined || color === undefined) return []
    const points = spec.derive === undefined
      ? buildMetricSamples(rows, (row) => spec.field === undefined ? undefined : storedNumber(row, spec.field))
      : derivedRowPoints(rows, spec.derive, rates)
    const format = (reading: number, place: Locale) => metricChartValue(reading, place, spec.unit)
    return [{
      color,
      helpKey: spec.help,
      id,
      label: t(spec.label),
      labelKey: spec.label,
      points,
      scale: spec.unit === "%" ? "percent" as const : "nonnegative" as const,
      tick: format,
      unit: metricChartUnit(spec, locale),
      value: format,
    }]
  })
}

export function metricHistoryRequest(spec: MetricSpec): MetricHistoryRequest | null {
  const section = spec.derive === undefined ? spec.section : DERIVE_INPUTS[spec.derive][0]
  if (section === undefined) return null
  const derivedFields = spec.derive === undefined ? [] : DERIVE_INPUTS[spec.derive][1]
  const compositionFields = MEMORY_BREAKDOWN_IDS.includes(spec.id as typeof MEMORY_BREAKDOWN_IDS[number]) ? MEMORY_FIELDS : []
  const fields = uniqueStrings([
    ...(spec.field === undefined ? [] : [spec.field]),
    ...derivedFields,
    ...compositionFields,
    ...(spec.resource === undefined ? [] : ["resource"]),
    ...registry.filter((layout) => layout.logicalName === section).flatMap((layout) => layout.identity),
  ])
  if (fields.length === 0) return null
  return {
    fields,
    section,
    where: spec.resource === undefined ? {} : { resource: String(spec.resource) },
  }
}

export function metricRequestKey(hour: number, spec: MetricSpec, request: MetricHistoryRequest): string {
  return JSON.stringify([hour, spec.id, request.section, request.fields, Object.entries(request.where).sort()])
}

export function metricHistoryPoints(spec: MetricSpec, rows: readonly DataRow[]): readonly ChartPoint[] {
  if (spec.derive !== undefined) return derivedRowPoints(rows, spec.derive, false)
  if (spec.field === undefined) return []
  const field = spec.field
  const points = buildMetricSamples(rows, (row) => {
    if (spec.resource !== undefined && asNumber(value(row, "resource")) !== spec.resource) return undefined
    return storedNumber(row, field)
  })
  return metricClass(spec) === "cumulative" ? cumulativeRate(points) : points
}

export function hasMetric(data: HourData, spec: MetricSpec): boolean {
  return metricPoints(data, spec).some((point) => point.value !== null && Number.isFinite(point.value))
}

function derivedPoints(data: HourData, derive: NonNullable<MetricSpec["derive"]>): readonly ChartPoint[] {
  const [section, fields] = DERIVE_INPUTS[derive]
  const announced = data.rateColumns?.[section] ?? []
  const rates = fields.length === 0 ? announced.length !== 0 : fields.some((field) => announced.includes(field))
  return derivedRowPoints(sectionRows(data, section), derive, rates)
}

function derivedRowPoints(rows: readonly DataRow[], derive: NonNullable<MetricSpec["derive"]>, rates: boolean): readonly ChartPoint[] {
  if (derive.startsWith("cpu_")) return cpuPoints(rows, derive, rates)
  if (derive === "mem_file_cache") return buildMetricSamples(rows, (row) => sumStored(row, ["cached", "buffers"]))
  if (derive === "mem_other") return buildMetricSamples(rows, (row) => difference(row, "mem_total", ["mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim"]))
  if (derive === "filesystem_free_min") return aggregateRows(rows, (sampleRows) => {
    const percentages = sampleRows.map((row) => {
      const total = storedNumber(row, "total_bytes")
      const free = storedNumber(row, "free_bytes")
      if (total === undefined || free === undefined) return undefined
      return total === null || free === null || total <= 0 ? null : free / total * 100
    })
    if (percentages.some((number) => number === undefined)) return undefined
    return percentages.some((number) => number === null) ? null : Math.min(...percentages as number[])
  })
  if (derive === "device_busy") return peakDeviceRate(rows, "io_time_ms", rates, 0.1)
  if (derive === "device_average_queue") return peakDeviceRate(rows, "io_weighted_time_ms", rates, 0.001)
  if (derive === "device_count" || derive === "filesystem_count" || derive === "interface_count") return aggregateRows(rows, (sampleRows) => sampleRows.length)
  if (derive === "device_active_io") return aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["io_in_progress"]))
  if (derive === "network_rx") return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["rx_bytes"])))
  if (derive === "network_tx") return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["tx_bytes"])))
  if (derive === "network_errors") return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["rx_errs", "tx_errs"])))
  return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["rx_drop", "tx_drop"])))
}

// The rollup peaks across devices at every instant. Counter layouts divide
// per device first — the peak of sums would double-count devices that share a
// busy period — while rate layouts peak directly.
function peakDeviceRate(rows: readonly DataRow[], field: string, rates: boolean, scale: number): readonly ChartPoint[] {
  if (rates) return aggregateRows(rows, (sampleRows) => maxField(sampleRows, field, scale))
  const devices = new Map<string, DataRow[]>()
  for (const row of rows) {
    const key = rawText(value(row, "major")) + ":" + rawText(value(row, "minor"))
    const stored = devices.get(key) ?? []
    stored.push(row)
    devices.set(key, stored)
  }
  const instants = new Map<string, { readonly segmentId: string; readonly timestamp: number; peak: number | null }>()
  for (const deviceRows of devices.values()) {
    for (const point of exactCounterRatePoints(deviceRows, field, scale)) {
      const key = `${point.segmentId}:${point.timestamp}`
      const stored = instants.get(key) ?? { segmentId: point.segmentId, timestamp: point.timestamp, peak: null }
      if (point.value !== null && (stored.peak === null || point.value > stored.peak)) stored.peak = point.value
      instants.set(key, stored)
    }
  }
  return [...instants.values()]
    .sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId))
    .map(({ segmentId, timestamp, peak }) => ({ segmentId, timestamp, value: peak }))
}

// The dock's storage chart is the rollup broken back down: one line per
// device, computed exactly like the peak's per-device leg. A device that
// registered no activity all hour is a flat zero and stays out of the
// picture; if every device was idle the flat truth is shown whole.
function deviceBreakdownSeries(
  rows: readonly DataRow[],
  field: string,
  scale: number,
  rates: boolean,
  locale: Locale,
): readonly RecordedSeries[] {
  const devices = new Map<string, { readonly name: string; readonly rows: DataRow[] }>()
  for (const row of rows) {
    const key = `${rawText(value(row, "major"))}:${rawText(value(row, "minor"))}`
    const stored = devices.get(key) ?? { name: rawText(value(row, "device")) ?? key, rows: [] }
    stored.rows.push(row)
    devices.set(key, stored)
  }
  const series = [...devices.entries()].map(([key, device]) => {
    const points = rates
      ? buildMetricSamples(device.rows, (row) => {
          const stored = storedNumber(row, field)
          return stored === undefined ? undefined : stored === null ? null : stored * scale
        })
      : exactCounterRatePoints(device.rows, field, scale)
    const peak = points.reduce((max, point) => point.value !== null && point.value > max ? point.value : max, 0)
    return { device, key, peak, points }
  })
  const active = series.filter(({ points }) => points.some((point) => point.value !== null && point.value > 0))
  const shown = (active.length === 0 ? series : active).sort((left, right) => right.peak - left.peak || left.key.localeCompare(right.key))
  const percent = scale === 0.1
  const format = percent
    ? (reading: number, place: Locale) => metricChartValue(reading, place, "%")
    : (reading: number, place: Locale) => measure(reading, place)
  return shown.map(({ device, key, points }, index) => ({
    color: BREAKDOWN_COLORS[index % BREAKDOWN_COLORS.length]!,
    helpKey: percent ? "system.field.device_busy.help" : "system.field.average_queue.help",
    id: `device_${key}`,
    label: device.name,
    labelKey: device.name,
    points,
    scale: percent ? "percent" as const : "nonnegative" as const,
    tick: format,
    unit: percent ? "%" : locale === "ru" ? "количество" : "count",
    value: format,
  }))
}

function aggregateRows(rows: readonly DataRow[], aggregate: (rows: readonly DataRow[]) => number | null | undefined): readonly ChartPoint[] {
  const groups = new Map<string, { readonly rows: DataRow[]; readonly segmentId: string; readonly timestamp: number }>()
  for (const row of rows) {
    const key = `${row.segmentId}:${row.timestamp}`
    const stored = groups.get(key) ?? { rows: [], segmentId: row.segmentId, timestamp: row.timestamp }
    stored.rows.push(row)
    groups.set(key, stored)
  }
  return buildMetricSamples(
    [...groups.values()].sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId)),
    (stored) => aggregate(stored.rows),
  )
}

// The worst device carries the rollup: a host is as busy as its busiest disk,
// an average would hide one saturated device behind idle ones.
function maxField(rows: readonly DataRow[], field: string, scale: number): number | null | undefined {
  let peak: number | null = null
  for (const row of rows) {
    const stored = storedNumber(row, field)
    if (stored === undefined) return undefined
    if (stored === null) continue
    if (peak === null || stored > peak) peak = stored
  }
  return peak === null ? null : peak * scale
}

function sumFields(rows: readonly DataRow[], fields: readonly string[]): number | null | undefined {
  let total = 0
  for (const row of rows) {
    for (const field of fields) {
      const number = storedNumber(row, field)
      if (number === undefined) return undefined
      if (number === null) return null
      total += number
    }
  }
  return total
}

function sumStored(row: DataRow, fields: readonly string[]): number | null | undefined {
  let total = 0
  for (const field of fields) {
    const stored = storedNumber(row, field)
    if (stored === undefined || stored === null) return stored
    total += stored
  }
  return total
}

function difference(row: DataRow, totalField: string, parts: readonly string[]): number | null | undefined {
  const total = storedNumber(row, totalField)
  const used = sumStored(row, parts)
  if (total === undefined || used === undefined) return undefined
  if (total === null || used === null || total < used) return null
  return total - used
}

function differencePoints(rows: readonly DataRow[], total: string, parts: readonly string[]): readonly ChartPoint[] {
  return buildMetricSamples(rows, (row) => difference(row, total, parts))
}

function exactInteger(row: DataRow, field: string): bigint | null {
  const stored = rawText(value(row, field))
  return stored !== null && /^-?\d+$/.test(stored) ? BigInt(stored) : null
}

function exactCounterRatePoints(rows: readonly DataRow[], field: string, scale: number): readonly ChartPoint[] {
  return exactDeltaPoints(rows, [field], ([delta], elapsed) => Number(delta!) * scale * 1_000_000 / elapsed)
}

function latencyPoints(rows: readonly DataRow[], operations: string, duration: string): readonly ChartPoint[] {
  return exactDeltaPoints(rows, [operations, duration], ([count, time]) => count! === 0n ? null : Number(time!) / Number(count!))
}

function cgroupCpuPoints(rows: readonly DataRow[], field: string): readonly ChartPoint[] {
  return exactDeltaPoints(rows, [field], ([delta], elapsed) => Number(delta!) / elapsed)
}

function cgroupOtherCpuPoints(rows: readonly DataRow[]): readonly ChartPoint[] {
  return exactDeltaPoints(rows, ["usage_usec", "user_usec", "system_usec"], ([usage, user, system], elapsed) => {
    const other = usage! - user! - system!
    return other < 0n ? null : Number(other) / elapsed
  })
}

function exactDeltaPoints(rows: readonly DataRow[], fields: readonly string[], output: (deltas: readonly bigint[], elapsedUsec: number) => number | null): readonly ChartPoint[] {
  let previous: { readonly at: number; readonly values: readonly bigint[] } | null = null
  return rows.slice().sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId)).map((row) => {
    const values = fields.map((field) => exactInteger(row, field))
    const elapsed = previous === null ? 0 : row.timestamp - previous.at
    const deltas = previous === null || values.some((stored) => stored === null)
      ? null : values.map((stored, index) => stored! - previous!.values[index]!)
    previous = values.every((stored): stored is bigint => stored !== null) ? { at: row.timestamp, values } : null
    const reading = deltas === null || elapsed <= 0 || deltas.some((delta) => delta < 0n) ? null : output(deltas, elapsed)
    return { segmentId: row.segmentId, timestamp: row.timestamp, value: reading !== null && Number.isFinite(reading) ? reading : null }
  })
}

function cpuPoints(rows: readonly DataRow[], derive: NonNullable<MetricSpec["derive"]>, rates: boolean): readonly ChartPoint[] {
  const fields = ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"] as const
  const groups = new Map<string, { readonly rows: DataRow[]; readonly segmentId: string; readonly timestamp: number }>()
  for (const row of rows) {
    if (asNumber(value(row, "scope")) !== 0) continue
    const key = `${row.segmentId}:${row.timestamp}`
    const stored = groups.get(key) ?? { rows: [], segmentId: row.segmentId, timestamp: row.timestamp }
    stored.rows.push(row)
    groups.set(key, stored)
  }
  const points: ChartPoint[] = []
  let previous: readonly number[] | null = null
  for (const group of [...groups.values()].sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId))) {
    const online = new Set(group.rows.flatMap((row) => {
      const id = asNumber(value(row, "cpu_id"))
      return id === null || id < 0 ? [] : [id]
    })).size
    if (derive === "cpu_capacity") {
      points.push({ segmentId: group.segmentId, timestamp: group.timestamp, value: online > 0 ? online : null })
      continue
    }
    const aggregate = group.rows.find((row) => asNumber(value(row, "cpu_id")) === -1)
    if (aggregate === undefined) continue
    const counters = fields.map((field) => storedNumber(aggregate, field))
    if (!counters.every((counter): counter is number => counter !== null && counter !== undefined)) {
      previous = null
      points.push({ segmentId: group.segmentId, timestamp: group.timestamp, value: null })
      continue
    }
    const deltas = rates ? counters : previous === null ? null : counters.map((counter, index) => counter - previous![index]!)
    previous = counters
    points.push({ segmentId: group.segmentId, timestamp: group.timestamp, value: deltas === null ? null : cpuValue(deltas, derive, online) })
  }
  return points
}

function cpuValue(parts: readonly number[], derive: NonNullable<MetricSpec["derive"]>, online: number): number | null {
  if (parts.some((part) => part < 0 || !Number.isFinite(part))) return null
  const total = parts.reduce((sum, part) => sum + part, 0)
  if (total <= 0) return null
  const values: Readonly<Record<string, number>> = {
    cpu_user: parts[0]! + parts[1]!, cpu_system: parts[2]!, cpu_idle: parts[3]!, cpu_iowait: parts[4]!,
    cpu_irq: parts[5]! + parts[6]!, cpu_steal: parts[7]!,
  }
  if (derive === "cpu_used_cores") return online <= 0 ? null : online * (total - parts[3]! - parts[4]!) / total
  const selected = values[derive]
  return selected === undefined ? null : selected / total * 100
}

export function currentValue(data: HourData, spec: MetricSpec, cursor: number, locale: Locale): string {
  return currentPointValue(metricPoints(data, spec), cursor, locale, spec.unit)
}

function currentPointValue(points: readonly ChartPoint[], cursor: number, locale: Locale, unit: string): string {
  return metricValue(readingAt(points, cursor), locale, unit)
}

function storedNumber(row: DataRow, field: string): number | null | undefined {
  return Object.hasOwn(row.values, field) ? asNumber(value(row, field)) : undefined
}

function cumulativeRate(points: readonly ChartPoint[]): readonly ChartPoint[] {
  let previous: { readonly timestamp: number; readonly value: number } | null = null
  return points.slice().sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId)).map((point) => {
    if (point.value === null) {
      previous = null
      return point
    }
    const seconds = previous === null ? 0 : (point.timestamp - previous.timestamp) / 1_000_000
    const delta = previous === null ? -1 : point.value - previous.value
    previous = { timestamp: point.timestamp, value: point.value }
    return { ...point, value: seconds > 0 && delta >= 0 ? delta / seconds : null }
  })
}

function metricClass(spec: MetricSpec): RegistryColumn["class"] | null {
  if (spec.section === undefined || spec.field === undefined) return null
  // A counter the collector already turns into a rate reads as a gauge here:
  // the value is a per-second reading, not a climbing total.
  if ((spec.series ?? "").startsWith("os_")) return "gauge"
  return registry.flatMap((layout) => layout.logicalName === spec.section ? layout.columnMetadata ?? [] : [])
    .find(({ name }) => name === spec.field)?.class ?? null
}

function registryColumn(typeId: string, field: string): RegistryColumn | null {
  return registry.find((layout) => layout.typeId === typeId)?.columnMetadata?.find(({ name }) => name === field) ?? null
}

export function metricValue(value: Cell, locale: Locale, unit: string): string {
  if (unit === "%") return humanPercent(value, locale)
  if (unit === " cores") return humanCores(value, locale)
  if (unit === " KiB") return humanBytes(asNumber(value) === null ? null : (asNumber(value) ?? 0) * 1024, locale)
  if (unit === " B") return humanBytes(value, locale, "/s")
  return measure(value, locale, unit)
}

export function metricChartValue(value: number, locale: Locale, unit: string): string {
  if (unit === "%") return humanPercent(value, locale)
  if (unit === " cores") return humanCores(value, locale)
  // Memory charts read in bytes: an axis of «млн KiB» is a lie of scale.
  if (unit === " KiB") return humanBytes(value * 1024, locale)
  if (unit === " B") return humanBytes(value, locale, "/s")
  return measure(value, locale)
}

export function fallbackMetric(logicalName: string): string | null {
  if (logicalName === "os_cpu" || logicalName === "os_stat") return "cpu_used_cores"
  if (logicalName === "os_loadavg") return "load1"
  if (logicalName === "os_meminfo") return "mem_available"
  if (logicalName === "os_vmstat") return "oom_kill"
  return logicalName === "health" ? "health" : null
}

export function systemEntityRows(data: HourData, section: string, cursor: number): readonly DataRow[] {
  let rows = snapshot(sectionRows(data, section), cursor)
  const pathField = section === "os_cgroup_cpu" ? "cpu_path" : section === "os_cgroup_memory" ? "memory_path" : section === "os_cgroup_io" ? "io_path" : null
  const context = snapshot(sectionRows(data, "os_cgroup_context"), cursor)[0] ?? null
  if (pathField !== null) {
    const path = rawText(value(context, pathField))
    const scope = rawText(value(context, "scope"))
    if (path === null || scope === null || !/^(?:0|[1-9]\d*)$/.test(scope)) return []
    rows = rows.filter((row) => rawText(value(row, "cgroup_path")) === path && rawText(value(row, "scope")) === scope)
  }
  return rows.map((row) => decorateSystemRow(row, context))
}

function decorateSystemRow(row: DataRow, context: DataRow | null): DataRow {
  const values: Record<string, Cell> = { ...row.values }
  const pair = deviceId(row)
  if (pair !== null) values.device_id = pair
  if (row.logicalName === "os_diskstats") {
    values.read_bytes = scaled(row, "read_sectors", 512)
    values.write_bytes = scaled(row, "write_sectors", 512)
    values.read_latency_ms = ratio(row, "read_time_ms", "reads")
    values.write_latency_ms = ratio(row, "write_time_ms", "writes")
    values.device_busy = scaled(row, "io_time_ms", 0.1)
    values.average_queue = scaled(row, "io_weighted_time_ms", 0.001)
  } else if (row.logicalName === "os_cgroup_cpu") {
    values.cgroup_used_cores = scaled(row, "usage_usec", 0.000_001)
    values.cgroup_user_cores = scaled(row, "user_usec", 0.000_001)
    values.cgroup_system_cores = scaled(row, "system_usec", 0.000_001)
    const usage = asNumber(value(row, "usage_usec"))
    const user = asNumber(value(row, "user_usec"))
    const system = asNumber(value(row, "system_usec"))
    values.cgroup_other_cores = usage === null || user === null || system === null || usage < user + system ? null : (usage - user - system) / 1_000_000
    const quota = ratio(row, "quota_usec", "period_usec")
    const quotaCores = quota !== null && quota > 0 ? quota : null
    const cpuset = asNumber(value(context, "cpuset_cpus"))
    const cpusetCpus = cpuset !== null && cpuset > 0 ? cpuset : null
    values.cgroup_quota = quotaCores
    values.cpuset_cpus = cpusetCpus
    values.cgroup_capacity = effectiveCpuCapacity(
      asNumber(value(context, "effective_cpu_quota_usec")),
      asNumber(value(context, "effective_cpu_period_usec")),
      cpusetCpus,
    )
  } else if (row.logicalName === "os_cgroup_memory") {
    const effective = asNumber(value(context, "effective_memory_max"))
    values.effective_memory_max = effective !== null && effective >= 0 ? effective : null
    values.kernel_other = difference(row, "kernel", ["slab"]) ?? null
    values.memory_unclassified = difference(row, "current", ["anon", "file", "kernel"]) ?? null
  }
  return { ...row, values }
}

function deviceId(row: DataRow): string | null {
  const major = rawText(value(row, "major"))
  const minor = rawText(value(row, "minor"))
  return major === null || minor === null ? null : `${major}:${minor}`
}

function scaled(row: DataRow, field: string, scale: number): number | null {
  const stored = storedNumber(row, field)
  return stored === undefined || stored === null || !Number.isFinite(stored) ? null : stored * scale
}

function ratio(row: DataRow, numerator: string, denominator: string): number | null {
  const top = storedNumber(row, numerator)
  const bottom = storedNumber(row, denominator)
  return top === undefined || top === null || bottom === undefined || bottom === null || bottom <= 0 ? null : top / bottom
}

export function effectiveCpuCapacity(quotaUsec: number | null, periodUsec: number | null, cpuset: number | null): number | null {
  const cpusetCpus = cpuset !== null && Number.isFinite(cpuset) && cpuset > 0 ? cpuset : null
  if (quotaUsec === -1) return cpusetCpus
  if (quotaUsec === null || periodUsec === null || !Number.isFinite(quotaUsec) || !Number.isFinite(periodUsec)
    || quotaUsec <= 0 || periodUsec <= 0) return null
  const quotaCores = quotaUsec / periodUsec
  return cpusetCpus === null ? quotaCores : Math.min(quotaCores, cpusetCpus)
}

function rowKey(row: DataRow): string { return `${row.segmentId}:${row.typeId}:${row.ordinal}` }

function distinctTimes(points: readonly ChartPoint[]): number {
  return new Set(points.map((point) => point.timestamp)).size
}

function uniqueStrings(values: readonly string[]): readonly string[] {
  return [...new Set(values)]
}

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

function pressureMetric(id: string, group: HostSection, label: string, resource: number): MetricSpec {
  return { id, group, label: `${label}.label`, help: `${label}.help`, section: "os_psi", field: "some_avg10", resource, unit: "%" }
}

function point(source: Point): ChartPoint { return source }
function systemColumn(field: string, kind: NonNullable<EntityColumn["kind"]>, width: number, sticky = false): SystemEntityColumn {
  const obvious = new Set(["device", "device_id", "cgroup_path", "mount_point", "fstype", "source", "iface", "cpu_id", "model_name"])
  return { field, label: `system.field.${field}.label`, ...(obvious.has(field) ? {} : { help: `system.field.${field}.help` }), kind, width, sticky }
}
function text(field: string, width = 130, sticky = false): SystemEntityColumn { return systemColumn(field, "text", width, sticky) }
function virtualText(field: string, fields: readonly string[]): SystemEntityColumn { return { ...text(field, 90), chartable: false, historyFields: fields } }
function number(field: string, width = 126): SystemEntityColumn { return systemColumn(field, "number", width) }
function id(field: string, width = 110, sticky = false): SystemEntityColumn { return systemColumn(field, "id", width, sticky) }
function bytes(field: string, width = 145): SystemEntityColumn { return systemColumn(field, "bytes", width) }
function boolean(field: string, width = 130): SystemEntityColumn { return systemColumn(field, "boolean", width) }
function rateColumn(column: SystemEntityColumn): SystemEntityColumn { return { ...column, rate: true } }
function rateNumber(field: string, width = 126): SystemEntityColumn { return rateColumn(number(field, width)) }
function rateBytes(field: string, width = 145): SystemEntityColumn { return rateColumn(bytes(field, width)) }
function derived(field: string, kind: NonNullable<EntityColumn["kind"]>, fields: readonly string[], points: (rows: readonly DataRow[]) => readonly ChartPoint[]): SystemEntityColumn {
  return { ...systemColumn(field, kind, 145), historyFields: fields, points }
}
function derivedNumber(field: string, fields: readonly string[], points: (rows: readonly DataRow[]) => readonly ChartPoint[]): SystemEntityColumn { return derived(field, "number", fields, points) }
function derivedCores(field: string, fields: readonly string[], points: (rows: readonly DataRow[]) => readonly ChartPoint[]): SystemEntityColumn { return derived(field, "cores", fields, points) }
function derivedBytes(field: string, fields: readonly string[], points: (rows: readonly DataRow[]) => readonly ChartPoint[]): SystemEntityColumn { return derived(field, "bytes", fields, points) }
function derivedRateBytes(field: string, fields: readonly string[], points: (rows: readonly DataRow[]) => readonly ChartPoint[]): SystemEntityColumn { return rateColumn(derivedBytes(field, fields, points)) }
function derivedPercent(field: string, fields: readonly string[], points: (rows: readonly DataRow[]) => readonly ChartPoint[]): SystemEntityColumn { return derived(field, "percent", fields, points) }
function latency(field: string, operations: string, duration: string): SystemEntityColumn { return derived(field, "milliseconds", [operations, duration], (rows) => latencyPoints(rows, operations, duration)) }
function nonChartNumber(field: string, fields: readonly string[]): SystemEntityColumn { return { ...number(field), chartable: false, historyFields: fields } }
function nonChartBytes(field: string, fields: readonly string[]): SystemEntityColumn { return { ...bytes(field), chartable: false, historyFields: fields } }
function nonChartCores(field: string, fields: readonly string[]): SystemEntityColumn { return { ...systemColumn(field, "cores", 145), chartable: false, historyFields: fields } }
