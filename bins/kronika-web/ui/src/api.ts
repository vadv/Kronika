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

/** A view asks for the sections it draws and, where it only needs a few
 *  numbers, the fields it reads. A process row carries a command line. */
export interface SectionRequest {
  readonly section: string
  readonly fields?: readonly string[]
}

/** Every view draws the timeline, so these four are the floor. */
/** The one series drawn end to end. The other lanes were three more sections
 *  times every segment in the hour, for lines beside the one being navigated
 *  by; they come from the snapshot under the cursor instead. */
export const TIMELINE_REQUESTS: readonly SectionRequest[] = [{ section: "health" }]

// The link to a monitored host is the slow part: a request costs about a second
// of latency whatever it returns, while the host answers one in a quarter of
// that on a single core. Deep enough to hide the latency, shallow enough not to
// queue behind that core.
const REQUEST_CONCURRENCY = 8

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

export interface TimelineData {
  readonly hour: number
  readonly availableHours: readonly number[]
  readonly segments: readonly SegmentBound[]
  readonly health: readonly DataRow[]
  readonly points: readonly Point[]
  readonly findings: readonly Finding[]
  readonly sourceFamilies: readonly SourceFamily[]
  readonly availableSections: readonly string[]
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

export async function loadHour(
  start: number,
  signal: AbortSignal,
  wanted?: readonly SectionRequest[],
  onlySegments?: readonly string[],
  kinds: readonly LoadTask["kind"][] = ["history", "index"],
): Promise<HourData> {
  const fixture = bundledFixtureHour(start)
  if (fixture !== null) return fixture
  const end = start + 3_600_000_000
  const catalog = await request(`/api/catalog?from=${start}&to=${end - 1}`, signal)
  const chosen = onlySegments === undefined ? null : new Set(onlySegments)
  const segments = catalogSegments(catalog).filter(
    (segment) => Number(segment.min_ts) < end && Number(segment.max_ts) >= start
      && (chosen === null || chosen.has(segment.id)),
  )
  const sourceFamilies = catalog
    .find((record) => record.record === "catalog")?.source_families as readonly SourceFamily[] | undefined
  const tasks = segments
    .flatMap((segment) => segmentTasks(segment, wanted))
    .filter((task) => kinds.includes(task.kind))
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

/** Add a later view's sections to what is already on screen. The caller only
 *  ever asks for names it does not hold, so rows never arrive twice. */
/** The hour as the rest of the screen holds it. The line goes in beside every
 *  other section, or the first snapshot to arrive merges it away. */
export function hourOf(timeline: TimelineData): HourData {
  return hourData({
    sections: { health: timeline.health },
    availableSections: timeline.availableSections,
    points: timeline.points,
    findings: timeline.findings,
    sourceFamilies: timeline.sourceFamilies,
    segmentCount: timeline.segments.length,
  })
}

export function mergeHourData(before: HourData, after: HourData): HourData {
  const sections: Record<string, readonly DataRow[]> = { ...before.sections }
  for (const [name, rows] of Object.entries(after.sections)) {
    sections[name] = [...(sections[name] ?? []), ...rows]
  }
  return hourData({
    sections,
    availableSections: unique([...before.availableSections, ...after.availableSections]),
    points: [...before.points, ...after.points].sort(pointOrder),
    findings: [...before.findings, ...after.findings].sort(findingOrder),
    sourceFamilies: after.sourceFamilies.length === 0 ? before.sourceFamilies : after.sourceFamilies,
    segmentCount: Math.max(before.segmentCount, after.segmentCount),
  })
}

export interface SegmentBound {
  readonly id: string
  readonly minTs: number
  readonly maxTs: number
}

/** The whole timeline of one hour in one request: which segments it touches,
 *  the health line and the marks. A round trip over the debugging link costs
 *  more than a second, so the count of requests is what the wait is made of. */
export async function loadTimeline(start: number | null, signal: AbortSignal): Promise<TimelineData> {
  const fixture = bundledFixtureHour(start ?? bundledFixtureRange()?.from ?? 0)
  const range = bundledFixtureRange()
  if (fixture !== null && range !== null) {
    return {
      hour: floorHour(range.from), availableHours: unique([floorHour(range.from), floorHour(range.to)]),
      segments: [], health: fixture.health, points: fixture.points,
      findings: fixture.findings, sourceFamilies: fixture.sourceFamilies,
      availableSections: fixture.availableSections,
    }
  }
  const window = start === null ? "" : `?from=${start}&to=${start + 3_600_000_000 - 1}`
  const records = await request(`/api/hour${window}`, signal)
  const header = records.find((record) => record.record === "hour")
  const hour = header?.from === null || header?.from === undefined
    ? floorHour(Date.now() * 1_000)
    : integer(header.from, "hour start")
  const end = hour + 3_600_000_000
  const all = catalogSegments(records)
  const segments = all
    .filter((segment) => Number(segment.min_ts) < end && Number(segment.max_ts) >= hour)
    .map((segment) => ({ id: segment.id, minTs: Number(segment.min_ts), maxTs: Number(segment.max_ts) }))
    .sort((left, right) => left.minTs - right.minTs)
  const points: Point[] = []
  const findings: Finding[] = []
  let segmentId = ""
  for (const record of records) {
    if (record.record === "index") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "index segment id")
    } else if (record.record === "point") {
      points.push(indexPoint(record, segmentId, HEALTH))
    } else if (isFindingRecord(record)) {
      findings.push(indexFinding(record, segmentId, HEALTH))
    }
  }
  return {
    hour,
    availableHours: ((header?.available_hours ?? []) as readonly string[])
      .map((value) => integer(value, "available hour")),
    segments,
    health: healthRows(points),
    points,
    findings,
    sourceFamilies: sourceFamiliesOf(records),
    availableSections: availableSectionNames(all),
  }
}

