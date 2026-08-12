import { registry } from "kronika:registry"

import { bundledFixtureHour, bundledFixtureRange } from "./fixture"
import { rowMatchesLocator } from "./locator"
import { decoratePostgresIntervalRow, intervalMetric, postgresIdentity, supportsPostgresDerivedOrder } from "./postgres-metrics"
import { apiFetch } from "./session"
import { readNdjson } from "./wire"

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
  postgresqlPlans: ["pg_store_plans", "pg_store_plans_info"] as const,
  postgresqlLocks: ["pg_locks"] as const,
  postgresqlDatabases: ["pg_stat_database"] as const,
  events: REGISTRY_LOGICAL_NAMES.filter((name) => name.startsWith("pg_log_") || name === "pgbouncer_events"),
} as const

const UI_SECTION_NAMES = unique(Object.values(PRODUCT_SECTION_GROUPS).flat())
const UI_SECTION_NAME_SET = new Set(UI_SECTION_NAMES)

export interface SectionRequest {
  readonly section: string
  readonly fields?: readonly string[]
  readonly typeIds?: readonly string[]
  readonly fieldsByType?: Readonly<Record<string, readonly string[]>>
  readonly typeId?: string
  readonly pageSize?: number
  readonly defaultOrder?: readonly string[]
  readonly order?: Readonly<Record<string, readonly string[]>>
  readonly fallbackOrder?: readonly string[]
}

export const POSTGRESQL_OVERVIEW_REQUESTS: readonly SectionRequest[] = [
  ...POSTGRESQL_OVERVIEW.map((section) => ({ section })),
  { section: "pg_stat_activity", fields: ["state", "wait_event"] },
  { section: "pg_stat_database" },
  { section: "pg_locks", fields: ["pid"] },
]

export const TIMELINE_REQUESTS: readonly SectionRequest[] = [{ section: "health" }]

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

export interface FindingGroup {
  readonly segmentId: string
  readonly logicalName: string
  readonly typeId: string
  readonly totalHits: number
  readonly shown: number
  readonly truncated: boolean
}

interface Section {
  readonly logical_name: string | null
  readonly type_id: string
}

interface Segment {
  readonly id: string
  readonly min_ts: string
  readonly max_ts: string
  readonly sections: readonly Section[]
}

export interface HourData {
  readonly sections: Readonly<Record<string, readonly DataRow[]>>
  readonly rateColumns: Readonly<Record<string, readonly string[]>>
  readonly snapshotRows: readonly SnapshotRows[]
  readonly availableSections: readonly string[]
  readonly processes: readonly DataRow[]
  readonly activities: readonly DataRow[]
  readonly load: readonly DataRow[]
  readonly memory: readonly DataRow[]
  readonly pressure: readonly DataRow[]
  readonly health: readonly DataRow[]
  readonly pgOverview: readonly DataRow[]
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly findings: readonly Finding[]
  readonly findingGroups: readonly FindingGroup[]
}

export interface SnapshotRows {
  readonly logicalName: string
  readonly eligible: number
  readonly returned: number
  readonly hasMore: boolean
  readonly truncated: boolean
  readonly nextCursor: string | null
  readonly pageSize: number
  readonly orderBy: readonly string[]
  readonly orderDirection: "desc"
  readonly from: number | null
  readonly to: number | null
}

export function snapshotRowKey(row: DataRow): string {
  return `${row.segmentId}:${row.typeId}:${row.ordinal}`
}

export function appendSnapshotRows(
  current: readonly DataRow[],
  incoming: readonly DataRow[],
): readonly DataRow[] {
  const seen = new Set(current.map(snapshotRowKey))
  return [...current, ...incoming.filter((row) => {
    const key = snapshotRowKey(row)
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })]
}

export function mergeSnapshotData(current: HourData, incoming: HourData, appendSection?: string): HourData {
  const sections = { ...current.sections, ...incoming.sections }
  if (appendSection !== undefined) {
    sections[appendSection] = appendSnapshotRows(
      current.sections[appendSection] ?? [],
      incoming.sections[appendSection] ?? [],
    )
  }
  return hourData({
    sections,
    rateColumns: { ...current.rateColumns, ...incoming.rateColumns },
    snapshotRows: incoming.snapshotRows.length === 0 ? current.snapshotRows : incoming.snapshotRows,
    availableSections: unique([...current.availableSections, ...incoming.availableSections]),
    points: [],
    lanePoints: [],
    findings: [],
  })
}

