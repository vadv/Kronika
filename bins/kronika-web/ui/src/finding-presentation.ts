import { registry } from "kronika:registry"

import { fieldNameForLocator, type Cell, type DataRow, type Finding, type HourData, type LanePoint } from "./api"
import { buildMetricSamples } from "./chart"
import type { Translate } from "./help"
import { asNumber, rawText, value } from "./model"
import { processLensHistory } from "./detail"
import { decoratePostgresIntervalRow, findingSemanticField, postgresHistory, postgresProjection } from "./postgres-metrics"
import type { ChartPoint } from "./series-chart"

const LAYOUTS = new Map(registry.map((layout) => [layout.typeId, layout]))
const LOG_TYPES: Readonly<Record<string, string>> = {
  pg_log_errors: "events.type.errors",
  pg_log_checkpoints: "events.type.checkpoints",
  pg_log_autovacuum: "events.type.autovacuum",
  pg_log_slow_queries: "events.type.slow_queries",
  pg_log_lock_waits: "events.type.lock_waits",
  pg_log_lifecycle: "events.type.lifecycle",
  pg_log_temp_files: "events.type.temp_files",
  pgbouncer_events: "events.type.pgbouncer",
}

const SOURCES: Readonly<Record<string, string>> = {
  health: "events.source.health",
  os_cpu: "events.source.cpu",
  os_meminfo: "events.source.memory",
  os_loadavg: "events.source.load",
  os_mountinfo: "events.source.filesystem",
  os_vmstat: "events.source.oom",
  os_process: "events.source.process",
  pg_stat_statements: "events.source.statement",
  pg_stat_activity: "events.source.activity",
  pg_stat_database: "events.source.database",
}

const ERROR_CATEGORIES = [
  "lock", "constraint", "serialization", "timeout", "resource", "data_corruption",
  "system", "connection", "auth", "syntax", "other",
] as const

const DETAIL_FIELDS: Readonly<Record<string, readonly string[]>> = {
  pg_log_errors: ["severity", "category", "sqlstate", "pattern", "count", "sample", "detail", "hint", "context", "statement", "database", "username"],
  pg_log_checkpoints: ["phase", "reason", "seconds_apart", "buffers_written", "write_ms", "sync_ms", "total_ms", "distance_kb", "estimate_kb", "wal_added", "wal_removed", "wal_recycled", "sync_files", "longest_sync_ms", "average_sync_ms"],
  pg_log_autovacuum: ["kind", "relation", "index_scans", "pages_removed", "pages_remaining", "tuples_removed", "tuples_remaining", "tuples_dead_not_removable", "elapsed_ms", "buffer_hits", "buffer_misses", "buffer_dirtied", "avg_read_rate_mbs", "avg_write_rate_mbs", "cpu_user_ms", "cpu_system_ms", "wal_records", "wal_fpi", "wal_bytes"],
  pg_log_slow_queries: ["pattern", "sample", "count", "max_duration_ms", "total_duration_ms"],
  pg_log_lock_waits: ["kind", "pid", "lock_mode", "lock_target", "duration_ms", "holding_pids", "wait_queue", "detail", "context", "statement"],
  pg_log_lifecycle: ["kind", "pid", "signal", "shutdown_mode", "message", "query_detail"],
  pg_log_temp_files: ["path", "size_bytes", "statement"],
  pgbouncer_events: ["level", "database", "username", "host", "text"],
}

export function findingKey(finding: Finding): string {
  return `${finding.segmentId}:${finding.typeId}:${finding.rowOrdinal}:${finding.fieldOrdinal}:${finding.timestamp}:${finding.kind}`
}

export function findingOrder(left: Finding, right: Finding): number {
  return left.timestamp - right.timestamp
    || kindOrder(left.kind) - kindOrder(right.kind)
    || textOrder(left.segmentId, right.segmentId)
    || textOrder(left.typeId, right.typeId)
    || textOrder(left.rowOrdinal, right.rowOrdinal)
    || left.fieldOrdinal - right.fieldOrdinal
    || textOrder(left.logicalName, right.logicalName)
}

