import { registry } from "kronika:registry"
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { HostMode, HostSection } from "./address"
import { fieldNameForLocator, loadSeries, resolveLocator, type Cell, type DataRow, type Finding, type HourData, type Point, type SectionRequest } from "./api"
import { buildMetricSamples } from "./chart"
import { contextualRows, type EntityContext } from "./entity-context"
import { EntityTable, type EntityColumn } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import { useHistoryRequest } from "./history-request"
import { InspectorPortal } from "./inspector"
import { asNumber, humanBytes, humanCores, humanDuration, humanHertz, humanPercent, measure, rawText, shownMoment, snapshot, value, type Locale } from "./model"
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
    | "cpu_actual_frequency"
    | "cpu_scaling_frequency"
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
  if (spec.unit === " MHz") return "MHz"
  if (spec.unit === " B") return ""
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
  derivedMetric("cpu_actual_frequency", "cpu", "system.metric.cpu_actual_frequency", "cpu_actual_frequency", "cpu_actual_frequency", " MHz"),
  derivedMetric("cpu_scaling_frequency", "cpu", "system.metric.cpu_scaling_frequency", "cpu_scaling_frequency", "cpu_scaling_frequency", " MHz"),
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
// Token order series-1..7 plus the neutral total: the palette was validated
// for colour-vision separation in exactly this adjacency.
const BREAKDOWN_COLORS: readonly RecordedSeries["color"][] = ["cyan", "amber", "green", "violet", "red", "blue", "gray", "rose"]

// The mount history request fetches both sides of the pair at once.
const MOUNT_PAIR_COLUMN: SystemEntityColumn = { ...bytes("free_bytes"), historyFields: ["free_bytes", "total_bytes"] }
const MOUNT_INODE_PAIR_COLUMN: SystemEntityColumn = { ...number("available_inodes"), historyFields: ["available_inodes", "total_inodes"] }

const HOST_MODES: Partial<Readonly<Record<HostSection, readonly HostMode[]>>> = {
  cpu: ["history", "topology"],
  storage: ["io", "filesystems", "topology"],
  cgroups: ["cpu", "memory", "io", "tasks"],
}

// Resource tabs own one operator question at a time. Static topology never
// masquerades as temporal history, and cgroup accounting never appears under
// host block-device or memory totals.
function sectionEntities(section: HostSection, mode: HostMode | null): readonly string[] {
  if (section === "cpu") return []
  if (section === "storage") {
    if (mode === "filesystems") return ["os_mountinfo"]
    if (mode === "topology") return []
    return ["os_diskstats"]
  }
  if (section === "cgroups") {
    if (mode === "memory") return ["os_cgroup_memory"]
    if (mode === "io") return ["os_cgroup_io"]
    if (mode === "tasks") return ["os_cgroup_pids"]
    return ["os_cgroup_cpu"]
  }
  if (section === "network") return ["os_netdev"]
  return []
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
  cpu_actual_frequency: ["os_cpufreq", ["policy_id", "actual_source", "actual_frequency_hz", "online_cpus"]],
  cpu_scaling_frequency: ["os_cpufreq", ["policy_id", "scaling_cur_freq_hz", "online_cpus"]],
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
    section: "os_cgroup_pids", label: "system.entities.cgroup_tasks",
    columns: [text("cgroup_path", 240, true), physicalNumber("tasks_current", "current"), physicalNumber("tasks_max", "max")],
  },
  {
    section: "os_mountinfo", label: "system.entities.mounts",
    columns: [
      text("mount_point", 240, true), text("root", 160), text("source", 180), text("fstype", 120), virtualText("device_id", ["major", "minor"]),
      bytes("free_bytes"), derivedPercent("filesystem_available_percent", ["free_bytes", "total_bytes"], (rows) => gaugePercentPoints(rows, "free_bytes", "total_bytes")), bytes("total_bytes"),
      number("available_inodes"), derivedPercent("inode_available_percent", ["available_inodes", "total_inodes"], (rows) => gaugePercentPoints(rows, "available_inodes", "total_inodes")), number("total_inodes"), boolean("is_k8s_infra"),
    ],
  },
  {
    section: "os_netdev", label: "system.entities.network",
    columns: [text("iface", 150, true), rateBytes("rx_bytes"), rateBytes("tx_bytes"), rateNumber("rx_packets"), rateNumber("tx_packets"), rateNumber("rx_errs"), rateNumber("tx_errs"), rateNumber("rx_drop"), rateNumber("tx_drop"), number("speed_mbit"), id("duplex")],
  },
  {
    section: "os_topology", label: "system.entities.topology",
    columns: [id("cpu_id", 90, true), id("socket_id"), id("core_id"), id("numa_node"), text("model_name", 300), number("mhz_max")]
      .map((column) => ({ ...column, chartable: false })),
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
  for (const section of ["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io", "os_cgroup_pids"]) need(section, ["cgroup_path", "scope"])
  need("os_cgroup_context", [
    "cpu_path", "memory_path", "io_path", "cpuset_cpus", "effective_cpu_quota_usec",
    "effective_cpu_period_usec", "effective_memory_max", "cgroup_version", "scope",
  ])
  need("instance_metadata", ["environment"])
  need("os_cpufreq_policy", [
    "policy_id", "related_cpus", "scaling_driver", "actual_source",
    "cpuinfo_min_freq_hz", "cpuinfo_max_freq_hz", "scope",
  ])
  need("os_block_topology", ["major", "minor", "parent_major", "parent_minor", "scope"])
  return [...wanted].map(([section, fields]) => ({ section, fields: [...fields] }))
}