export interface LanePoint {
  readonly segmentId: string
  readonly lane: string
  readonly timestamp: number
  readonly value: number | null
}

export interface TimelineData {
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly lanes: Readonly<Record<string, readonly DataRow[]>>
  readonly availableHours: readonly number[]
  readonly segments: readonly SegmentBound[]
  readonly health: readonly DataRow[]
  readonly points: readonly Point[]
  readonly findings: readonly Finding[]
  readonly findingGroups: readonly FindingGroup[]
  readonly availableSections: readonly string[]
}

export interface ResolvedLocator {
  readonly logicalName: string
  readonly row: DataRow
  readonly fieldName: string | null
}

export const ACTIVITY_FIELDS = [
  "pid", "leader_pid", "datname", "usename", "application_name", "client_addr", "backend_type",
  "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age",
  "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change",
] as const

export function hourOf(timeline: TimelineData): HourData {
  return hourData({
    sections: timeline.lanes,
    availableSections: timeline.availableSections,
    points: timeline.points,
    lanePoints: timeline.lanePoints,
    findings: timeline.findings,
    findingGroups: timeline.findingGroups,
  })
}

export function sampleAt(line: readonly DataRow[], cursor: number): number | null {
  let chosen: number | null = null
  for (const row of line) {
    if (row.timestamp <= cursor && (chosen === null || row.timestamp > chosen)) chosen = row.timestamp
  }
  return chosen ?? (line.length === 0 ? null : Math.min(...line.map((row) => row.timestamp)))
}

export function viewData(timeline: HourData, current: HourData): HourData {
  return hourData({
    sections: { ...timeline.sections, ...current.sections },
    rateColumns: current.rateColumns,
    snapshotRows: current.snapshotRows,
    availableSections: timeline.availableSections,
    points: timeline.points,
    lanePoints: timeline.lanePoints,
    findings: timeline.findings,
    findingGroups: timeline.findingGroups,
  })
}

export interface SegmentBound {
  readonly id: string
  readonly minTs: number
  readonly maxTs: number
  readonly sections: readonly SegmentSection[]
}

export interface SegmentSection {
  readonly logicalName: string
  readonly typeId: string
}

export async function loadTimeline(start: number | null, signal: AbortSignal): Promise<TimelineData> {
  const range = bundledFixtureRange()
  const requested = floorHour(start ?? range?.from ?? 0)
  const fixture = bundledFixtureHour(requested)
  if (fixture !== null && range !== null) {
    return {
      hour: requested, availableHours: unique([floorHour(range.from), floorHour(range.to)]),
      segments: [], lanePoints: fixture.lanePoints, lanes: fixture.sections, health: fixture.health, points: fixture.points,
      findings: fixture.findings,
      findingGroups: fixture.findingGroups,
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
    .map((segment) => ({
      id: segment.id,
      minTs: Number(segment.min_ts),
      maxTs: Number(segment.max_ts),
      sections: segment.sections.flatMap((section) => section.logical_name === null ? [] : [{
        logicalName: section.logical_name,
        typeId: section.type_id,
      }]),
    }))
    .sort((left, right) => left.minTs - right.minTs)
  const points: Point[] = []
  const findings: Finding[] = []
  const findingGroups: FindingGroup[] = []
  const lanePoints: LanePoint[] = []
  const lanes: Record<string, DataRow[]> = { [HEALTH]: [] }
  const layouts = new Map<string, readonly string[]>()
  let segmentId = ""
  for (const record of records) {
    const layout = layoutRecord(record)
    if (record.record === "index") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "index segment id")
    } else if (record.record === "point") {
      points.push(indexPoint(record, segmentId, HEALTH))
    } else if (isFindingRecord(record)) {
      findings.push(indexFinding(record, segmentId, HEALTH))
    } else if (record.record === "findings") {
      const typeId = requiredText(record.type_id, "finding group type_id")
      const logicalName = typeof record.logical_name === "string"
        ? record.logical_name
        : logicalNameForTypeId(typeId) ?? HEALTH
      const totalHits = integer(record.total_hits, "finding hit count")
      if (typeof record.truncated !== "boolean") throw new Error("finding truncation flag is invalid")
      findingGroups.push({
        segmentId,
        logicalName,
        typeId,
        totalHits,
        shown: 0,
        truncated: record.truncated,
      })
    } else if (layout !== null) {
      layouts.set(layout.typeId, layout.columns)
    } else if (record.record === "row") {
      const row = laneRow(record, segmentId, layouts)
      if (row !== null) (lanes[row.logicalName] ??= []).push(row)
    } else if (record.record === "lane") {
      lanePoints.push({
        segmentId: requiredText(record.segment_id, "lane segment id"),
        lane: requiredText(record.lane, "lane name"),
        timestamp: integer(record.ts, "lane timestamp"),
        value: record.value === null ? null : finiteNumber(record.value, "lane value"),
      })
    }
  }
  lanes[HEALTH] = healthRows(points) as DataRow[]
  const resolvedFindingGroups = findingGroups.map((group) => ({
    ...group,
    shown: findings.filter((finding) => finding.segmentId === group.segmentId && finding.typeId === group.typeId).length,
  }))
  return {
    hour,
    lanePoints,
    lanes,
    availableHours: ((header?.available_hours ?? []) as readonly string[])
      .map((value) => integer(value, "available hour")),
    segments,
    health: lanes[HEALTH] ?? [],
    points,
    findings,
    findingGroups: resolvedFindingGroups,
    availableSections: availableSectionNames(all),
  }
}