export function findingCategory(finding: Finding, t: Translate): string {
  return t(`locator.${finding.kind}`)
}

export function findingSource(finding: Finding, t: Translate): string {
  const log = LOG_TYPES[finding.logicalName]
  if (log !== undefined) {
    const category = finding.logicalName === "pg_log_errors" ? findingLogCategory(finding.category, t) : null
    return category === null ? t(log) : `${t(log)} · ${category}`
  }
  const source = SOURCES[finding.logicalName]
  if (source !== undefined) return t(source)
  return finding.logicalName
}

export function findingLogCategory(category: number | null, t: Translate): string | null {
  const name = category === null ? undefined : ERROR_CATEGORIES[category]
  return name === undefined ? null : t(`events.category.${name}`)
}

export function findingSummary(findings: readonly Finding[], t: Translate): string {
  const counts = new Map<string, number>()
  for (const finding of findings) {
    const label = finding.kind === "event" ? findingSource(finding, t) : findingCategory(finding, t)
    counts.set(label, (counts.get(label) ?? 0) + 1)
  }
  return [...counts].map(([label, count]) => `${label} ×${count}`).join(" · ")
}

export function findingProjection(finding: Finding): readonly string[] {
  if (finding.logicalName === "pg_stat_statements") return postgresProjection(finding.typeId)
  const layout = LAYOUTS.get(finding.typeId)
  return layout?.columns.filter((field) => field !== "ts") ?? []
}

export function findingHistoryRequest(finding: Finding, row: DataRow): {
  readonly fields: readonly string[]
  readonly where: Readonly<Record<string, string>>
} | null {
  if (finding.typeId === "0" || laneFor(finding) !== null || finding.logicalName === "pg_stat_activity"
    || finding.kind === "event" || finding.logicalName.startsWith("pg_log_")) return null
  const layout = LAYOUTS.get(finding.typeId)
  if (layout === undefined) return null
  const identityFields = finding.logicalName === "os_process" ? ["pid"] : layout.identity
  let fields = [...identityFields, fieldNameForLocator(finding)].filter((field): field is string => field !== null)
  if (finding.logicalName === "pg_stat_statements") fields = [...postgresProjection(finding.typeId)]
  if (finding.logicalName === "os_process") fields = ["pid", "read_bytes"]
  if (finding.logicalName === "os_cpu") fields = [...layout.identity, "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"]
  if (finding.logicalName === "os_mountinfo") fields = [...layout.identity, "total_bytes", "free_bytes"]
  if (finding.logicalName === "os_meminfo") fields = [...layout.identity, "mem_total", "mem_available"]
  const identity = identityFields.map((field) => [field, rawText(value(row, field))] as const)
  if (identity.some(([, stored]) => stored === null)) return null
  const where = Object.fromEntries(identity as readonly (readonly [string, string])[])
  return { fields: [...new Set(fields)], where }
}

export interface FindingMetric {
  readonly field: string | null
  readonly helpKey: string
  readonly label: string
  readonly labelKey: string
  readonly unit: "bytes_per_second" | "count" | "milliseconds" | "milliseconds_per_call" | "number" | "percent"
  readonly boundary: string | null
}