/** The line reads a row per moment, and the index carries a point per series,
 *  so the series of one moment become the fields of one row. */
function healthRows(points: readonly Point[]): readonly DataRow[] {
  const byMoment = new Map<string, DataRow & { values: Record<string, Cell> }>()
  for (const point of points) {
    const key = `${point.segmentId}:${point.timestamp}`
    const stored = byMoment.get(key) ?? {
      segmentId: point.segmentId,
      logicalName: HEALTH,
      typeId: point.typeId,
      ordinal: key,
      timestamp: point.timestamp,
      values: {},
    }
    stored.values[point.series] = point.value
    byMoment.set(key, stored)
  }
  return [...byMoment.values()].sort((left, right) => left.timestamp - right.timestamp)
}

/** The segments an hour touches, in order. Held for the hour so that moving the
 *  cursor costs no request of its own. */
export async function listSegments(start: number, signal: AbortSignal): Promise<readonly SegmentBound[]> {
  const end = start + 3_600_000_000
  const catalog = await request(`/api/catalog?from=${start}&to=${end - 1}`, signal)
  return catalogSegments(catalog)
    .filter((segment) => Number(segment.min_ts) < end && Number(segment.max_ts) >= start)
    .map((segment) => ({ id: segment.id, minTs: Number(segment.min_ts), maxTs: Number(segment.max_ts) }))
    .sort((left, right) => left.minTs - right.minTs)
}

/** The segment holding a moment, or the last one before it. A table shows one
 *  snapshot, so it needs one segment and not the hour around it. */
