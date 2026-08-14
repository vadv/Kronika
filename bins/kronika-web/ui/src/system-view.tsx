import { registry } from "kronika:registry"
import { useEffect, useMemo, useState } from "react"

import { acceptResponse, fieldNameForLocator, loadSeries, resolveLocator, type Cell, type DataRow, type Finding, type HourData, type Point, type SectionRequest } from "./api"
import { buildMetricSamples } from "./chart"
import { contextualRows, type EntityContext } from "./entity-context"
import { EntityTable, type EntityColumn } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import { asNumber, humanBytes, humanPercent, measure, rawText, shownMoment, snapshot, value, type Locale } from "./model"
import { readingAt, SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"
import { UseTable } from "./use-table"

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

interface RegistryColumn {
  readonly class: "cumulative" | "gauge" | "label" | "timestamp"
  readonly name: string
  readonly type: string
  readonly unit: string | null
}

export function metricChartUnit(spec: MetricSpec, locale: Locale): string {
  if (spec.unit === "%") return "%"
  if (spec.unit === " KiB") return "KiB"
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

export const SYSTEM_METRICS: readonly MetricSpec[] = [
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
  derivedMetric("device_count", "storage", "system.metric.device_count", "os_device_count", "device_count", ""),
  derivedMetric("device_active_io", "storage", "system.metric.device_active_io", "os_device_active_io", "device_active_io", ""),
  derivedMetric("filesystem_count", "storage", "system.metric.filesystem_count", "os_filesystem_count", "filesystem_count", ""),
  derivedMetric("interface_count", "network", "system.metric.interface_count", "os_interface_count", "interface_count", ""),
  derivedMetric("network_rx", "network", "system.metric.network_rx", "os_network_rx", "network_rx", " B"),
  derivedMetric("network_tx", "network", "system.metric.network_tx", "os_network_tx", "network_tx", " B"),
  derivedMetric("network_errors", "network", "system.metric.network_errors", "os_network_errors", "network_errors", ""),
  derivedMetric("network_drops", "network", "system.metric.network_drops", "os_network_drops", "network_drops", ""),
]

const GROUPS: readonly { readonly id: MetricSpec["group"]; readonly label: string }[] = [
  { id: "cpu", label: "system.group.cpu" },
  { id: "load", label: "system.group.load" },
  { id: "memory", label: "system.group.memory" },
  { id: "pressure", label: "system.group.pressure" },
  { id: "storage", label: "system.group.storage" },
  { id: "network", label: "system.group.network" },
]

const DERIVE_INPUTS: Readonly<Record<NonNullable<MetricSpec["derive"]>, readonly [string, readonly string[]]>> = {
  cpu_busy: ["os_cpu", ["cpu_id", "scope", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"]],
  mem_available_percent: ["os_meminfo", ["mem_total", "mem_available"]],
  filesystem_free_min: ["os_mountinfo", ["total_bytes", "free_bytes"]],
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
  ["cpu", "memory", "pressure"],
  ["load", "storage", "network"],
]

export const SYSTEM_ENTITIES: readonly {
  readonly section: string
  readonly label: string
  readonly columns: readonly EntityColumn[]
}[] = [
  {
    section: "os_diskstats", label: "system.entities.devices",
    columns: [text("device", 150, true), rateNumber("reads"), rateNumber("writes"), rateNumber("read_sectors"), rateNumber("write_sectors"), rateMilliseconds("read_time_ms"), rateMilliseconds("write_time_ms"), rateNumber("discards"), rateNumber("flushes"), number("io_in_progress")],
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
    ...panel.columns.map((column: EntityColumn) => column.field),
    ...registry.filter((layout) => layout.logicalName === panel.section).flatMap((layout) => layout.identity),
  ])
  return [...wanted].map(([section, fields]) => ({ section, fields: [...fields] }))
}

export const SYSTEM_REQUESTS = systemRequests()

export function SystemView({
  context,
  contextRow,
  cursor,
  data,
  focus,
  hour,
  locale,
  onCursor,
  onContextClear,
  onFinding,
  t,
}: {
  readonly context: EntityContext | null
  readonly contextRow: DataRow | null
  readonly cursor: number
  readonly data: HourData
  readonly focus: Finding | null
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onContextClear: () => void
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
  const fallbackPoints = selectedMetric?.points ?? []
  const request = useMemo(() => selectedMetric === undefined ? null : metricHistoryRequest(selectedMetric.spec), [selectedMetric])
  const requestKey = request === null || selectedMetric === undefined ? null : metricRequestKey(hour, selectedMetric.spec, request)
  const [loadedHistory, setLoadedHistory] = useState<{ readonly key: string; readonly rows: readonly DataRow[] } | null>(null)
  useEffect(() => {
    if (request === null || requestKey === null || distinctTimes(fallbackPoints) > 1) {
      setLoadedHistory(null)
      return
    }
    const controller = new AbortController()
    acceptResponse(loadSeries(hour, request.section, request.where, request.fields, controller.signal), controller.signal,
      (rows) => setLoadedHistory({ key: requestKey, rows }), () => setLoadedHistory(null))
    return () => controller.abort()
  }, [fallbackPoints, hour, request, requestKey])
  const selectedPoints = useMemo(() => {
    if (selectedMetric === undefined || loadedHistory?.key !== requestKey) return fallbackPoints
    const loadedPoints = metricHistoryPoints(selectedMetric.spec, loadedHistory.rows)
    return loadedPoints.length === 0 ? fallbackPoints : loadedPoints
  }, [fallbackPoints, loadedHistory, requestKey, selectedMetric])
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={onCursor} onFinding={onFinding} primaryLane={timelineLane(selectedMetric?.spec.id)} shownAt={shownAt} t={t} />
    <section className="system-console">
      <div className="metric-groups">
        {GROUP_COLUMNS.map((column, index) => <div className="metric-column" key={index}>
          {column.map((id) => {
            const group = GROUPS.find((candidate) => candidate.id === id)
            const metrics = available.filter(({ spec }) => spec.group === id)
            if (group === undefined || metrics.length === 0) return null
            return <section className="metric-group" data-testid={`system-group-${group.id}`} key={group.id}>
              <h2><span>{t(group.label)}</span></h2>
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
          : <SeriesChart cursor={cursor} empty={t("status.no_data")} format={(reading, place) => metricChartValue(reading, place, selectedMetric.spec.unit)} hour={hour} label={t(selectedMetric.spec.label)} locale={locale} onCursor={onCursor} points={selectedPoints} scale={selectedMetric.spec.unit === "%" ? "percent" : "nonnegative"} unit={metricChartUnit(selectedMetric.spec, locale)} />}
      </section>
    </section>
    <UseTable cursor={cursor} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={onCursor} t={t} />
    <section className="entity-panels">
      {SYSTEM_ENTITIES.map((entity) => {
        const allRows = snapshot(sectionRows(data, entity.section), cursor)
        const activeContext = context?.logicalName === entity.section ? context : null
        const rows = contextualRows(allRows, activeContext, activeContext === null ? null : contextRow)
        if (rows.length === 0 && activeContext === null) return null
        const finding = focus?.logicalName === entity.section ? focus : null
        return <SystemEntityPanel
          columns={entity.columns}
          contextLabel={activeContext?.label}
          cursor={cursor}
          finding={finding}
          hour={hour}
          key={entity.section}
          label={t(entity.label)}
          locale={locale}
          onContextClear={activeContext === null ? undefined : onContextClear}
          onCursor={onCursor}
          rows={rows}
          section={entity.section}
          t={t}
        />
      })}
    </section>
  </>
}

function SystemEntityPanel({
  columns,
  contextLabel,
  cursor,
  finding,
  hour,
  label,
  locale,
  onContextClear,
  onCursor,
  rows,
  section,
  t,
}: {
  readonly columns: readonly EntityColumn[]
  readonly contextLabel?: string | undefined
  readonly cursor: number
  readonly finding: Finding | null
  readonly hour: number
  readonly label: string
  readonly locale: Locale
  readonly onContextClear?: (() => void) | undefined
  readonly onCursor: (timestamp: number) => void
  readonly rows: readonly DataRow[]
  readonly section: string
  readonly t: Translate
}) {
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
  const historyRequest = selectedRow === null || selectedColumn === undefined ? null : entityHistoryRequest(selectedRow, selectedColumn)
  const historyKey = historyRequest === null ? null : `${hour}:${historyRequest.key}`
  const [history, setHistory] = useState<{ readonly key: string; readonly rows: readonly DataRow[] } | null>(null)
  const requestFields = historyRequest === null ? "[]" : JSON.stringify(historyRequest.fields)
  const requestWhere = historyRequest === null ? "{}" : JSON.stringify(historyRequest.where)
  const requestSection = historyRequest?.section ?? ""
  const requestTypeId = historyRequest?.typeId
  useEffect(() => {
    if (historyKey === null || requestSection === "" || requestTypeId === undefined) {
      setHistory(null)
      return
    }
    const controller = new AbortController()
    const fields = JSON.parse(requestFields) as readonly string[]
    const where = JSON.parse(requestWhere) as Readonly<Record<string, string>>
    acceptResponse(loadSeries(hour, requestSection, where, fields, controller.signal, requestTypeId), controller.signal,
      (loaded) => setHistory({ key: historyKey, rows: loaded }), () => setHistory(null))
    return () => controller.abort()
  }, [historyKey, hour, requestFields, requestSection, requestTypeId, requestWhere])
  const chartRows = history?.key === historyKey ? history.rows : selectedRow === null ? [] : [selectedRow]
  const chartPoints = useMemo(() => selectedColumn === undefined ? [] : entityMetricPoints(chartRows, selectedColumn), [chartRows, selectedColumn])
  const chartMetadata = selectedRow === null || selectedColumn === undefined ? null : registryColumn(selectedRow.typeId, physicalField(selectedColumn, selectedRow.typeId))
  return <section className="entity-panel" data-testid={`system-panel-${section}`}>
    <h2><span>{label}</span></h2>
    <EntityTable
      columns={columns}
      contextLabel={contextLabel}
      empty={t("table.no_rows")}
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
    {selectedRow !== null && selectedColumn !== undefined && <section className="system-entity-history" data-testid={`system-${section}-history`}>
      <header>
        <div className="system-history-selector" role="group">
          {availableColumns.map((column) => <button aria-pressed={column.field === selectedColumn.field} key={column.field} onClick={() => setSelectedField(column.field)} type="button">{t(column.label)}</button>)}
        </div>
        <button aria-label={t("common.close")} className="system-history-close" onClick={() => setSelectedKey(null)} type="button">×</button>
      </header>
      <SeriesChart
        cursor={cursor}
        empty={t("status.no_data")}
        format={(reading, place) => entityMetricValue(reading, place, selectedColumn, chartMetadata)}
        hour={hour}
        label={t(selectedColumn.label)}
        locale={locale}
        onCursor={onCursor}
        points={chartPoints}
        scale={selectedColumn.kind === "percent" ? "percent" : "nonnegative"}
        unit={entityMetricUnit(selectedColumn, locale, chartMetadata)}
      />
    </section>}
  </section>
}

export function chartableEntityColumns(columns: readonly EntityColumn[]): readonly EntityColumn[] {
  return columns.filter((column) => column.kind === "number"
    || column.kind === "estimated_rows"
    || column.kind === "bytes"
    || column.kind === "kib"
    || column.kind === "milliseconds"
    || column.kind === "duration"
    || column.kind === "microseconds"
    || column.kind === "percent")
}

export function entityHistoryRequest(row: DataRow, column: EntityColumn): EntityHistoryRequest | null {
  if (!chartableEntityColumns([column]).includes(column)) return null
  const layout = registry.find((candidate) => candidate.typeId === row.typeId && candidate.logicalName === row.logicalName)
  if (layout === undefined || layout.identity.length === 0) return null
  const identities = layout.identity.map((field) => [field, rawText(value(row, field))] as const)
  if (identities.some(([, stored]) => stored === null)) return null
  const field = physicalField(column, row.typeId)
  const where = Object.fromEntries(identities) as Readonly<Record<string, string>>
  const fields = uniqueStrings([field, ...layout.identity])
  return {
    fields,
    key: JSON.stringify([row.segmentId, row.typeId, identities, field]),
    section: row.logicalName,
    typeId: row.typeId,
    where,
  }
}

function entityMetricPoints(rows: readonly DataRow[], column: EntityColumn): readonly ChartPoint[] {
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

function entityMetricUnit(column: EntityColumn, locale: Locale, metadata: RegistryColumn | null): string {
  const perSecond = metadata?.class === "cumulative" ? "/s" : ""
  if (column.field === "speed_mbit") return "Mbit/s"
  if (column.field === "mhz_max") return "MHz"
  if (column.kind === "bytes" || column.kind === "kib") return `${locale === "ru" ? "байты" : "bytes"}${perSecond}`
  if (column.kind === "milliseconds" || column.kind === "duration") return `${locale === "ru" ? "мс" : "ms"}${perSecond}`
  if (column.kind === "microseconds") return `${locale === "ru" ? "мкс" : "µs"}${perSecond}`
  if (column.kind === "percent") return "%"
  if (metadata?.unit === "sectors") return `${locale === "ru" ? "секторы" : "sectors"}${perSecond}`
  return metadata?.class === "cumulative" ? (locale === "ru" ? "1/с" : "1/s") : (locale === "ru" ? "количество" : "count")
}

function entityMetricValue(reading: number, locale: Locale, column: EntityColumn, metadata: RegistryColumn | null): string {
  const suffix = metadata?.class === "cumulative" ? "/s" : ""
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
  if (metric === "cpu_busy") return "cpu_busy"
  if (metric === "cpu_pressure") return "cpu_stall"
  if (metric === "io_pressure") return "io_stall"
  if (metric?.startsWith("mem_") === true) return "memory"
  return "health"
}

function normalizedMetricPoints(data: HourData, spec: MetricSpec): readonly ChartPoint[] {
  const mapping: Readonly<Record<string, readonly [string, (value: number) => number]>> = {
    cpu_busy: ["cpu_busy", (number) => number],
    cpu_pressure: ["cpu_stall", (number) => number],
    io_pressure: ["io_stall", (number) => number],
    mem_available_percent: ["memory", (number) => 100 - number],
    network_rx: ["net_rx", (number) => number],
    network_tx: ["net_tx", (number) => number],
    network_errors: ["net_errors", (number) => number],
    network_drops: ["net_drop", (number) => number],
    oom_kill: ["mem_oom", (number) => number],
  }
  const selected = mapping[spec.id]
  if (selected === undefined) return []
  const [lane, transform] = selected
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
  const field = spec.field
  return buildMetricSamples(sectionRows(data, spec.section), (row) => {
    if (spec.resource !== undefined && asNumber(value(row, "resource")) !== spec.resource) return undefined
    return storedNumber(row, field)
  })
}

export function metricHistoryRequest(spec: MetricSpec): MetricHistoryRequest | null {
  const section = spec.derive === undefined ? spec.section : DERIVE_INPUTS[spec.derive][0]
  if (section === undefined) return null
  const derivedFields = spec.derive === undefined ? [] : DERIVE_INPUTS[spec.derive][1]
  const fields = uniqueStrings([
    ...(spec.field === undefined ? [] : [spec.field]),
    ...derivedFields,
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
  if (spec.derive !== undefined) return derivedRowPoints(rows, spec.derive)
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
  const [section] = DERIVE_INPUTS[derive]
  return derivedRowPoints(sectionRows(data, section), derive)
}

function derivedRowPoints(rows: readonly DataRow[], derive: NonNullable<MetricSpec["derive"]>): readonly ChartPoint[] {
  if (derive === "cpu_busy") return cpuBusyPoints(rows)
  if (derive === "mem_available_percent") return buildMetricSamples(rows, (row) => {
    const available = storedNumber(row, "mem_available")
    const total = storedNumber(row, "mem_total")
    if (available === undefined || total === undefined) return undefined
    return available === null || total === null || total <= 0 ? null : available / total * 100
  })
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
  if (derive === "device_count" || derive === "filesystem_count" || derive === "interface_count") return aggregateRows(rows, (sampleRows) => sampleRows.length)
  if (derive === "device_active_io") return aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["io_in_progress"]))
  if (derive === "network_rx") return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["rx_bytes"])))
  if (derive === "network_tx") return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["tx_bytes"])))
  if (derive === "network_errors") return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["rx_errs", "tx_errs"])))
  return cumulativeRate(aggregateRows(rows, (sampleRows) => sumFields(sampleRows, ["rx_drop", "tx_drop"])))
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
      const counters = fields.map((field) => storedNumber(row, field))
      if (counters.some((counter) => counter === undefined)) continue
      let output: number | null = null
      if (previous !== null && counters.every((counter): counter is number => counter !== null && counter !== undefined)) {
        const deltas = counters.map((counter, index) => counter - (previous?.[index] ?? counter))
        const total = deltas.reduce((sum, delta) => sum + delta, 0)
        const idle = (deltas[3] ?? 0) + (deltas[4] ?? 0)
        if (total > 0 && deltas.every((delta) => delta >= 0)) output = (total - idle) / total * 100
      }
      points.push({ segmentId, timestamp: row.timestamp, value: output })
      previous = counters.every((counter): counter is number => counter !== null && counter !== undefined) ? counters : null
    }
  }
  return points
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
  return registry.flatMap((layout) => layout.logicalName === spec.section ? layout.columnMetadata ?? [] : [])
    .find(({ name }) => name === spec.field)?.class ?? null
}