export async function loadSeries(
  from: number,
  section: string,
  where: Readonly<Record<string, string>>,
  fields: readonly string[],
  signal: AbortSignal,
  typeId?: string | undefined,
  to = from + 3_600_000_000 - 1,
): Promise<readonly DataRow[]> {
  signal.throwIfAborted()
  if (bundledFixtureRange() !== null) {
    const fieldsToKeep = unique([...fields, ...Object.keys(where)])
    const rows: DataRow[] = []
    for (let hour = floorHour(from); hour <= floorHour(to); hour += 3_600_000_000) {
      const fixture = bundledFixtureHour(hour)
      if (fixture !== null) rows.push(...(fixture.sections[section] ?? []))
    }
    return [...new Map(rows.map((row) => [`${row.segmentId}:${row.typeId}:${row.ordinal}`, row])).values()]
      .filter((row) => row.timestamp >= from && row.timestamp <= to)
      .filter((row) => row.typeId === (typeId ?? row.typeId) && fixtureMatches(row, where))
      .map((row) => projectFixtureRow(row, fieldsToKeep))
  }
  const query = [
    `from=${from}`,
    `to=${to}`,
    `section=${encodeURIComponent(section)}`,
    ...fields.map((name) => `field=${encodeURIComponent(name)}`),
    ...Object.entries(where).map(([column, value]) => `where.${encodeURIComponent(column)}=${encodeURIComponent(value)}`),
    ...(typeId === undefined ? [] : [`type_id=${encodeURIComponent(typeId)}`]),
  ].join("&")
  const records = await request(`/api/hour?${query}`, signal)
  const layouts = new Map<string, readonly string[]>()
  const rows: DataRow[] = []
  let segmentId = ""
  for (const record of records) {
    const layout = layoutRecord(record)
    if (record.record === "series_segment") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "series segment id")
    } else if (layout !== null) {
      layouts.set(layout.typeId, layout.columns)
    } else if (record.record === "row") {
      const row = laneRow(record, segmentId, layouts)
      if (row !== null) rows.push(row)
    }
  }
  return rows
}

function laneRow(
  record: Record<string, unknown>,
  segmentId: string,
  layouts: ReadonlyMap<string, readonly string[]>,
): DataRow | null {
  const typeId = requiredText(record.type_id, "row type_id")
  const names = layouts.get(typeId)
  const logicalName = logicalNameForTypeId(typeId)
  const values = record.values
  if (names === undefined || logicalName === null || !Array.isArray(values)) return null
  return {
    segmentId,
    logicalName,
    typeId,
    ordinal: requiredText(record.ordinal, "row ordinal"),
    timestamp: integer(record.timestamp, "row timestamp"),
    values: rowValues(names, values),
  }
}

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
    if (!Object.hasOwn(stored.values, point.series) || point.value === null) {
      stored.values[point.series] = point.value
    }
    byMoment.set(key, stored)
  }
  return [...byMoment.values()].sort((left, right) => left.timestamp - right.timestamp)
}