export function findingMetric(finding: Finding, t: Translate): FindingMetric {
  if (finding.kind === "event") return metric(null, "events.metric.unavailable", "events.metric.unavailable.help", t, "number", null)
  const physical = fieldNameForLocator(finding)
  const semantic = physical === null ? null : findingSemanticField(finding.typeId, physical)
  if (finding.logicalName === "os_cpu") return metric("cpu_busy", "events.metric.cpu_busy", "lane.cpu_busy.help", t, "percent", t("events.boundary.cpu"))
  if (finding.logicalName === "os_meminfo") return metric("mem_available", "events.metric.memory_available", "system.metric.mem_available.help", t, "percent", t("events.boundary.memory"))
  if (finding.logicalName === "os_loadavg") return metric("load1", "events.metric.load1", "system.metric.load1.help", t, "number", t("events.boundary.load"))
  if (finding.logicalName === "os_mountinfo") return metric("free_bytes", "events.metric.filesystem_used", "events.metric.filesystem_used.help", t, "percent", t("events.boundary.filesystem"))
  if (finding.logicalName === "os_vmstat" && physical === "oom_kill") return metric(physical, "events.metric.oom_kill", "system.metric.oom_kill.help", t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "pg_stat_database" && physical === "deadlocks") return metric(physical, "events.metric.deadlocks", "pg.field.deadlocks.help", t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "pg_stat_activity") return metric("active_backends", "events.metric.active_backends", "events.metric.active_backends.help", t, "count", t("events.boundary.active_backends"))
  if (finding.logicalName === "pg_log_slow_queries") return metric("max_duration_ms", "events.metric.slow_query", "events.metric.slow_query.help", t, "milliseconds", t("events.boundary.slow_query"))
  if (finding.logicalName === "health") return metric(physical, "events.metric.overall_health", "lane.health.help", t, "percent", t("events.boundary.health"))
  if (finding.logicalName === "os_process") return metric("read_bytes", "events.metric.process_read", "col.read_bytes.help", t, "bytes_per_second", null)
  if (semantic !== null) {
    return metric(semantic, `pg.field.${semantic}.label`, `pg.field.${semantic}.help`, t, semantic === "mean_exec_ms_per_call" ? "milliseconds_per_call" : "number", null)
  }
  return metric(physical, physical ?? "events.metric.unavailable", "events.metric.unavailable.help", t, unitFor(physical), null)
}

export function findingHistory(
  finding: Finding,
  rows: readonly DataRow[],
  data: Pick<HourData, "health" | "lanePoints" | "points">,
): readonly ChartPoint[] {
  const lane = laneFor(finding)
  if (lane !== null) return data.lanePoints.filter((point) => point.lane === lane).map((point) => ({
    ...lanePoint(point),
    value: finding.logicalName === "os_meminfo" && point.value !== null ? 100 - point.value : point.value,
  }))
  if (finding.logicalName === "pg_stat_activity") {
    return data.points.filter((point) => point.typeId === finding.typeId && point.series === "active_backends").map((point) => ({
      segmentId: point.segmentId,
      timestamp: point.timestamp,
      value: point.value,
    }))
  }
  if (finding.typeId === "0") {
    const field = fieldNameForLocator(finding)
    if (field === null) return []
    return buildMetricSamples(data.health, (row) => Object.hasOwn(row.values, field) ? asNumber(value(row, field)) : undefined)
  }
  if (finding.logicalName === "pg_stat_statements") {
    const physical = fieldNameForLocator(finding)
    const semantic = physical === null ? null : findingSemanticField(finding.typeId, physical)
    if (semantic === null) return []
    return buildMetricSamples(postgresHistory(rows), (point) => Object.hasOwn(point, semantic) ? point[semantic] : undefined)
  }
  if (finding.logicalName === "os_process") {
    return processLensHistory(rows, "disk").find((series) => series.field === "read_bytes")?.points ?? []
  }
  const field = fieldNameForLocator(finding)
  if (field === null) return []
  return buildMetricSamples(
    rows.slice().sort((left, right) => left.timestamp - right.timestamp),
    (item) => ownsMetric(finding, item) ? directValue(finding, item) : undefined,
  )
}

export function findingReadings(
  finding: Finding,
  row: DataRow | null,
  history: readonly ChartPoint[],
  data: Pick<HourData, "health" | "lanePoints" | "points">,
): { readonly previous: number | null; readonly current: number | null } {
  const points = history.length === 0 ? findingHistory(finding, row === null ? [] : [row], data) : history
  const ordered = points.slice().sort((left, right) => left.timestamp - right.timestamp)
  const at = ordered.findIndex((point) => point.segmentId === finding.segmentId && point.timestamp === finding.timestamp)
  const exact = row === null ? null : directValue(finding, row)
  const current = at < 0 ? exact : ordered[at]?.value ?? exact
  const previous = at <= 0 ? null : ordered[at - 1]?.value ?? null
  return { previous, current }
}