function registryColumn(typeId: string, field: string): RegistryColumn | null {
  return registry.find((layout) => layout.typeId === typeId)?.columnMetadata?.find(({ name }) => name === field) ?? null
}

export function metricValue(value: Cell, locale: Locale, unit: string): string {
  if (unit === " KiB") return humanBytes(asNumber(value) === null ? null : (asNumber(value) ?? 0) * 1024, locale)
  if (unit === " B") return humanBytes(value, locale, "/s")
  return measure(value, locale, unit)
}

function metricChartValue(value: number, locale: Locale, unit: string): string {
  if (unit === "%") return humanPercent(value, locale)
  if (unit === " KiB") return measure(value, locale, " KiB")
  if (unit === " B") return humanBytes(value, locale, "/s")
  return measure(value, locale)
}

export function fallbackMetric(logicalName: string): string | null {
  if (logicalName === "os_cpu" || logicalName === "os_stat") return "cpu_busy"
  if (logicalName === "os_loadavg") return "load1"
  if (logicalName === "os_meminfo") return "mem_available_percent"
  if (logicalName === "os_vmstat") return "oom_kill"
  return logicalName === "health" ? "health" : null
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

function pressureMetric(id: string, label: string, resource: number): MetricSpec {
  return { id, group: "pressure", label: `${label}.label`, help: `${label}.help`, section: "os_psi", field: "some_avg10", resource, unit: "%" }
}

function point(source: Point): ChartPoint { return source }
function systemColumn(field: string, kind: NonNullable<EntityColumn["kind"]>, width: number, sticky = false): EntityColumn {
  const obvious = new Set(["device", "mount_point", "fstype", "source", "iface", "cpu_id", "model_name"])
  return { field, label: `system.field.${field}.label`, ...(obvious.has(field) ? {} : { help: `system.field.${field}.help` }), kind, width, sticky }
}
function text(field: string, width = 130, sticky = false): EntityColumn { return systemColumn(field, "text", width, sticky) }
function number(field: string, width = 126): EntityColumn { return systemColumn(field, "number", width) }
function id(field: string, width = 110, sticky = false): EntityColumn { return systemColumn(field, "id", width, sticky) }
function bytes(field: string, width = 145): EntityColumn { return systemColumn(field, "bytes", width) }
function milliseconds(field: string, width = 145): EntityColumn { return systemColumn(field, "milliseconds", width) }
function boolean(field: string, width = 130): EntityColumn { return systemColumn(field, "boolean", width) }
function rateColumn(column: EntityColumn): EntityColumn { return { ...column, rate: true } }
function rateNumber(field: string, width = 126): EntityColumn { return rateColumn(number(field, width)) }
function rateBytes(field: string, width = 145): EntityColumn { return rateColumn(bytes(field, width)) }
function rateMilliseconds(field: string, width = 145): EntityColumn { return rateColumn(milliseconds(field, width)) }