const CGROUP_PATH_FIELDS = {
  os_cgroup_cpu: "cpu_path",
  os_cgroup_memory: "memory_path",
  os_cgroup_io: "io_path",
  os_cgroup_pids: "pids_path",
} as const
const ALL_SYSTEM_REQUESTS = systemRequests()
const CGROUP_SECTIONS = new Set(["os_cgroup_context", ...Object.keys(CGROUP_PATH_FIELDS)])

export const CGROUP_SNAPSHOT_REQUESTS = ALL_SYSTEM_REQUESTS.filter(({ section }) => CGROUP_SECTIONS.has(section))
export const SYSTEM_REQUESTS = ALL_SYSTEM_REQUESTS.filter(({ section }) => !CGROUP_SECTIONS.has(section))

export function cgroupSnapshotPlan(
  segmentId: string,
  cursor: number,
  data: Pick<HourData, "sections">,
  requests: readonly SectionRequest[] = CGROUP_SNAPSHOT_REQUESTS,
): CgroupSnapshotPlan {
  const environment = recordedEnvironment(data, cursor)
  const key = JSON.stringify([segmentId, cursor, environment])
  const loads = environment === "container"
    ? requests.map((request) => ({ request, filters: {} }))
    : []
  return { key, loads }
}

export function recordedEnvironment(data: Pick<HourData, "sections">, cursor: number): "machine" | "container" | null {
  const row = snapshot(data.sections.instance_metadata ?? [], cursor)[0]
  const environment = asNumber(value(row ?? null, "environment"))
  return environment === 0 ? "machine" : environment === 1 ? "container" : null
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
  metric,
  mode,
  navigationTimestamps,
  onCursor,
  onContextClear,
  onFinding,
  onOpenChart,
  onOpenDetail,
  onMetric,
  onMode,
  onSelectedLane,
  onSelectedKey,
  selectedLane,
  selectedKey,
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
  readonly metric: string | null
  readonly mode: HostMode | null
  readonly navigationTimestamps: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly onContextClear: () => void
  readonly onFinding: (finding: Finding) => void
  readonly onOpenChart: () => void
  readonly onOpenDetail: () => void
  readonly onMetric: (metric: string | null) => void
  readonly onMode: (mode: HostMode | null) => void
  readonly onSelectedLane: (lane: string) => void
  readonly onSelectedKey: (key: string | null) => void
  readonly selectedLane: string
  readonly selectedKey: string | null
  readonly tablesLoading?: boolean | undefined
  readonly t: Translate
}) {
  const available = useMemo(() => SYSTEM_METRICS.map((spec) => ({ points: metricPoints(data, spec), spec }))
    .filter(({ points, spec }) => points.some((point) => point.value !== null && Number.isFinite(point.value))
      || (spec.id === "cpu_actual_frequency" && sectionRows(data, "os_cpufreq").some((row) => {
        const frequency = storedNumber(row, "actual_frequency_hz")
        return frequency !== undefined && frequency !== null && Number.isFinite(frequency)
      }))), [data])
  const referenceMode = mode === "topology"
  const sectionMetrics = useMemo(() => referenceMode ? [] : available.filter(({ spec }) => spec.group === section), [available, referenceMode, section])
  const [dismissedOverview, setDismissedOverview] = useState(false)
  const autoMetric = useRef<string | null>(null)
  // Overview opens on the first metric that has a factual sample. A metric in
  // the address is an explicit operator choice and is never replaced here.
  useEffect(() => {
    const first = available[0]
    if (section !== "overview" || dismissedOverview || first === undefined) return
    if (metric === null) {
      autoMetric.current = first.spec.id
      onMetric(first.spec.id)
      return
    }
    if (autoMetric.current !== metric) {
      autoMetric.current = null
      return
    }
    if (first.spec.id !== metric) {
      autoMetric.current = first.spec.id
      onMetric(first.spec.id)
    }
  }, [available, dismissedOverview, metric, onMetric, section])
  useEffect(() => {
    if (section !== "overview") setDismissedOverview(false)
  }, [section])
  const openMetric = (id: string) => {
    autoMetric.current = null
    setDismissedOverview(false)
    onSelectedKey(null)
    onMetric(id)
    onOpenDetail()
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
      autoMetric.current = null
      onMetric(match.spec.id)
    }
  }, [available, data, focus, onMetric])
  const selectedSpec = metric === null ? undefined : SYSTEM_METRICS.find((spec) => spec.id === metric)
  // The spec comes from the static catalog so the dock keeps its frame while
  // its section is mid-reload; the points honestly empty out for that window.
  const selectedMetric = selectedSpec === undefined ? undefined : {
    points: available.find(({ spec }) => spec.id === selectedSpec.id)?.points ?? [],
    spec: selectedSpec,
  }
  const selectedResource = selectedMetric === undefined ? null : metricResource(selectedMetric.spec)
  const dockShown = selectedMetric !== undefined && selectedKey === null
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
  const modes = HOST_MODES[section] ?? []
  const topologyRows = section === "cpu" && mode === "topology" ? systemEntityRows(data, "os_topology", cursor) : []
  const policyRows = section === "cpu" && mode === "topology" ? systemEntityRows(data, "os_cpufreq_policy", cursor) : []
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={onCursor} onFinding={onFinding} onOpenChart={onOpenChart} onSelectedLane={onSelectedLane} primaryLane={selectedMetric === undefined ? "health" : metricLane(selectedMetric.spec)} selectedLane={selectedLane} shownAt={shownAt} t={t} />
    {modes.length > 0 && <div aria-label={t(`section.${section}`)} className="lensbar !mt-0 border-t-0" data-testid={`host-${section}-modes`} role="group">
      <div className="lens-tabs">
        {modes.map((choice) => <button aria-pressed={mode === choice} key={choice} onClick={() => onMode(choice)} type="button">{t(`host.mode.${choice}`)}</button>)}
      </div>
    </div>}
    <div className={`${modes.length === 0 ? "mt-0" : "mt-2"} system-main min-w-0 [&>.use-table]:mt-0 [&>.metric-groups]:mt-2 [&>.table-empty]:mt-2`}>
        {section === "overview" && <UseTable canOpen={(resource) => resourceSelection(available, resource) !== null} cursor={cursor} lanePoints={data.lanePoints} locale={locale} onSelect={(resource) => {
          const target = resourceSelection(available, resource)
          if (target !== null) openMetric(target)
        }} selected={dockShown ? selectedResource : null} t={t} />}
      {dockShown && selectedMetric !== undefined && <InspectorPortal autoOpen={false} identity={`system:${selectedMetric.spec.id}`} onClose={() => { setDismissedOverview(true); onMetric(null) }} title={t(selectedMetric.spec.label)}><SystemDock
        chart={breakdown.length === 0
          ? <SeriesChart cursor={cursor} empty={t("history.empty")} format={(reading, place) => metricChartValue(reading, place, selectedMetric.spec.unit)} helpKey={selectedMetric.spec.help} hour={hour} labelKey={selectedMetric.spec.label} locale={locale} onCursor={onCursor} points={selectedPoints} scale={selectedMetric.spec.unit === "%" ? "percent" : "nonnegative"} second={secondPoints} secondHelpKey={secondLane === null ? undefined : "system.metric.network_tx.help"} secondLabelKey={secondLane === null ? undefined : "system.metric.network_tx.label"} stats status={needsHistory ? loadedHistory.status : "ready"} t={t} tickFormat={selectedMetric.spec.unit === " B" ? (reading, place) => humanBytes(reading, place, "/s") : undefined} unit={metricChartUnit(selectedMetric.spec, locale)} />
          : <div className="series-chart"><UPlotChart cursor={cursor} hour={hour} isolate={{ anchor: selectedMetric.spec.id }} locale={locale} onCursor={onCursor} reading={currentPointValue(selectedPoints, cursor, locale, selectedMetric.spec.unit)} series={breakdown} stats status={!needsHistory || loadedHistory.status === "ready" ? undefined : <p className={`series-status series-status-${loadedHistory.status}`} role={loadedHistory.status === "error" ? "alert" : "status"}>{t(`history.${loadedHistory.status}`)}</p>} t={t} testId={`system-${selectedMetric.spec.group}-composition`} /></div>}
        label={`section.${selectedMetric.spec.group}`}
        metrics={dockMeta.chips}
        onSelect={openMetric}
        selected={dockMeta.chartChip(selectedMetric.spec.id)}
        t={t}
      /></InspectorPortal>}
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

    {section === "cpu" && mode === "topology" && <CpuTopologyReference locale={locale} policies={policyRows} rows={topologyRows} t={t} />}
    {section === "storage" && mode === "topology" && <StorageTopologyReference
      devices={systemEntityRows(data, "os_diskstats", cursor)}
      edges={systemEntityRows(data, "os_block_topology", cursor)}
      mounts={systemEntityRows(data, "os_mountinfo", cursor)}
      t={t}
    />}

    <section className="entity-panels mt-2 grid grid-cols-1 content-start gap-2">
      {SYSTEM_ENTITIES.filter((entity) => sectionEntities(section, mode).includes(entity.section)).map((entity) => {
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
          onMetric={onMetric}
          onSelectedKey={onSelectedKey}
          rows={rows}
          section={entity.section}
          selectedField={metric}
          selectedKey={selectedKey}
          tablesLoading={tablesLoading}
          t={t}
        />
      })}
    </section>
  </>
}

