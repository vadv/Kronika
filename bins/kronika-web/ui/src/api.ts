import { registry } from "kronika:registry"

import { bundledFixtureHour, bundledFixtureRange } from "./fixture"
import { parseNdjson } from "./wire"

export type Cell = null | boolean | number | string | readonly number[] | { readonly [key: string]: unknown }

const REGISTRY_BY_TYPE_ID = new Map(registry.map((layout) => [layout.typeId, layout]))
const REGISTRY_LOGICAL_NAMES = unique(registry.flatMap((layout) =>
  layout.logicalName === null ? [] : [layout.logicalName],
))

const POSTGRESQL_OVERVIEW = [
  "pg_stat_bgwriter",
  "pg_stat_checkpointer",
  "pg_stat_wal",
  "pg_stat_archiver",
  "pg_stat_io",
  "pg_prepared_xacts",
  "pg_stat_statements_info",
] as const

export const PRODUCT_SECTION_GROUPS = {
  host: REGISTRY_LOGICAL_NAMES.filter((name) => name === "instance_metadata" || name.startsWith("os_")),
  postgresqlOverview: POSTGRESQL_OVERVIEW,
  postgresqlActivity: ["pg_stat_activity", "pg_stat_progress_vacuum"] as const,
  postgresqlStatements: ["pg_stat_statements"] as const,
  postgresqlLocks: ["pg_locks"] as const,
  postgresqlDatabases: ["pg_stat_database"] as const,
  events: REGISTRY_LOGICAL_NAMES.filter((name) => name.startsWith("pg_log_") || name === "pgbouncer_events"),
} as const

const UI_SECTION_NAMES = unique(Object.values(PRODUCT_SECTION_GROUPS).flat())
const UI_SECTION_NAME_SET = new Set(UI_SECTION_NAMES)
const REQUEST_CONCURRENCY = 4

export interface DataRow {
  readonly segmentId: string
  readonly logicalName: string
  readonly typeId: string
  readonly ordinal: string
  readonly timestamp: number
  readonly values: Readonly<Record<string, Cell>>
}

export interface Point {
  readonly segmentId: string
  readonly logicalName: string
  readonly typeId: string
  readonly series: string
  readonly timestamp: number
  readonly identity: Readonly<Record<string, Cell>>
  readonly value: number | null
}

export interface Finding {
  readonly segmentId: string
  readonly logicalName: string
  readonly kind: "known_bad" | "spike" | "event"
  readonly typeId: string
  readonly timestamp: number
  readonly category: number | null
  readonly rowOrdinal: string
  readonly fieldOrdinal: number
}

interface Section {
  readonly logical_name: string | null
}

interface Segment {
  readonly id: string
  readonly min_ts: string
  readonly max_ts: string
  readonly sections: readonly Section[]
}

export interface SourceFamily {
  readonly name: string
  readonly configured: boolean
  readonly present: boolean
}

export interface HourSelection {
  readonly latest: number
  readonly available: readonly number[]
}

export interface HourData {
  /** Rows keyed by their registry logical section name. */
  readonly sections: Readonly<Record<string, readonly DataRow[]>>
  /** Catalog-backed names, including present sections that have no row in this hour. */
  readonly availableSections: readonly string[]
  readonly processes: readonly DataRow[]
  readonly activities: readonly DataRow[]
  readonly load: readonly DataRow[]
  readonly memory: readonly DataRow[]
  readonly pressure: readonly DataRow[]
  readonly health: readonly DataRow[]
  readonly pgOverview: readonly DataRow[]
  readonly pgStatements: readonly DataRow[]
  readonly pgLocks: readonly DataRow[]
  readonly pgDatabases: readonly DataRow[]
  readonly pgEvents: readonly DataRow[]
  readonly points: readonly Point[]
  readonly findings: readonly Finding[]
  readonly sourceFamilies: readonly SourceFamily[]
  readonly segmentCount: number
}

export interface ResolvedLocator {
  readonly logicalName: string
  readonly row: DataRow
  readonly fieldName: string | null
}

export const PROCESS_FIELDS = [
  "pid", "starttime", "ppid", "uid", "euid", "gid", "egid", "state", "num_threads",
  "tty", "comm", "cmdline", "utime", "stime", "nice", "prio", "rtprio", "policy",
  "curcpu", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "minflt", "majflt",
  "vmem_kb", "rmem_kb", "vswap_kb", "syscr", "syscw", "rchar", "wchar", "read_bytes",
  "write_bytes", "cancelled_write_bytes", "exit_signal", "scope",
] as const

export const ACTIVITY_FIELDS = [
  "pid", "leader_pid", "datname", "usename", "application_name", "client_addr", "backend_type",
  "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age",
  "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change",
] as const