export function findingEntity(row: DataRow | null): string | null {
  if (row === null) return null
  for (const field of ["query", "sample", "statement", "cmdline", "message", "pattern"]) {
    const text = rawText(value(row, field))?.trim()
    if (text) return text.length > 160 ? `${text.slice(0, 157)}…` : text
  }
  const pid = rawText(value(row, "pid"))
  if (pid !== null) return `PID ${pid}`
  const database = rawText(value(row, "datname")) ?? rawText(value(row, "database"))
  const role = rawText(value(row, "usename")) ?? rawText(value(row, "username"))
  if (database !== null || role !== null) return [database, role].filter((part) => part !== null).join(" · ")
  return null
}

export function findingDetailFields(row: DataRow, finding: Finding): readonly (readonly [string, Cell])[] {
  const hidden = new Set(["ts", "starttime", "source_file", "system_identifier"])
  const preferred = finding.logicalName === "pg_stat_statements"
    ? ["datname", "usename", "queryid", "dbid", "userid", "toplevel"]
    : DETAIL_FIELDS[finding.logicalName] ?? ["pid", "cmdline", "comm", "datname", "usename", "mount_point", "device"]
  return preferred.flatMap((field) => hidden.has(field) || !Object.hasOwn(row.values, field) || value(row, field) === null
    ? []
    : [[field, value(row, field)] as const])
}

function directValue(finding: Finding, row: DataRow): number | null {
  if (finding.logicalName === "os_process") return null
  if (finding.logicalName === "os_mountinfo") {
    const total = asNumber(value(row, "total_bytes"))
    const free = asNumber(value(row, "free_bytes"))
    return total === null || free === null || total <= 0 ? null : 100 * (total - free) / total
  }
  if (finding.logicalName === "os_meminfo") {
    const total = asNumber(value(row, "mem_total"))
    const available = asNumber(value(row, "mem_available"))
    return total === null || available === null || total <= 0 ? null : 100 * available / total
  }
  if (finding.logicalName === "pg_stat_statements") {
    const physical = fieldNameForLocator(finding)
    const semantic = physical === null ? null : findingSemanticField(finding.typeId, physical)
    return semantic === null ? null : asNumber(value(decoratePostgresIntervalRow(row), semantic))
  }
  const field = finding.logicalName === "pg_log_slow_queries" ? "max_duration_ms" : fieldNameForLocator(finding)
  return field === null ? null : asNumber(value(row, field))
}

function ownsMetric(finding: Finding, row: DataRow): boolean {
  if (finding.logicalName === "os_mountinfo") return Object.hasOwn(row.values, "total_bytes") && Object.hasOwn(row.values, "free_bytes")
  if (finding.logicalName === "os_meminfo") return Object.hasOwn(row.values, "mem_total") && Object.hasOwn(row.values, "mem_available")
  const field = finding.logicalName === "pg_log_slow_queries" ? "max_duration_ms" : fieldNameForLocator(finding)
  return field !== null && Object.hasOwn(row.values, field)
}

function laneFor(finding: Finding): string | null {
  if (finding.logicalName === "os_cpu") return "cpu_busy"
  if (finding.logicalName === "os_meminfo") return "memory"
  return null
}

function lanePoint(point: LanePoint): ChartPoint {
  return { segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }
}

function metric(field: string | null, labelKey: string, helpKey: string, t: Translate, unit: FindingMetric["unit"], boundary: string | null): FindingMetric {
  return { field, helpKey, label: t(labelKey), labelKey, unit, boundary }
}

function unitFor(field: string | null): FindingMetric["unit"] {
  if (field?.endsWith("_bytes") === true) return "count"
  if (field?.endsWith("_ms") === true) return "milliseconds"
  return "number"
}

function kindOrder(kind: Finding["kind"]): number {
  return kind === "event" ? 0 : kind === "known_bad" ? 1 : 2
}

function textOrder(left: string, right: string): number { return left < right ? -1 : left > right ? 1 : 0 }