function CpuTopologyReference({ locale, policies, rows, t }: { readonly locale: Locale; readonly policies: readonly DataRow[]; readonly rows: readonly DataRow[]; readonly t: Translate }) {
  if (rows.length === 0 && policies.length === 0) return <p className="table-empty mt-2">{t("status.no_data")}</p>
  const policyByCpu = new Map<number, string>()
  for (const policy of policies) {
    const id = rawText(value(policy, "policy_id")) ?? "—"
    for (const cpu of parseCpuList(rawText(value(policy, "related_cpus")))) policyByCpu.set(cpu, id)
  }
  const groups = new Map<string, DataRow[]>()
  for (const row of rows) {
    const key = [rawText(value(row, "numa_node")) ?? "—", rawText(value(row, "socket_id")) ?? "—", rawText(value(row, "core_id")) ?? "—"].join(":")
    const stored = groups.get(key) ?? []
    stored.push(row)
    groups.set(key, stored)
  }
  return <section className="panel mt-2" data-testid="cpu-topology-reference">
    <h2 className="panel-head">{t("system.entities.topology")}</h2>
    <div className="grid grid-cols-[repeat(auto-fit,minmax(220px,1fr))]">
      {[...groups.entries()].map(([key, members]) => <div className="min-w-0 border-b border-r border-line px-2 py-1.5 text-sm" key={key}>
        <strong className="block font-medium text-fg2">{t("system.topology.group", { group: key })}</strong>
        <span className="text-fg3">{t("system.topology.cpus", { cpus: members.map((row) => rawText(value(row, "cpu_id")) ?? "—").join(", ") })}</span>
        <span className="block text-fg3">{t("system.topology.policies", { policies: [...new Set(members.flatMap((row) => {
          const cpu = asNumber(value(row, "cpu_id"))
          return cpu === null ? [] : [policyByCpu.get(cpu) ?? "—"]
        }))].join(", ") })}</span>
        <span className="block overflow-hidden text-ellipsis whitespace-nowrap text-fg4">{rawText(value(members[0] ?? null, "model_name")) ?? "—"}</span>
      </div>)}
    </div>
    {policies.length > 0 && <div className="border-t border-line2" data-testid="cpufreq-policy-reference">
      {policies.map((policy) => <div className="grid min-h-[34px] grid-cols-[90px_minmax(120px,1fr)_minmax(130px,1fr)_minmax(150px,1.2fr)] items-center border-b border-line px-2 py-1.5 text-sm last:border-b-0 max-[700px]:grid-cols-2 [&>*]:pr-2" key={rawText(value(policy, "policy_id")) ?? policy.ordinal}>
        <strong className="font-medium text-fg2">{t("system.topology.policy", { policy: rawText(value(policy, "policy_id")) ?? "—" })}</strong>
        <span className="text-fg3">{rawText(value(policy, "related_cpus")) ?? "—"}</span>
        <span className="text-fg3">{rawText(value(policy, "scaling_driver")) ?? "—"}</span>
        <span className="text-fg3">{rawText(value(policy, "actual_source")) ?? t("common.unavailable")} · {humanHertz(value(policy, "cpuinfo_min_freq_hz"), locale)}–{humanHertz(value(policy, "cpuinfo_max_freq_hz"), locale)}</span>
      </div>)}
    </div>}
  </section>
}