export async function discoverHourSelection(signal: AbortSignal): Promise<HourSelection> {
  const fixture = bundledFixtureRange()
  if (fixture !== null) {
    return {
      latest: floorHour(fixture.from),
      available: unique([floorHour(fixture.from), floorHour(fixture.to)]).sort((left, right) => left - right),
    }
  }
  const records = await request("/api/catalog", signal)
  const segments = catalogSegments(records)
  const latest = segments.reduce((current, segment) => Math.max(current, Number(segment.max_ts)), 0)
  return {
    latest: latest > 0 ? floorHour(latest) : floorHour(Date.now() * 1_000),
    available: unique(segments.flatMap((segment) => {
      const from = Number(segment.min_ts)
      const to = Number(segment.max_ts)
      return Number.isFinite(from) && Number.isFinite(to) && to >= from ? [floorHour(from), floorHour(to)] : []
    })).sort((left, right) => left - right),
  }
}

export async function discoverLatestHour(signal: AbortSignal): Promise<number> {
  return (await discoverHourSelection(signal)).latest
}

export async function loadHour(start: number, signal: AbortSignal): Promise<HourData> {
  const fixture = bundledFixtureHour(start)
  if (fixture !== null) return fixture
  const end = start + 3_600_000_000
  const catalog = await request(`/api/catalog?from=${start}&to=${end - 1}`, signal)
  const segments = catalogSegments(catalog).filter(
    (segment) => Number(segment.min_ts) < end && Number(segment.max_ts) >= start,
  )
  const sourceFamilies = catalog
    .find((record) => record.record === "catalog")?.source_families as readonly SourceFamily[] | undefined
  const tasks = segments.flatMap(segmentTasks)
  const loaded = await mapLimited(tasks, REQUEST_CONCURRENCY, (task) => runTask(task, signal))
  const within = (row: { readonly timestamp: number }) => row.timestamp >= start && row.timestamp < end
  const grouped: Record<string, DataRow[]> = {}
  const points: Point[] = []
  const findings: Finding[] = []
  for (const result of loaded) {
    if (result.kind === "history") {
      const rows = grouped[result.logicalName] ?? []
      rows.push(...result.rows.filter(within))
      grouped[result.logicalName] = rows
    } else {
      points.push(...result.points.filter(within))
      findings.push(...result.findings.filter(within))
    }
  }
  const availableSections = availableSectionNames(segments)
  for (const name of availableSections) grouped[name] ??= []
  const sections = Object.fromEntries(availableSections.map((name) => [name, grouped[name] ?? []]))
  return hourData({
    sections,
    availableSections,
    points: points.sort(pointOrder),
    findings: findings.sort(findingOrder),
    sourceFamilies: sourceFamilies ?? [],
    segmentCount: segments.length,
  })
}

export function sectionRows(data: HourData, logicalName: string): readonly DataRow[] {
  return data.sections[logicalName] ?? []
}

export function logicalNameForTypeId(typeId: string): string | null {
  if (typeId === "0") return "health"
  return REGISTRY_BY_TYPE_ID.get(typeId)?.logicalName ?? null
}

export function fieldNameForLocator(locator: Pick<Finding, "typeId" | "fieldOrdinal">): string | null {
  if (locator.typeId === "0") return locator.fieldOrdinal === 0 ? "os_health" : null
  return REGISTRY_BY_TYPE_ID.get(locator.typeId)?.columns[locator.fieldOrdinal]?.name ?? null
}

export function resolveLoadedRow(
  data: Pick<HourData, "sections">,
  locator: Pick<Finding, "segmentId" | "typeId" | "rowOrdinal" | "timestamp">,
): DataRow | null {
  const logicalName = logicalNameForTypeId(locator.typeId)
  if (logicalName === null) return null
  return (data.sections[logicalName] ?? []).find((row) =>
    row.segmentId === locator.segmentId
      && row.typeId === locator.typeId
      && row.ordinal === locator.rowOrdinal
      && row.timestamp === locator.timestamp,
  ) ?? null
}

export function resolveLocator(data: Pick<HourData, "sections">, finding: Finding): ResolvedLocator | null {
  const row = resolveLoadedRow(data, finding)
  if (row === null) return null
  return {
    logicalName: row.logicalName,
    row,
    fieldName: fieldNameForLocator(finding),
  }
}