export function segmentAt(segments: readonly SegmentBound[], at: number): string | null {
  return segmentBoundAt(segments, at)?.id ?? null
}

export function segmentBoundAt(segments: readonly SegmentBound[], at: number): SegmentBound | null {
  return segments.find((segment) => segment.minTs <= at && segment.maxTs >= at)
    ?? segments.filter((segment) => segment.maxTs <= at).at(-1)
    ?? null
}

export function requestsForSegment(
  requests: readonly SectionRequest[],
  segment: SegmentBound,
): readonly SectionRequest[] {
  return requests.flatMap((request) => {
    const typeIds = segment.sections
      .filter((section) => section.logicalName === request.section)
      .map((section) => section.typeId)
      .filter((typeId) => request.typeIds === undefined || request.typeIds.includes(typeId))
    if (typeIds.length === 0) return []
    if (request.fields === undefined && request.fieldsByType === undefined) return [request]
    if (request.fields !== undefined
      && request.fieldsByType === undefined
      && request.typeIds === undefined
      && batchableSnapshotSection(request)) {
      const physical = new Set(typeIds.flatMap((typeId) => REGISTRY_BY_TYPE_ID.get(typeId)?.columns ?? []))
      const fields = unique(request.fields.filter((field) => physical.has(field)))
      return fields.length === 0 ? [] : [{ section: request.section, fields }]
    }
    const global = request.pageSize !== undefined && request.fieldsByType !== undefined
    const { fieldsByType: _fieldsByType, typeIds: _typeIds, ...base } = request
    const groups = global ? [unique(typeIds)] : unique(typeIds).map((typeId) => [typeId])
    return groups.flatMap((types) => {
      const physical = new Set(types.flatMap((typeId) => REGISTRY_BY_TYPE_ID.get(typeId)?.columns ?? []))
      const projection = types.flatMap((typeId) => request.fieldsByType?.[typeId] ?? request.fields ?? [])
      const fields = unique(projection.filter((field) => physical.has(field)))
      // An empty projection would request every column.
      if (fields.length === 0) return []
      const keep = (candidates: readonly string[]) => candidates.filter((field) => field.startsWith("derived.")
        ? types.some((typeId) => supportsPostgresDerivedOrder(typeId, field))
        : physical.has(field))
      const order = request.order === undefined
        ? undefined
        : Object.fromEntries(Object.entries(request.order).map(([name, candidates]) => [name, keep(candidates)]))
      return [{
        ...base,
        ...(global ? {} : { typeId: types[0] }),
        fields,
        ...(request.defaultOrder === undefined ? {} : { defaultOrder: keep(request.defaultOrder) }),
        ...(order === undefined ? {} : { order }),
        ...(request.fallbackOrder === undefined ? {} : { fallbackOrder: keep(request.fallbackOrder) }),
      }]
    })
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
  if (locator.typeId === "0") {
    if (locator.fieldOrdinal === 0) return "os_health"
    if (locator.fieldOrdinal === 1) return "overall_health"
    return null
  }
  return REGISTRY_BY_TYPE_ID.get(locator.typeId)?.columns[locator.fieldOrdinal] ?? null
}

export function resolveLoadedRow(
  data: Pick<HourData, "sections">,
  locator: Pick<Finding, "segmentId" | "typeId" | "rowOrdinal" | "timestamp">,
): DataRow | null {
  const logicalName = logicalNameForTypeId(locator.typeId)
  if (logicalName === null) return null
  return (data.sections[logicalName] ?? []).find((row) => rowMatchesLocator(row, locator)) ?? null
}

export function resolveLocator(data: Pick<HourData, "sections">, finding: Finding): ResolvedLocator | null {
  const fieldName = fieldNameForLocator(finding)
  const row = finding.typeId === "0"
    ? (data.sections.health ?? []).find((candidate) => candidate.segmentId === finding.segmentId
      && candidate.timestamp === finding.timestamp
      && fieldName !== null
      && Object.hasOwn(candidate.values, fieldName)) ?? null
    : resolveLoadedRow(data, finding)
  if (row === null) return null
  return {
    logicalName: row.logicalName,
    row,
    fieldName,
  }
}

function hourData(input: {
  readonly sections: Readonly<Record<string, readonly DataRow[]>>
  readonly rateColumns?: Readonly<Record<string, readonly string[]>>
  readonly snapshotRows?: readonly SnapshotRows[]
  readonly availableSections: readonly string[]
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly findings: readonly Finding[]
  readonly findingGroups?: readonly FindingGroup[]
}): HourData {
  const rows = (name: string) => input.sections[name] ?? []
  const flatten = (names: readonly string[]) => names.flatMap(rows)
  return {
    ...input,
    rateColumns: input.rateColumns ?? {},
    snapshotRows: input.snapshotRows ?? [],
    findingGroups: input.findingGroups ?? [],
    processes: rows("os_process"),
    activities: rows("pg_stat_activity"),
    load: rows("os_loadavg"),
    memory: rows("os_meminfo"),
    pressure: rows("os_psi"),
    health: rows("health"),
    pgOverview: flatten(PRODUCT_SECTION_GROUPS.postgresqlOverview),
  }
}

const CELL_TEXT = 160

export interface SnapshotOptions {
  readonly filters?: Readonly<Record<string, string>>
  readonly typeId?: string
  readonly rowOrdinal?: string
  readonly fullText?: boolean
  readonly cursor?: string
  readonly search?: readonly string[]
}

interface SnapshotOrder {
  readonly column: string
  readonly descending: boolean
}

export async function loadSnapshot(
  segmentId: string,
  at: number,
  sections: readonly (SectionRequest | string)[],
  signal: AbortSignal,
  order?: SnapshotOrder | undefined,
  options?: SnapshotOptions | Readonly<Record<string, string>> | undefined,
): Promise<HourData> {
  const requests = sections.map((section) => typeof section === "string" ? { section } : section)
  const chosen = snapshotOptions(options)
  if (requests.length === 0) return emptyHour()
  if ((chosen.filters !== undefined || chosen.typeId !== undefined || chosen.rowOrdinal !== undefined
      || chosen.cursor !== undefined || chosen.search !== undefined)
    && requests.length !== 1) {
    throw new Error("a filtered, searched, paged, or exact snapshot needs one section")
  }
  if (requests.some((request) => request.pageSize !== undefined) && requests.length !== 1) {
    throw new Error("a paged snapshot needs one section")
  }
  if (chosen.rowOrdinal !== undefined && chosen.typeId === undefined && requests[0]?.typeId === undefined) {
    throw new Error("an exact snapshot row needs typeId")
  }
  if (chosen.rowOrdinal !== undefined && chosen.filters !== undefined) {
    throw new Error("an exact snapshot row cannot carry filters")
  }
  if (chosen.rowOrdinal !== undefined && (chosen.cursor !== undefined || chosen.search !== undefined)) {
    throw new Error("an exact snapshot row cannot carry paging or search")
  }
  if (chosen.cursor !== undefined && requests[0]?.pageSize === undefined) {
    throw new Error("a snapshot cursor needs a paged section")
  }
  for (const section of requests) {
    if (section.fieldsByType !== undefined) {
      throw new Error(`the ${section.section} projection needs the cursor segment`)
    }
    if (section.typeId !== undefined && chosen.typeId !== undefined && section.typeId !== chosen.typeId) {
      throw new Error(`the ${section.section} snapshot has conflicting type IDs`)
    }
    if (section.fields !== undefined && section.fields.length === 0) {
      throw new Error(`the ${section.section} projection has no physical fields`)
    }
  }
  signal.throwIfAborted()
  const fixture = fixtureSnapshot(segmentId, at, requests, chosen, order)
  if (fixture !== null) return fixture
  const whole = requests.filter((section) => batchableSnapshotSection(section) && section.fields === undefined)
  const projected = requests.filter((section) => batchableSnapshotSection(section) && section.fields !== undefined)
  const individual = requests.filter((section) => !batchableSnapshotSection(section))
  const batches = chosen.filters !== undefined || chosen.typeId !== undefined || chosen.rowOrdinal !== undefined
    ? requests.map((section) => [section] as const)
    : [
        ...(whole.length === 0 ? [] : [whole]),
        ...(projected.length === 0 ? [] : [projected]),
        ...individual.map((section) => [section] as const),
      ]
  const responses = await Promise.all(batches.map(async (batch) => {
    const query = snapshotQuery(at, batch, order, chosen)
    return request(`/api/segments/${encodeURIComponent(segmentId)}/snapshot?${query}`, signal)
  }))
  const records = responses.flat()
  const layouts = new Map<string, { readonly logicalName: string; readonly columns: readonly string[] }>()
  const grouped: Record<string, DataRow[]> = {}
  const rateColumns: Record<string, readonly string[]> = {}
  const snapshotRows: SnapshotRows[] = []
  for (const section of requests) grouped[section.section] = []
  for (const record of records) {
    const layout = layoutRecord(record)
    if (layout !== null) {
      const logicalName = requiredText(layout.logicalName, "logical name")
      layouts.set(layout.typeId, {
        logicalName,
        columns: layout.columns,
      })
      if (Array.isArray(record.rates)) {
        rateColumns[logicalName] = unique([
          ...(rateColumns[logicalName] ?? []),
          ...record.rates.map((name) => requiredText(name, "rate column")),
        ])
      }
    } else if (record.record === "row") {
      const typeId = requiredText(record.type_id, "row type_id")
      const layout = layouts.get(typeId)
      const values = record.values
      if (layout === undefined || !Array.isArray(values)) {
        throw new Error(`row for layout ${typeId} arrived before its layout`)
      }
      const { columns, logicalName } = layout
      const rows = grouped[logicalName] ?? []
      rows.push({
        segmentId,
        logicalName,
        typeId,
        ordinal: requiredText(record.ordinal, "row ordinal"),
        timestamp: record.timestamp === null ? at : integer(record.timestamp, "row timestamp"),
        values: rowValues(columns, values),
      })
      grouped[logicalName] = rows
    } else if (record.record === "snapshot_page") {
      const logicalName = requiredText(record.logical_name, "snapshot page logical name")
      if (record.order_direction !== "desc"
        || typeof record.has_more !== "boolean"
        || typeof record.truncated !== "boolean"
        || !Array.isArray(record.order_by)
        || (record.next_cursor !== null && typeof record.next_cursor !== "string")) {
        throw new Error(`snapshot page for ${logicalName} is invalid`)
      }
      snapshotRows.push({
        logicalName,
        eligible: integer(record.eligible, "eligible row count"),
        returned: integer(record.returned, "returned row count"),
        hasMore: record.has_more,
        truncated: record.truncated,
        nextCursor: record.next_cursor,
        pageSize: integer(record.page_size, "snapshot page size"),
        orderBy: record.order_by.map((field) => requiredText(field, "snapshot order field")),
        orderDirection: record.order_direction,
        from: record.from === null ? null : integer(record.from, "snapshot interval start"),
        to: record.to === null ? null : integer(record.to, "snapshot interval end"),
      })
    }
  }
  return hourData({
    sections: grouped,
    rateColumns,
    snapshotRows,
    availableSections: unique(requests.map((section) => section.section)),
    points: [],
    lanePoints: [],
    findings: [],
    findingGroups: [],
  })
}

function rowValues(columns: readonly string[], cells: readonly unknown[]): Readonly<Record<string, Cell>> {
  return Object.fromEntries(columns.flatMap((name, index) => name === "ts"
    ? []
    : [[name, index < cells.length ? cells[index] as Cell : null]]))
}

function fixtureSnapshot(
  segmentId: string,
  at: number,
  requests: readonly SectionRequest[],
  options: SnapshotOptions,
  order: SnapshotOrder | undefined,
): HourData | null {
  const fixture = bundledFixtureHour(floorHour(at))
  if (fixture === null) return null
  const grouped: Record<string, readonly DataRow[]> = {}
  const snapshotRows: SnapshotRows[] = []
  for (const request of requests) {
    const typeId = request.typeId ?? options.typeId
    const sourceRows = fixture.sections[request.section] ?? []
    let rows = sourceRows
      .filter((row) => row.segmentId === segmentId)
      .filter((row) => typeId === undefined || row.typeId === typeId)
      .filter((row) => fixtureMatches(row, options.filters ?? {}))
      .filter((row) => options.rowOrdinal === undefined
        || (row.ordinal === options.rowOrdinal && row.timestamp === at))
    if (options.rowOrdinal === undefined && rows.length !== 0) {
      const before = rows.filter((row) => row.timestamp <= at)
      rows = before.length === 0
        ? []
        : rows.filter((row) => row.timestamp === Math.max(...before.map((row) => row.timestamp)))
    }
    const eligible = rows.length
    const orderFields = snapshotOrder(request, order)
    const orderField = orderFields.find((field) => field.startsWith("derived.") || rows.some((row) => row.values[field] !== undefined))
    if (orderField !== undefined) {
      const value = (row: DataRow) => orderField.startsWith("derived.")
        ? fixtureDerivedOrderValue(row, sourceRows, orderField.slice("derived.".length))
        : row.values[orderField]
      rows = rows.slice().sort((left, right) => fixtureOrder(value(right), value(left)))
    }
    if (request.pageSize !== undefined) rows = rows.slice(0, request.pageSize)
    if (request.pageSize !== undefined && orderField !== undefined) {
      if ((request.typeId ?? options.typeId ?? rows[0]?.typeId) !== undefined) snapshotRows.push({
        logicalName: request.section,
        eligible,
        returned: rows.length,
        hasMore: eligible > rows.length,
        truncated: eligible > rows.length,
        nextCursor: null,
        pageSize: request.pageSize,
        orderBy: [orderField.replace(/^derived\./, "")],
        orderDirection: "desc",
        from: null,
        to: rows[0]?.timestamp ?? null,
      })
    }
    const fields = request.fields === undefined
      ? undefined
      : unique([...request.fields, ...Object.keys(options.filters ?? {})])
    const selected = fields === undefined
      ? rows
      : rows.map((row) => projectFixtureRow(row, fields))
    grouped[request.section] = [...(grouped[request.section] ?? []), ...selected]
  }
  return hourData({
    sections: grouped,
    rateColumns: {},
    snapshotRows,
    availableSections: unique(requests.map((request) => request.section)),
    points: [],
    lanePoints: [],
    findings: [],
    findingGroups: [],
  })
}

export function fixtureDerivedOrderValue(row: DataRow, rows: readonly DataRow[], field: string): Cell {
  if (field === "cv") return decoratePostgresIntervalRow(row).values.cv ?? null
  const identity = postgresIdentity(row.typeId)
  const before = rows
    .filter((candidate) => candidate.typeId === row.typeId && candidate.timestamp < row.timestamp
      && identity.every((name) => JSON.stringify(candidate.values[name]) === JSON.stringify(row.values[name])))
    .sort((left, right) => left.timestamp - right.timestamp)
    .at(-1)
  if (before === undefined) return null
  const values = Object.fromEntries(Object.keys(row.values).map((name) => [name, intervalMetric(before, row, name)]))
  return decoratePostgresIntervalRow({ ...row, values }).values[field] ?? null
}

function fixtureMatches(row: DataRow, filters: Readonly<Record<string, string>>): boolean {
  return Object.entries(filters).every(([field, expected]) => fixtureText(row.values[field]) === expected)
}

function fixtureText(cell: Cell | undefined): string | null {
  return typeof cell === "string" || typeof cell === "number" || typeof cell === "boolean"
    ? String(cell)
    : null
}

function fixtureOrder(left: Cell | undefined, right: Cell | undefined): number {
  const leftNumber = typeof left === "number" ? left : typeof left === "string" ? Number(left) : Number.NaN
  const rightNumber = typeof right === "number" ? right : typeof right === "string" ? Number(right) : Number.NaN
  if (Number.isFinite(leftNumber) && Number.isFinite(rightNumber)) return leftNumber - rightNumber
  return (fixtureText(left) ?? "").localeCompare(fixtureText(right) ?? "")
}

function projectFixtureRow(row: DataRow, fields: readonly string[]): DataRow {
  return {
    ...row,
    values: Object.fromEntries(fields.flatMap((field) => Object.hasOwn(row.values, field)
      ? [[field, row.values[field] ?? null]]
      : [])),
  }
}

function batchableSnapshotSection(section: SectionRequest): boolean {
  return section.typeId === undefined
    && section.pageSize === undefined
    && section.defaultOrder === undefined
    && section.order === undefined
    && section.fallbackOrder === undefined
}

function snapshotQuery(
  at: number,
  sections: readonly SectionRequest[],
  order: SnapshotOrder | undefined,
  options: SnapshotOptions,
): string {
  const section = sections.length === 1 ? sections[0] : undefined
  const typeId = section?.typeId ?? options.typeId
  const fields = section?.fields ?? unique(sections.flatMap((request) => request.fields ?? []))
  const ordered = section === undefined || options.rowOrdinal !== undefined
    ? []
    : snapshotOrder(section, order)
  return [
    `at=${at}`,
    ...sections.map((request) => `section=${encodeURIComponent(request.section)}`),
    ...fields.map((field) => `field=${encodeURIComponent(field)}`),
    ...ordered.map((field) => `by=${encodeURIComponent(field)}`),
    ...(section?.pageSize === undefined || options.rowOrdinal !== undefined ? [] : [`page_size=${section.pageSize}`]),
    ...(options.fullText === true ? [] : [`text=${CELL_TEXT}`]),
    ...(options.cursor === undefined ? [] : [`cursor=${encodeURIComponent(options.cursor)}`]),
    ...(options.search ?? []).map((pattern) => `search=${encodeURIComponent(pattern)}`),
    ...Object.entries(options.filters ?? {}).map(([column, value]) =>
      `where.${encodeURIComponent(column)}=${encodeURIComponent(value)}`),
    ...(typeId === undefined ? [] : [`type_id=${encodeURIComponent(typeId)}`]),
    ...(options.rowOrdinal === undefined ? [] : [`row_ordinal=${encodeURIComponent(options.rowOrdinal)}`]),
  ].join("&")
}

function snapshotOrder(section: SectionRequest, chosen: SnapshotOrder | undefined): readonly string[] {
  const requested = chosen !== undefined
    ? section.order === undefined
      ? section.fields?.includes(chosen.column) === true ? [chosen.column] : undefined
      : section.order[chosen.column]
    : section.defaultOrder
  if (requested !== undefined && requested.length > 0) return unique(requested)
  if (chosen !== undefined && section.defaultOrder !== undefined && section.defaultOrder.length > 0) {
    return unique(section.defaultOrder)
  }
  return unique(section.fallbackOrder ?? [])
}

function snapshotOptions(
  value: SnapshotOptions | Readonly<Record<string, string>> | undefined,
): SnapshotOptions {
  if (value === undefined) return {}
  if ("filters" in value || "typeId" in value || "rowOrdinal" in value || "fullText" in value
    || "cursor" in value || "search" in value) {
    return value as SnapshotOptions
  }
  return { filters: value as Readonly<Record<string, string>> }
}

function emptyHour(): HourData {
  return hourData({
    sections: {},
    rateColumns: {},
    availableSections: [],
    points: [],
    lanePoints: [],
    findings: [],
  })
}

const HEALTH = "health"

function isFindingRecord(record: Record<string, unknown>): boolean {
  return record.record === "finding"
    && (record.kind === "known_bad" || record.kind === "spike" || record.kind === "event")
}

function indexPoint(record: Record<string, unknown>, segmentId: string, logicalName: string): Point {
  const typeId = requiredText(record.type_id, "point type_id")
  return {
    segmentId,
    logicalName: logicalNameForTypeId(typeId) ?? logicalName,
    typeId,
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
    logicalName: typeof record.logical_name === "string"
      ? record.logical_name
      : logicalNameForTypeId(typeId) ?? logicalName,
    kind: record.kind as Finding["kind"],
    typeId,
    timestamp: integer(record.ts, "finding timestamp"),
    category: typeof record.category === "number" ? record.category : null,
    rowOrdinal: requiredText(record.row_ordinal, "finding row ordinal"),
    fieldOrdinal: integer(record.field_ordinal, "finding field ordinal"),
  }
}

function layoutRecord(record: Record<string, unknown>): {
  readonly typeId: string
  readonly logicalName: unknown
  readonly columns: readonly string[]
} | null {
  if (record.record !== "layout") return null
  const layout = record.layout as {
    readonly type_id: unknown
    readonly logical_name?: unknown
    readonly columns?: readonly { readonly name: unknown }[]
  }
  const typeId = requiredText(layout.type_id, "layout type_id")
  if (!Array.isArray(layout.columns)) throw new Error(`layout ${typeId} has no columns`)
  return {
    typeId,
    logicalName: layout.logical_name,
    columns: layout.columns.map((column) => requiredText(column.name, "column name")),
  }
}

async function request(path: string, signal: AbortSignal): Promise<readonly Record<string, unknown>[]> {
  const response = await apiFetch(path, { headers: { Accept: "application/x-ndjson" }, signal })
  if (!response.ok) {
    throw new Error(`HTTP ${response.status} for ${path}`)
  }
  return readNdjson(response, path, signal)
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

function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}