function parseCpuList(text: string | null): readonly number[] {
  if (text === null) return []
  const cpus = new Set<number>()
  for (const token of text.split(/[\s,]+/)) {
    if (token === "") continue
    const [firstText, lastText = firstText] = token.split("-")
    const first = Number(firstText)
    const last = Number(lastText)
    if (!Number.isSafeInteger(first) || !Number.isSafeInteger(last) || first < 0 || last < first || last - first > 4096) return []
    for (let cpu = first; cpu <= last; cpu += 1) {
      cpus.add(cpu)
      if (cpus.size > 4096) return []
    }
  }
  return [...cpus]
}

export function storageTopologyEntries(devices: readonly DataRow[], edges: readonly DataRow[], mounts: readonly DataRow[]): readonly {
  readonly associations: readonly string[]
  readonly id: string
  readonly name: string
  readonly parent: string | null
}[] {
  const names = new Map(devices.flatMap((row) => {
    const id = deviceId(row)
    return id === null ? [] : [[id, rawText(value(row, "device")) ?? id] as const]
  }))
  const mappings = new Map<string, DataRow[]>()
  const parents = new Map(edges.flatMap((row) => {
    const child = deviceId(row)
    const parentMajor = rawText(value(row, "parent_major"))
    const parentMinor = rawText(value(row, "parent_minor"))
    return child === null || parentMajor === null || parentMinor === null ? [] : [[child, `${parentMajor}:${parentMinor}`] as const]
  }))
  for (const row of mounts) {
    const id = deviceId(row) ?? "—"
    const stored = mappings.get(id) ?? []
    stored.push(row)
    mappings.set(id, stored)
  }
  return [...new Set([...names.keys(), ...mappings.keys(), ...parents.keys()])].sort().map((id) => ({
    associations: (mappings.get(id) ?? []).map((row) => `${rawText(value(row, "source")) ?? "—"} ${rawText(value(row, "root")) ?? "—"} → ${rawText(value(row, "mount_point")) ?? "—"}`),
    id,
    name: names.get(id) ?? id,
    parent: parents.get(id) ?? null,
  }))
}