function hourData(input: {
  readonly sections: Readonly<Record<string, readonly DataRow[]>>
  readonly availableSections: readonly string[]
  readonly points: readonly Point[]
  readonly findings: readonly Finding[]
  readonly sourceFamilies: readonly SourceFamily[]
  readonly segmentCount: number
}): HourData {
  const rows = (name: string) => input.sections[name] ?? []
  const flatten = (names: readonly string[]) => names.flatMap(rows)
  return {
    ...input,
    processes: rows("os_process"),
    activities: rows("pg_stat_activity"),
    load: rows("os_loadavg"),
    memory: rows("os_meminfo"),
    pressure: rows("os_psi"),
    health: rows("health"),
    pgOverview: flatten(PRODUCT_SECTION_GROUPS.postgresqlOverview),
    pgStatements: flatten(PRODUCT_SECTION_GROUPS.postgresqlStatements),
    pgLocks: flatten(PRODUCT_SECTION_GROUPS.postgresqlLocks),
    pgDatabases: flatten(PRODUCT_SECTION_GROUPS.postgresqlDatabases),
    pgEvents: flatten(PRODUCT_SECTION_GROUPS.events),
  }
}

type LoadTask =
  | { readonly kind: "history"; readonly segmentId: string; readonly logicalName: string }
  | { readonly kind: "index"; readonly segmentId: string; readonly logicalName: string }

type LoadResult =
  | { readonly kind: "history"; readonly logicalName: string; readonly rows: readonly DataRow[] }
  | { readonly kind: "index"; readonly points: readonly Point[]; readonly findings: readonly Finding[] }

function segmentTasks(segment: Segment): readonly LoadTask[] {
  const names = segmentSectionNames(segment)
  const tasks: LoadTask[] = names.map((logicalName) => ({
    kind: "history",
    segmentId: segment.id,
    logicalName,
  }))
  for (const logicalName of names) tasks.push({ kind: "index", segmentId: segment.id, logicalName })
  if (names.some((name) => name.startsWith("os_"))) {
    tasks.push(
      { kind: "history", segmentId: segment.id, logicalName: "health" },
      { kind: "index", segmentId: segment.id, logicalName: "health" },
    )
  }
  return tasks
}

async function runTask(task: LoadTask, signal: AbortSignal): Promise<LoadResult> {
  if (task.kind === "history") {
    const rows = await readHistory(
      task.segmentId,
      task.logicalName,
      fieldsForLogicalName(task.logicalName),
      signal,
    ).catch((error: unknown) => task.logicalName === "health" ? absent(error) : Promise.reject(error))
    return { kind: "history", logicalName: task.logicalName, rows }
  }
  const result = await readIndex(task.segmentId, task.logicalName, signal)
    .catch((error: unknown) => absentIndex(error))
  return { kind: "index", ...result }
}

export function fieldsForLogicalName(logicalName: string): readonly string[] {
  if (logicalName === "health") return ["health"]
  return unique(registry
    .filter((layout) => layout.logicalName === logicalName)
    .flatMap((layout) => layout.columns.map((column) => column.name))
    .filter((name) => name !== "ts"))
}

async function readHistory(
  segmentId: string,
  logicalName: string,
  fields: readonly string[],
  signal: AbortSignal,
): Promise<readonly DataRow[]> {
  const query = fields.map((field) => `field=${encodeURIComponent(field)}`).join("&")
  const suffix = query === "" ? "" : `?${query}`
  const records = await request(
    `/api/segments/${encodeURIComponent(segmentId)}/sections/${encodeURIComponent(logicalName)}/history${suffix}`,
    signal,
  )
  const layouts = new Map<string, readonly string[]>()
  const rows: DataRow[] = []
  for (const record of records) {
    if (record.record === "layout") {
      const layout = record.layout as {
        readonly type_id: unknown
        readonly columns: readonly { readonly name: unknown }[]
      }
      const typeId = requiredText(layout.type_id, "layout type_id")
      const registeredName = logicalNameForTypeId(typeId)
      if (registeredName !== logicalName) {
        throw new Error(`layout ${typeId} does not belong to ${logicalName}`)
      }
      if (!Array.isArray(layout.columns)) throw new Error(`layout ${typeId} has no columns`)
      layouts.set(typeId, layout.columns.map((column) => requiredText(column.name, "column name")))
    } else if (record.record === "row") {
      const typeId = requiredText(record.type_id, "row type_id")
      const names = layouts.get(typeId)
      const values = record.values as readonly Cell[]
      if (names === undefined || !Array.isArray(values)) {
        throw new Error(`row for layout ${typeId} arrived before its layout`)
      }
      rows.push({
        segmentId,
        logicalName,
        typeId,
        ordinal: requiredText(record.ordinal, "row ordinal"),
        timestamp: integer(record.timestamp, "row timestamp"),
        values: Object.fromEntries(names.map((name, index) => [
          name,
          index < values.length ? values[index] as Cell : null,
        ])),
      })
    }
  }
  return rows
}