export function segmentAt(segments: readonly SegmentBound[], at: number): string | null {
  const holding = segments.find((segment) => segment.minTs <= at && segment.maxTs >= at)
  return holding?.id ?? segments.at(-1)?.id ?? null
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
  | {
    readonly kind: "history"
    readonly segmentId: string
    readonly logicalName: string
    readonly fields?: readonly string[]
  }
  | { readonly kind: "index"; readonly segmentId: string; readonly logicalName: string }

type LoadResult =
  | { readonly kind: "history"; readonly logicalName: string; readonly rows: readonly DataRow[] }
  | { readonly kind: "index"; readonly points: readonly Point[]; readonly findings: readonly Finding[] }

function segmentTasks(segment: Segment, wanted?: readonly SectionRequest[]): readonly LoadTask[] {
  const keep = wanted === undefined ? null : new Map(wanted.map((request) => [request.section, request.fields]))
  const names = segmentSectionNames(segment).filter((name) => keep === null || keep.has(name))
  const tasks: LoadTask[] = names.map((logicalName) => {
    const fields = keep?.get(logicalName)
    return fields === undefined
      ? { kind: "history" as const, segmentId: segment.id, logicalName }
      : { kind: "history" as const, segmentId: segment.id, logicalName, fields }
  })
  // An index resource builds its file when one is not on disk, and the root
  // takes a single writer, so asking every segment for its marks at once waits
  // out every build. Callers take the history kind first and come back.
  for (const logicalName of names) tasks.push({ kind: "index", segmentId: segment.id, logicalName })
  if ((keep === null || keep.has("health")) && segmentSectionNames(segment).some((name) => name.startsWith("os_"))) {
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
      task.fields ?? fieldsForLogicalName(task.logicalName),
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

/** One request for a moment across several sections. Counters arrive already
 *  divided by the interval, so nothing is subtracted here. */
/** A statement table on a busy server is thousands of rows and megabytes of
 *  query text, and a screen holds a few dozen. The server orders and cuts. */
const SECTION_ORDER: Readonly<Record<string, { readonly by: string; readonly top: number }>> = {
  pg_stat_statements: { by: "total_exec_time", top: 200 },
}

function orderFor(section: string | undefined): readonly string[] {
  const order = section === undefined ? undefined : SECTION_ORDER[section]
  return order === undefined ? [] : [`by=${encodeURIComponent(order.by)}`, `top=${order.top}`]
}

export async function loadSnapshot(
  segmentId: string,
  at: number,
  sections: readonly string[],
  signal: AbortSignal,
): Promise<HourData> {
  const query = [
    `at=${at}`,
    ...sections.map((name) => `section=${encodeURIComponent(name)}`),
    ...(sections.length === 1 ? orderFor(sections[0]) : []),
  ].join("&")
  const records = await request(
    `/api/segments/${encodeURIComponent(segmentId)}/snapshot?${query}`,
    signal,
  )
  const named = new Map<string, string>()
  const grouped: Record<string, DataRow[]> = {}
  for (const name of sections) grouped[name] = []
  for (const record of records) {
    if (record.record === "layout") {
      const layout = record.layout as { readonly type_id: unknown; readonly logical_name: unknown }
      named.set(requiredText(layout.type_id, "layout type_id"), requiredText(layout.logical_name, "logical name"))
    } else if (record.record === "row") {
      const typeId = requiredText(record.type_id, "row type_id")
      const logicalName = named.get(typeId)
      const values = record.values
      if (logicalName === undefined || values === null || typeof values !== "object") {
        throw new Error(`row for layout ${typeId} arrived before its layout`)
      }
      const rows = grouped[logicalName] ?? []
      rows.push({
        segmentId,
        logicalName,
        typeId,
        ordinal: requiredText(record.ordinal, "row ordinal"),
        timestamp: record.timestamp === null ? at : integer(record.timestamp, "row timestamp"),
        values: values as Readonly<Record<string, Cell>>,
      })
      grouped[logicalName] = rows
    }
  }
  return hourData({
    sections: grouped,
    availableSections: sections,
    points: [],
    findings: [],
    sourceFamilies: [],
    segmentCount: 1,
  })
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
    if (record.record === "point") points.push(indexPoint(record, segmentId, logicalName))
    else if (isFindingRecord(record)) findings.push(indexFinding(record, segmentId, logicalName))
  }
  return { points, findings }
}

const HEALTH = "health"

function isFindingRecord(record: Record<string, unknown>): boolean {
  return record.record === "finding"
    && (record.kind === "known_bad" || record.kind === "spike" || record.kind === "event")
}

function indexPoint(record: Record<string, unknown>, segmentId: string, logicalName: string): Point {
  return {
    segmentId,
    logicalName,
    typeId: requiredText(record.type_id, "point type_id"),
    series: requiredText(record.series, "point series"),
    timestamp: integer(record.ts, "point timestamp"),
    identity: cellRecord(record.identity),
    value: record.value === null ? null : finiteNumber(record.value, "point value"),
  }
}

function indexFinding(record: Record<string, unknown>, segmentId: string, logicalName: string): Finding {
  const typeId = requiredText(record.type_id, "finding type_id")
  return {
    segmentId,
    logicalName: logicalNameForTypeId(typeId) ?? logicalName,
    kind: record.kind as Finding["kind"],
    typeId,
    timestamp: integer(record.ts, "finding timestamp"),
    category: typeof record.category === "number" ? record.category : null,
    rowOrdinal: requiredText(record.row_ordinal, "finding row ordinal"),
    fieldOrdinal: integer(record.field_ordinal, "finding field ordinal"),
  }
}

function sourceFamiliesOf(records: readonly Record<string, unknown>[]): readonly SourceFamily[] {
  const found = records.find((record) => record.record === "catalog")?.source_families
  return (found as readonly SourceFamily[] | undefined) ?? []
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