function StorageTopologyReference({ devices, edges, mounts, t }: { readonly devices: readonly DataRow[]; readonly edges: readonly DataRow[]; readonly mounts: readonly DataRow[]; readonly t: Translate }) {
  const entries = storageTopologyEntries(devices, edges, mounts)
  if (entries.length === 0) return <p className="table-empty mt-2">{t("status.no_data")}</p>
  return <section className="panel mt-2" data-testid="storage-topology-reference">
    <h2 className="panel-head">{t("host.mode.topology")}</h2>
    <p className="m-0 border-b border-line px-2 py-1.5 text-sm text-fg3">{t("system.storage.topology_scope")}</p>
    {entries.map((entry) => <div className="grid min-h-[34px] grid-cols-[minmax(110px,180px)_minmax(0,1fr)] items-start border-b border-line px-2 py-1.5 text-sm last:border-b-0 [&>*+*]:ml-2 max-[520px]:grid-cols-1 max-[520px]:[&>*+*]:ml-0 max-[520px]:[&>*+*]:mt-2" key={entry.id}>
      <strong className="font-medium tabular-nums text-fg2">{entry.name} · {entry.id}{entry.parent === null ? "" : ` → ${entry.parent}`}</strong>
      <span className="min-w-0 text-fg3">{entry.associations.join(" · ") || t("system.storage.topology_opaque")}</span>
    </div>)}
  </section>
}

// The dock is the System counterpart of the PostgreSQL detail panel: a click
// on a Use row or a metric chip opens it on the resource's group, and the chart
// lives only inside it — nothing on the page silently swaps its content.
function SystemDock({
  chart,
  label,
  metrics,
  onSelect,
  selected,
  t,
}: {
  readonly chart: ReactNode
  readonly label: string
  readonly metrics: readonly MetricSpec[]
  readonly onSelect: (id: string) => void
  readonly selected: string
  readonly t: Translate
}) {
  return <aside aria-label={t(label)} className="pg-detail system-dock" data-testid="system-dock">
    <header className="pg-detail-head">
      <div><span>{t("system.history")}</span><h2>{t(label)}</h2></div>
    </header>
    <section className="process-history mt-2.5 grid min-w-0 gap-[7px] border-t border-line3 pt-[7px]">
      <div aria-label={t(label)} className="dock-tabs history-selector flex max-w-full gap-[5px] overflow-x-auto p-px pb-[3px] [scrollbar-width:thin]" role="group">
        {metrics.map((spec) => <button aria-pressed={spec.id === selected} data-testid={`system-dock-metric-${spec.id}`} key={spec.id} onClick={() => onSelect(spec.id)} type="button">{t(spec.label)}</button>)}
      </div>
      {chart}
    </section>
  </aside>
}