async function readIndex(segmentId: string, logicalName: string, signal: AbortSignal) {
  const records = await request(
    `/api/segments/${encodeURIComponent(segmentId)}/sections/${encodeURIComponent(logicalName)}/index`,
    signal,
  )
  const points: Point[] = []
  const findings: Finding[] = []
  for (const record of records) {
    if (record.record === "point") {
      const typeId = requiredText(record.type_id, "point type_id")
      points.push({
        segmentId,
        logicalName,
        typeId,
        series: requiredText(record.series, "point series"),
        timestamp: integer(record.ts, "point timestamp"),
        identity: cellRecord(record.identity),
        value: record.value === null ? null : finiteNumber(record.value, "point value"),
      })
    } else if (record.record === "finding"
      && (record.kind === "known_bad" || record.kind === "spike" || record.kind === "event")) {
      const typeId = requiredText(record.type_id, "finding type_id")
      findings.push({
        segmentId,
        logicalName: logicalNameForTypeId(typeId) ?? logicalName,
        kind: record.kind,
        typeId,
        timestamp: integer(record.ts, "finding timestamp"),
        category: typeof record.category === "number" ? record.category : null,
        rowOrdinal: requiredText(record.row_ordinal, "finding row ordinal"),
        fieldOrdinal: integer(record.field_ordinal, "finding field ordinal"),
      })
    }
  }
  return { points, findings }
}

async function request(path: string, signal: AbortSignal): Promise<readonly Record<string, unknown>[]> {
  const response = await fetch(path, { headers: { Accept: "application/x-ndjson" }, signal })
  if (!response.ok) {
    const error = new Error(`HTTP ${response.status} for ${path}`) as Error & { status?: number }
    error.status = response.status
    throw error
  }
  const body = await response.text()
  return parseNdjson(body, path)
}

function catalogSegments(records: readonly Record<string, unknown>[]): readonly Segment[] {
  return records.filter(
    (record) => record.record === "finished_segment" || record.record === "active_segment",
  ) as unknown as readonly Segment[]
}

function segmentSectionNames(segment: Segment): string[] {
  const present = new Set(segment.sections.flatMap((section) =>
    section.logical_name !== null && UI_SECTION_NAME_SET.has(section.logical_name) ? [section.logical_name] : [],
  ))
  return UI_SECTION_NAMES.filter((name) => present.has(name))
}

function availableSectionNames(segments: readonly Segment[]): string[] {
  const present = new Set(segments.flatMap(segmentSectionNames))
  if ([...present].some((name) => name.startsWith("os_"))) present.add("health")
  return [...UI_SECTION_NAMES, "health"].filter((name) => present.has(name))
}

function absent(error: unknown): readonly DataRow[] {
  if (status(error) === 404) return []
  throw error
}

function absentIndex(error: unknown): { readonly points: readonly Point[]; readonly findings: readonly Finding[] } {
  if (status(error) === 404) return { points: [], findings: [] }
  throw error
}

function status(error: unknown): number | undefined {
  return error instanceof Error ? (error as Error & { readonly status?: number }).status : undefined
}

function requiredText(value: unknown, name: string): string {
  if ((typeof value === "string" && value !== "") || typeof value === "number") return String(value)
  throw new Error(`${name} is invalid`)
}

function integer(value: unknown, name: string): number {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN
  if (!Number.isSafeInteger(parsed)) throw new Error(`${name} is invalid`)
  return parsed
}

function finiteNumber(value: unknown, name: string): number {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN
  if (!Number.isFinite(parsed)) throw new Error(`${name} is invalid`)
  return parsed
}

function cellRecord(value: unknown): Readonly<Record<string, Cell>> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return {}
  return value as Readonly<Record<string, Cell>>
}

function unique<Value>(values: readonly Value[]): Value[] {
  return [...new Set(values)]
}

function pointOrder(left: Point, right: Point): number {
  return left.timestamp - right.timestamp
    || left.segmentId.localeCompare(right.segmentId)
    || left.typeId.localeCompare(right.typeId)
    || left.series.localeCompare(right.series)
}

function findingOrder(left: Finding, right: Finding): number {
  return left.timestamp - right.timestamp
    || left.segmentId.localeCompare(right.segmentId)
    || left.typeId.localeCompare(right.typeId)
    || Number(left.rowOrdinal) - Number(right.rowOrdinal)
    || left.fieldOrdinal - right.fieldOrdinal
}

async function mapLimited<Input, Output>(
  inputs: readonly Input[],
  limit: number,
  run: (input: Input) => Promise<Output>,
): Promise<readonly Output[]> {
  const outputs = new Array<Output>(inputs.length)
  let next = 0
  const worker = async () => {
    while (next < inputs.length) {
      const index = next
      next += 1
      const input = inputs[index]
      if (input !== undefined) outputs[index] = await run(input)
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, inputs.length) }, worker))
  return outputs
}

function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}