// `free_bytes` stores statvfs.f_bavail: capacity available to an unprivileged
// writer. It is not f_bfree, so Total − Available must never be called Used.
// Chart the two exact stored facts without inventing allocated/reserved space.
export function mountPairSeries(rows: readonly DataRow[], t: Translate, kind: "bytes" | "inodes" = "bytes"): readonly RecordedSeries[] {
  const totalField = kind === "bytes" ? "total_bytes" : "total_inodes"
  const availableField = kind === "bytes" ? "free_bytes" : "available_inodes"
  const total = buildMetricSamples(rows, (row) => storedNumber(row, totalField))
  const available = buildMetricSamples(rows, (row) => storedNumber(row, availableField))
  if (!available.some((point) => point.value !== null) && !total.some((point) => point.value !== null)) return []
  const format = (reading: number, place: Locale) => kind === "bytes" ? humanBytes(reading, place) : measure(reading, place)
  const availableMeta = kind === "bytes"
    ? { helpKey: "system.field.available_bytes.help", id: "available_bytes", labelKey: "system.field.available_bytes.label" }
    : { helpKey: "system.field.available_inodes.help", id: "available_inodes", labelKey: "system.field.available_inodes.label" }
  const totalKey = kind === "bytes" ? "system.field.total_bytes" : "system.field.total_inodes"
  return [
    { color: "cyan", helpKey: availableMeta.helpKey, id: availableMeta.id, label: t(availableMeta.labelKey), labelKey: availableMeta.labelKey, points: available, scale: "nonnegative", tick: format, unit: kind === "bytes" ? "B" : "count", value: format },
    { color: "gray", helpKey: `${totalKey}.help`, id: totalField, label: t(`${totalKey}.label`), labelKey: `${totalKey}.label`, points: total, scale: "nonnegative", tick: format, unit: kind === "bytes" ? "B" : "count", value: format },
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
  onMetric,
  onSelectedKey,
  rows,
  section,
  selectedField,
  selectedKey,
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
  readonly onMetric: (field: string | null) => void
  readonly onSelectedKey: (key: string | null) => void
  readonly rows: readonly DataRow[]
  readonly tablesLoading?: boolean | undefined
  readonly section: string
  readonly selectedField: string | null
  readonly selectedKey: string | null
  readonly t: Translate
}) {
  const metricColumns = useMemo(() => chartableEntityColumns(columns), [columns])
  const selectedRow = selectedKey === null ? null : rows.find((row) => entityRowKey(row) === selectedKey) ?? null
  const availableColumns = useMemo(() => selectedRow === null
    ? []
    : metricColumns.filter((column) => Object.hasOwn(selectedRow.values, physicalField(column, selectedRow.typeId))), [metricColumns, selectedRow])
  useEffect(() => {
    if (selectedKey !== null && selectedRow === null && !tablesLoading) onSelectedKey(null)
  }, [onSelectedKey, selectedKey, selectedRow, tablesLoading])
  useEffect(() => {
    if (selectedRow === null) return
    if (availableColumns.some((column) => column.field === selectedField)) return
    onMetric(availableColumns[0]?.field ?? null)
  }, [availableColumns, onMetric, selectedField, selectedRow])
  const selectedColumn = availableColumns.find((column) => column.field === selectedField) ?? availableColumns[0]
  const mountPair = section === "os_mountinfo" && selectedRow !== null
    && Object.hasOwn(selectedRow.values, "free_bytes") && Object.hasOwn(selectedRow.values, "total_bytes")
  const mountPairKind = mountPair && ["available_inodes", "inode_available_percent", "total_inodes"].includes(selectedColumn?.field ?? "") ? "inodes" : "bytes"
  const requestColumn = mountPair ? (mountPairKind === "inodes" ? MOUNT_INODE_PAIR_COLUMN : MOUNT_PAIR_COLUMN) : selectedColumn
  const historyRequest = selectedRow === null || requestColumn === undefined ? null : entityHistoryRequest(selectedRow, requestColumn)
  const historyKey = historyRequest === null ? null : `${hour}:${historyRequest.key}`
  const requestFields = historyRequest === null ? "[]" : JSON.stringify(historyRequest.fields)
  const requestWhere = historyRequest === null ? "{}" : JSON.stringify(historyRequest.where)
  const requestSection = historyRequest?.section ?? ""
  const requestTypeId = historyRequest?.typeId
  const visibleHistoryKey = historyKey
  const history = useHistoryRequest(visibleHistoryKey, historyRevision,
    visibleHistoryKey === null || requestSection === "" || requestTypeId === undefined ? null : (signal) => {
    const fields = JSON.parse(requestFields) as readonly string[]
    const where = JSON.parse(requestWhere) as Readonly<Record<string, string>>
    return loadSeries(hour, requestSection, where, fields, signal, requestTypeId)
  })
  const chartRows = history.value?.length ? history.value : selectedRow === null ? [] : [selectedRow]
  const chartPoints = useMemo(() => selectedColumn === undefined ? [] : entityMetricPoints(chartRows, selectedColumn), [chartRows, selectedColumn])
  const pairSeries = useMemo(() => mountPair ? mountPairSeries(chartRows, t, mountPairKind) : null, [chartRows, mountPair, mountPairKind, t])
  const chartMetadata = selectedRow === null || selectedColumn === undefined || selectedColumn.historyFields !== undefined
    ? null : registryColumn(selectedRow.typeId, physicalField(selectedColumn, selectedRow.typeId))
  return <section className="entity-panel panel min-w-0" data-testid={`system-panel-${section}`}>
    <h2 className="panel-head"><span>{label}</span></h2>
    <div className="contents">
    <EntityTable
      columns={columns}
      contentSized
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
        onSelectedKey(key)
        if (key !== selectedKey) {
          const first = metricColumns.find((column) => Object.hasOwn(row.values, physicalField(column, row.typeId)))
          onMetric(first?.field ?? null)
        }
      } })}
      rowKey={entityRowKey}
      rows={rows}
      selectedKey={selectedKey}
      t={t}
      testId={`system-${section}`}
    />
    </div>
    {selectedRow !== null && (mountPair || selectedColumn !== undefined) && <InspectorPortal identity={`system:${section}:${entityRowKey(selectedRow)}`} onClose={() => { onSelectedKey(null); onMetric(null) }} title={`${label} · ${entityRowKey(selectedRow)}`}><section className="system-entity-history min-w-0" data-testid={`system-${section}-history`}>
      <header className="flex items-start px-[7px] pt-1.5">
        {mountPair
          ? <div className="system-history-selector flex max-w-full overflow-x-auto pb-[3px] [scrollbar-width:thin] [&>button+button]:ml-1 [&>button]:min-h-[27px] [&>button]:flex-none [&>button]:cursor-pointer [&>button]:border [&>button]:border-line3 [&>button]:bg-s2 [&>button]:px-[7px] [&>button]:py-1 [&>button]:text-xs [&>button]:text-fg2 [&>button[aria-pressed=true]]:border-accent [&>button[aria-pressed=true]]:bg-accent-soft [&>button[aria-pressed=true]]:text-fg" role="group">
              <button aria-pressed={mountPairKind === "bytes"} onClick={() => onMetric("free_bytes")} type="button">{t("system.field.free_bytes.label")}</button>
              <button aria-pressed={mountPairKind === "inodes"} onClick={() => onMetric("available_inodes")} type="button">{t("system.field.available_inodes.label")}</button>
            </div>
          : <div className="system-history-selector flex max-w-full gap-1 overflow-x-auto pb-[3px] [scrollbar-width:thin] [&>button]:min-h-[27px] [&>button]:flex-none [&>button]:cursor-pointer [&>button]:border [&>button]:border-line3 [&>button]:bg-s2 [&>button]:px-[7px] [&>button]:py-1 [&>button]:text-xs [&>button]:text-fg2 [&>button[aria-pressed=true]]:border-accent [&>button[aria-pressed=true]]:bg-accent-soft [&>button[aria-pressed=true]]:text-fg" role="group">
            {availableColumns.map((column) => <button aria-pressed={column.field === selectedColumn?.field} key={column.field} onClick={() => onMetric(column.field)} type="button">{t(column.label)}</button>)}
          </div>}
      </header>
      {mountPair
        ? pairSeries === null || (pairSeries.length === 0 && history.status === "ready")
          ? <p className="table-empty">{t("status.no_data")}</p>
          : <div className="series-chart"><UPlotChart
              cursor={cursor}
              hour={hour}
              locale={locale}
              onCursor={onCursor}
              reading={(() => { const stored = readingAt(pairSeries?.[0]?.points ?? [], cursor); return stored === null ? "—" : mountPairKind === "bytes" ? humanBytes(stored, locale) : measure(stored, locale) })()}
              series={pairSeries ?? []}
              status={history.status === "ready" ? undefined : <p className={`series-status series-status-${history.status}`} role={history.status === "error" ? "alert" : "status"}>{t(`history.${history.status}`)}</p>}
              t={t}
              testId={`system-${section}-pair`}
            /></div>
        : selectedColumn !== undefined && <SeriesChart
            cursor={cursor}
            durationAxis={selectedColumn.kind === "milliseconds" || selectedColumn.kind === "duration"}
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
    </section></InspectorPortal>}
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
  if (column.kind === "bytes" || column.kind === "kib") return ""
  if (column.kind === "milliseconds" || column.kind === "duration" || column.kind === "microseconds") return perSecond
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
  if (column.kind === "milliseconds" || column.kind === "duration") return humanDuration(reading, locale, "milliseconds", suffix)
  if (column.kind === "microseconds") return humanDuration(reading, locale, "microseconds", suffix)
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
  if (selectedId === "cpu_actual_frequency") return frequencyBreakdownSeries(rows, "actual_frequency_hz", t)
  if (selectedId === "cpu_scaling_frequency") return frequencyBreakdownSeries(rows, "scaling_cur_freq_hz", t)
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
  if (derive === "cpu_actual_frequency") return frequencyPoints(rows, "actual_frequency_hz")
  if (derive === "cpu_scaling_frequency") return frequencyPoints(rows, "scaling_cur_freq_hz")
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

function frequencyPoints(rows: readonly DataRow[], field: string): readonly ChartPoint[] {
  return aggregateRows(rows, (sampleRows) => {
    let weighted = 0
    let online = 0
    let actualSource: string | null = null
    for (const row of sampleRows) {
      const frequency = storedNumber(row, field)
      const count = storedNumber(row, "online_cpus")
      if (frequency === undefined || count === undefined) return undefined
      if (frequency === null || count === null || count < 0) return null
      if (field === "actual_frequency_hz") {
        const source = rawText(value(row, "actual_source"))
        if (source === null) return null
        if (actualSource !== null && actualSource !== source) return null
        actualSource = source
      }
      weighted += frequency * count
      online += count
    }
    return online > 0 ? weighted / online / 1_000_000 : null
  })
}

function frequencyBreakdownSeries(rows: readonly DataRow[], field: string, t: Translate): readonly RecordedSeries[] {
  const metricKey = field === "actual_frequency_hz" ? "system.metric.cpu_actual_frequency" : "system.metric.cpu_scaling_frequency"
  const policies = new Map<string, DataRow[]>()
  for (const row of rows) {
    const id = rawText(value(row, "policy_id"))
    if (id === null) continue
    const stored = policies.get(id) ?? []
    stored.push(row)
    policies.set(id, stored)
  }
  return [...policies.entries()].sort(([left], [right]) => left.localeCompare(right, undefined, { numeric: true })).map(([id, policyRows], index) => ({
    color: BREAKDOWN_COLORS[index % BREAKDOWN_COLORS.length]!,
    helpKey: `${metricKey}.help`,
    id: `cpufreq_policy_${id}`,
    label: t("system.topology.policy", { policy: id }),
    labelKey: `${metricKey}.label`,
    points: buildMetricSamples(policyRows, (row) => {
      const hertz = storedNumber(row, field)
      return hertz === undefined || hertz === null ? hertz : hertz / 1_000_000
    }),
    scale: "nonnegative" as const,
    tick: (reading: number, place: Locale) => measure(reading, place),
    unit: "MHz",
    value: (reading: number, place: Locale) => measure(reading, place, " MHz"),
  }))
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

function gaugePercentPoints(rows: readonly DataRow[], numerator: string, denominator: string): readonly ChartPoint[] {
  return buildMetricSamples(rows, (row) => {
    const result = ratio(row, numerator, denominator)
    return result === null ? null : result * 100
  })
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
  const context = snapshot(sectionRows(data, "os_cgroup_context"), cursor)[0] ?? null
  const pathField = section === "os_cgroup_cpu" ? "cpu_path" : section === "os_cgroup_memory" ? "memory_path" : section === "os_cgroup_io" ? "io_path" : null
  return rows.map((row) => {
    const collectorContext = context !== null && pathField !== null
      && rawText(value(row, "cgroup_path")) === rawText(value(context, pathField))
      && rawText(value(row, "scope")) === rawText(value(context, "scope"))
      ? context
      : null
    return decorateSystemRow(row, collectorContext)
  })
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
  } else if (row.logicalName === "os_mountinfo") {
    const availableBytes = ratio(row, "free_bytes", "total_bytes")
    const availableInodes = ratio(row, "available_inodes", "total_inodes")
    values.filesystem_available_percent = availableBytes === null ? null : availableBytes * 100
    values.inode_available_percent = availableInodes === null ? null : availableInodes * 100
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
  const obvious = new Set(["device", "device_id", "cgroup_path", "mount_point", "root", "fstype", "source", "iface", "cpu_id", "model_name"])
  return { field, label: `system.field.${field}.label`, ...(obvious.has(field) ? {} : { help: `system.field.${field}.help` }), kind, width, sticky }
}
function text(field: string, width = 130, sticky = false): SystemEntityColumn { return systemColumn(field, "text", width, sticky) }
function virtualText(field: string, fields: readonly string[]): SystemEntityColumn { return { ...text(field, 90), chartable: false, historyFields: fields } }
function number(field: string, width = 126): SystemEntityColumn { return systemColumn(field, "number", width) }
function physicalNumber(field: string, physicalField: string, width = 126): SystemEntityColumn { return { ...number(field, width), physicalField } }
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
