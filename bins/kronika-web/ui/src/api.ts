import { registry } from "kronika:registry"

import { bundledFixtureHour, bundledFixtureRange } from "./fixture"
import { rowMatchesLocator } from "./locator"
import { decoratePostgresIntervalRow, intervalMetric, postgresIdentity, supportsPostgresDerivedOrder, unique } from "./postgres-metrics"
import { parseRelationLayout, parseRelationRow, relationGroup, relationLayoutKey, relationRateFields, relationRowKey, type RelationGroup, type RelationLayout, type RelationRow } from "./postgres-relations"
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
  "pg_stat_io",
  "pg_prepared_xacts",
  "pg_stat_statements_info",
] as const

export const PRODUCT_SECTION_GROUPS = {
  host: REGISTRY_LOGICAL_NAMES.filter((name) => name === "instance_metadata" || name.startsWith("os_")),
  postgresqlOverview: [...POSTGRESQL_OVERVIEW, "pg_wal_storage"] as const,
  postgresqlSettings: ["pg_settings"] as const,
  postgresqlActivity: ["pg_stat_activity", "pg_stat_progress_vacuum"] as const,
  postgresqlStatements: ["pg_stat_statements"] as const,
  postgresqlPlans: ["pg_store_plans", "pg_store_plans_info"] as const,
  postgresqlLocks: ["pg_locks"] as const,
  postgresqlDatabases: ["pg_stat_database"] as const,
  postgresqlTables: ["pg_stat_user_tables"] as const,
  postgresqlIndexes: ["pg_stat_user_indexes"] as const,
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
  readonly group?: RelationGroup
  readonly filters?: Readonly<Record<string, string>>
}

export const POSTGRESQL_OVERVIEW_REQUESTS: readonly SectionRequest[] = [
  ...POSTGRESQL_OVERVIEW.map((section) => ({ section })),
  { section: "pg_wal_storage", fields: ["wal_files_bytes"] },
  { section: "pg_stat_activity", fields: ["state", "wait_event", "backend_type"] },
  { section: "pg_stat_database" },
  { section: "pg_locks", fields: ["pid"] },
]

export const POSTGRESQL_CONTEXT_REQUESTS: readonly SectionRequest[] = [
  { section: "pg_settings", fields: ["name", "setting"] },
]

export const TIMELINE_REQUESTS: readonly SectionRequest[] = [{ section: "health" }]

export interface DataRow {
  readonly segmentId: string
  readonly logicalName: string
  readonly typeId: string
  readonly ordinal: string
  readonly timestamp: number
  readonly values: Readonly<Record<string, Cell>>
  readonly relation?: RelationRow
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
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
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
  readonly orderDirection: "asc" | "desc"
  readonly from: number | null
  readonly to: number | null
  readonly group?: RelationGroup
}

export function snapshotRowKey(row: DataRow): string {
  if (row.relation !== undefined) return `${row.segmentId}:${relationRowKey(row)}`
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
  const sections: Record<string, readonly DataRow[]> = { ...current.sections }
  for (const [logicalName, rows] of Object.entries(incoming.sections)) {
    sections[logicalName] = appendSnapshotRows(current.sections[logicalName] ?? [], rows)
  }
  if (appendSection !== undefined && incoming.sections[appendSection] === undefined) {
    sections[appendSection] = current.sections[appendSection] ?? []
  }
  const rateColumns: Record<string, readonly string[]> = { ...current.rateColumns }
  for (const [logicalName, columns] of Object.entries(incoming.rateColumns)) {
    rateColumns[logicalName] = unique([...(rateColumns[logicalName] ?? []), ...columns])
  }
  return hourData({
    sections,
    rateColumns,
    snapshotRows: incoming.snapshotRows.length === 0 ? current.snapshotRows : incoming.snapshotRows,
    availableSections: unique([...current.availableSections, ...incoming.availableSections]),
    postgresqlConfigured: current.postgresqlConfigured === true || incoming.postgresqlConfigured === true,
    postgresqlPresent: current.postgresqlPresent === true || incoming.postgresqlPresent === true,
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
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
}

export interface ResolvedLocator {
  readonly logicalName: string
  readonly row: DataRow
  readonly fieldName: string | null
}

export const ACTIVITY_FIELDS = [
  "pid", "leader_pid", "datid", "datname", "usename", "application_name", "client_addr", "backend_type",
  "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age",
  "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change",
] as const

export function hourOf(timeline: TimelineData): HourData {
  return hourData({
    sections: timeline.lanes,
    availableSections: timeline.availableSections,
    postgresqlConfigured: timeline.postgresqlConfigured ?? false,
    postgresqlPresent: timeline.postgresqlPresent ?? false,
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
    postgresqlConfigured: timeline.postgresqlConfigured ?? false,
    postgresqlPresent: timeline.postgresqlPresent ?? false,
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

export interface SnapshotRequestGroup {
  readonly anchor: SegmentBound
  readonly requests: readonly SectionRequest[]
}

export async function loadTimeline(start: number | null, signal: AbortSignal, onBytes?: (received: number) => void): Promise<TimelineData> {
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
      postgresqlConfigured: fixture.availableSections.some((name) => name.startsWith("pg_")),
      postgresqlPresent: fixture.availableSections.some((name) => name.startsWith("pg_") && !name.startsWith("pg_log_")),
    }
  }
  const window = start === null ? "" : `?from=${start}&to=${start + 3_600_000_000 - 1}`
  const records = await request(`/api/hour${window}`, signal, onBytes)
  const header = records.find((record) => record.record === "hour")
  const catalog = records.find((record) => record.record === "catalog")
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
      sections: segment["sections"].flatMap((section) => section.logical_name === null ? [] : [{
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
  const layouts = new Map<string, RowLayout>()
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
      if (typeof record["truncated"] !== "boolean") throw new Error("finding truncation flag is invalid")
      findingGroups.push({
        segmentId,
        logicalName,
        typeId,
        totalHits,
        shown: 0,
        truncated: record["truncated"],
      })
    } else if (layout !== null) {
      layouts.set(layout.typeId, layout)
    } else if (record.record === "row") {
      const row = laneRow(record, segmentId, layouts)
      if (row !== null) (lanes[row.logicalName] ??= []).push(row)
    } else if (record.record === "lane") {
      lanePoints.push({
        segmentId: requiredText(record.segment_id, "lane segment id"),
        lane: requiredText(record["lane"], "lane name"),
        timestamp: integer(record.ts, "lane timestamp"),
        value: record.value === null ? null : finiteNumber(record.value, "lane value"),
      })
    }
  }
  const healthMetadata = await loadHealthMetadata(points, signal)
  lanes[HEALTH] = healthRows(points, healthMetadata) as DataRow[]
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
    postgresqlConfigured: sourceConfigured(catalog, "postgresql"),
    postgresqlPresent: sourceMetricsPresent(catalog, "postgresql"),
  }
}

export async function loadSeries(
  selectedHour: number,
  section: string,
  where: Readonly<Record<string, string>>,
  fields: readonly string[],
  signal: AbortSignal,
  typeId?: string | undefined,
  group?: RelationGroup | undefined,
): Promise<readonly DataRow[]> {
  signal.throwIfAborted()
  const from = floorHour(selectedHour)
  const to = from + 3_600_000_000 - 1
  if (bundledFixtureRange() !== null) {
    const rows: DataRow[] = []
    const fixture = bundledFixtureHour(from)
    if (fixture !== null) rows.push(...(fixture.sections[section] ?? []))
    return [...new Map(rows.map((row) => [`${row.segmentId}:${row.typeId}:${row.ordinal}`, row])).values()]
      .filter((row) => row.timestamp >= from && row.timestamp <= to)
      .filter((row) => row.typeId === (typeId ?? row.typeId) && fixtureMatches(row, where))
      .filter((row) => group === undefined || row.relation?.group === group)
      .map((row) => projectFixtureRow(row, fields))
  }
  const query = [
    `from=${from}`,
    `to=${to}`,
    `section=${encodeURIComponent(section)}`,
    ...fields.map((name) => `field=${encodeURIComponent(name)}`),
    ...Object.entries(where).map(([column, value]) => `where.${encodeURIComponent(column)}=${encodeURIComponent(value)}`),
    ...(typeId === undefined ? [] : [`type_id=${encodeURIComponent(typeId)}`]),
    ...(group === undefined ? [] : [`group=${group}`]),
  ].join("&")
  const records = await request(`/api/hour?${query}`, signal)
  const layouts = new Map<string, RowLayout>()
  const relationLayouts = new Map<string, RelationLayout>()
  const rows: DataRow[] = []
  let segmentId = ""
  for (const record of records) {
    const layout = layoutRecord(record)
    const relationLayout = parseRelationLayout(record)
    if (record.record === "series_segment") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "series segment id")
    } else if (relationLayout !== null) {
      relationLayouts.set(relationLayoutKey(relationLayout), relationLayout)
    } else if (layout !== null) {
      layouts.set(layout.typeId, layout)
    } else if (record.record === "relation") {
      const row = parseRelationRow(record, relationLayouts, segmentId, from)
      if (row === null) throw new Error("relation series row is invalid")
      rows.push(row)
    } else if (record.record === "row") {
      const row = laneRow(record, segmentId, layouts)
      if (row !== null) rows.push(row)
    }
  }
  return rows
}

export function acceptResponse<T>(promise: Promise<T>, signal: AbortSignal, apply: (value: T) => void, reject?: () => void): void {
  void promise.then((value) => { if (!signal.aborted) apply(value) }).catch(() => { if (!signal.aborted) reject?.() })
}

function laneRow(
  record: Record<string, unknown>,
  segmentId: string,
  layouts: ReadonlyMap<string, RowLayout>,
): DataRow | null {
  const typeId = requiredText(record.type_id, "row type_id")
  const layout = layouts.get(typeId)
  const logicalName = layout?.logicalName ?? logicalNameForTypeId(typeId)
  const values = record.values
  if (layout === undefined || logicalName === null || !Array.isArray(values)) return null
  return {
    segmentId,
    logicalName,
    typeId,
    ordinal: requiredText(record.ordinal, "row ordinal"),
    timestamp: integer(record.timestamp, "row timestamp"),
    values: rowValues(layout.columns, values),
  }
}

export function healthRows(points: readonly Point[], metadata: readonly DataRow[] = []): readonly DataRow[] {
  const healthPoints = new Map<string, Point>()
  for (const point of points) {
    if (!HEALTH_SERIES.has(point.series)) continue
    const key = `${point.segmentId}:${point.timestamp}:${point.series}`
    if (!healthPoints.has(key) || point.value === null) healthPoints.set(key, point)
  }
  const stored = [...healthPoints.values()]
  const segmentOrder = new Map<string, number>()
  for (const point of stored) {
    if (!segmentOrder.has(point.segmentId)) segmentOrder.set(point.segmentId, segmentOrder.size)
  }
  const postgresIntervals = new Map(metadata.flatMap((row) => {
    const seconds = row.values.postgresql_interval_seconds
    const interval = typeof seconds === "number" ? seconds * 1_000_000 : Number.NaN
    return Number.isSafeInteger(interval) && interval >= 0 ? [[row.segmentId, interval] as const] : []
  }))
  const postgresSegments = new Set(stored.filter((point) => point.series === "postgres_health").map((point) => point.segmentId))
  const osSegments = new Set(stored.filter((point) => point.series === "os_health").map((point) => point.segmentId))
  const postgres = stored.filter((point) => point.series === "postgres_health")
    .sort((left, right) => left.timestamp - right.timestamp
      || (segmentOrder.get(left.segmentId) ?? 0) - (segmentOrder.get(right.segmentId) ?? 0))
  const evaluation = new Map<string, Point>()
  for (const point of stored) {
    if (point.series !== "overall_health") continue
    evaluation.set(`${point.segmentId}:${point.timestamp}`, point)
  }
  return [...evaluation.values()]
    .sort((left, right) => left.timestamp - right.timestamp
      || (segmentOrder.get(left.segmentId) ?? 0) - (segmentOrder.get(right.segmentId) ?? 0))
    .map((overall) => {
      const { segmentId, timestamp } = overall
      const at = (series: string) => healthPoints.get(`${segmentId}:${timestamp}:${series}`)
      const os = at("os_health")
      const values: Record<string, Cell> = {}
      values.overall_health = overall.value
      if (osSegments.has(segmentId)) values.os_health = os?.value ?? null
      if (postgresSegments.has(segmentId)) {
        const exact = at("postgres_health")
        const currentOrder = segmentOrder.get(segmentId) ?? Number.MAX_SAFE_INTEGER
        const latest = postgres.filter((point) => point.timestamp <= timestamp
          && (segmentOrder.get(point.segmentId) ?? Number.MAX_SAFE_INTEGER) <= currentOrder).at(-1)
        const interval = postgresIntervals.get(segmentId)
        const fresh = latest !== undefined && interval !== undefined && timestamp - latest.timestamp <= interval
        values.postgres_health = exact === undefined
          ? overall.value !== null || (os?.value === null && fresh) ? latest?.value ?? null : null
          : exact.value
      }
      return {
        segmentId,
        logicalName: HEALTH,
        typeId: overall.typeId,
        ordinal: `${segmentId}:${timestamp}`,
        timestamp,
        values,
      }
    })
}

async function loadHealthMetadata(points: readonly Point[], signal: AbortSignal): Promise<readonly DataRow[]> {
  const postgresSegments = new Set(points.filter((point) => point.series === "postgres_health").map((point) => point.segmentId))
  const evaluations = new Map<string, number>()
  for (const point of points) {
    if (point.series !== "overall_health" || !postgresSegments.has(point.segmentId)) continue
    evaluations.set(point.segmentId, Math.max(evaluations.get(point.segmentId) ?? Number.MIN_SAFE_INTEGER, point.timestamp))
  }
  return (await Promise.all([...evaluations].map(async ([segmentId, at]) => {
    try {
      const snapshot = await loadSnapshot(segmentId, at, [{
        section: "instance_metadata",
        fields: ["postgresql_interval_seconds"],
      }], signal)
      return (snapshot.sections.instance_metadata ?? []).map((row) => ({ ...row, segmentId }))
    } catch (error) {
      if (signal.aborted) throw error
      return []
    }
  }))).flat()
}

const HEALTH_SERIES = new Set(["overall_health", "os_health", "postgres_health"])

export function segmentAt(segments: readonly SegmentBound[], at: number): string | null {
  return segmentBoundAt(segments, at)?.id ?? null
}

export function segmentBoundAt(segments: readonly SegmentBound[], at: number): SegmentBound | null {
  return newestSegment(segments.filter((segment) => segment.minTs <= at && segment.maxTs >= at))
    ?? newestSegment(segments.filter((segment) => segment.maxTs <= at))
}

export function snapshotRequestGroups(
  segments: readonly SegmentBound[],
  at: number,
  requests: readonly SectionRequest[],
): readonly SnapshotRequestGroup[] {
  const eligible = segments.filter((segment) => segment.minTs <= at)
  const grouped = new Map<string, { anchor: SegmentBound; requests: SectionRequest[] }>()
  const add = (anchor: SegmentBound, resolved: readonly SectionRequest[]) => {
    if (resolved.length === 0) return
    const group = grouped.get(anchor.id)
    if (group === undefined) grouped.set(anchor.id, { anchor, requests: [...resolved] })
    else group.requests.push(...resolved)
  }
  for (const request of requests) {
    const matching = eligible.flatMap((segment) => segment.sections
      .filter((section) => section.logicalName === request.section && requestAcceptsLayout(request, section.typeId))
      .map((section) => ({ segment, typeId: section.typeId })))
    if (matching.length === 0) continue
    if (request.pageSize !== undefined || request.group !== undefined) {
      const anchor = newestSegment(matching.map(({ segment }) => segment))
      if (anchor !== null) add(anchor, requestsForSegment([request], anchor))
      continue
    }
    const anchors = new Map<string, { anchor: SegmentBound; typeIds: string[] }>()
    for (const typeId of unique(matching.map((candidate) => candidate.typeId))) {
      const anchor = newestSegment(matching
        .filter((candidate) => candidate.typeId === typeId)
        .map((candidate) => candidate.segment))
      if (anchor === null) continue
      const assigned = anchors.get(anchor.id)
      if (assigned === undefined) anchors.set(anchor.id, { anchor, typeIds: [typeId] })
      else assigned.typeIds.push(typeId)
    }
    if (anchors.size === 1) {
      const assigned = anchors.values().next().value
      if (assigned !== undefined) add(assigned.anchor, requestsForSegment([request], assigned.anchor))
      continue
    }
    for (const { anchor, typeIds } of anchors.values()) {
      for (const typeId of typeIds) {
        add(anchor, requestsForSegment([{ ...request, typeId }], anchor))
      }
    }
  }
  return [...grouped.values()]
}

function requestAcceptsLayout(request: SectionRequest, typeId: string): boolean {
  if (request.typeId !== undefined && request.typeId !== typeId) return false
  if (request.typeIds !== undefined && !request.typeIds.includes(typeId)) return false
  const fields = request.fieldsByType?.[typeId] ?? request.fields
  if (fields === undefined) return request.fieldsByType === undefined
  const physical = new Set(REGISTRY_BY_TYPE_ID.get(typeId)?.columns ?? [])
  return fields.some((field) => physical.has(field))
}

function newestSegment(segments: readonly SegmentBound[]): SegmentBound | null {
  let newest: SegmentBound | null = null
  for (const segment of segments) {
    if (newest === null || compareSegmentIds(segment.id, newest.id) > 0) newest = segment
  }
  return newest
}

function compareSegmentIds(left: string, right: string): number {
  if (/^-?\d+$/.test(left) && /^-?\d+$/.test(right)) {
    const leftId = BigInt(left)
    const rightId = BigInt(right)
    return leftId < rightId ? -1 : leftId > rightId ? 1 : 0
  }
  return left.localeCompare(right, undefined, { numeric: true })
}

export function requestsForSegment(
  requests: readonly SectionRequest[],
  segment: SegmentBound,
): readonly SectionRequest[] {
  return requests.flatMap((request) => {
    const typeIds = segment.sections
      .filter((section) => section.logicalName === request.section)
      .map((section) => section.typeId)
      .filter((typeId) => request.typeId === undefined || request.typeId === typeId)
      .filter((typeId) => request.typeIds === undefined || request.typeIds.includes(typeId))
    if (typeIds.length === 0) return []
    if (request.group !== undefined) return [request]
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
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly findings: readonly Finding[]
  readonly findingGroups?: readonly FindingGroup[]
}): HourData {
  const rows = (name: string) => input.sections[name] ?? []
  const flatten = (names: readonly string[]) => names.flatMap(rows)
  return {
    ...input,
    postgresqlConfigured: input.postgresqlConfigured ?? false,
    postgresqlPresent: input.postgresqlPresent ?? false,
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
  readonly search?: string
}

export interface SnapshotOrder {
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
  const relationLayouts = new Map<string, RelationLayout>()
  const grouped: Record<string, DataRow[]> = {}
  const rateColumns: Record<string, readonly string[]> = {}
  const snapshotRows: SnapshotRows[] = []
  for (const section of requests) grouped[section.section] = []
  for (const record of records) {
    const layout = layoutRecord(record)
    const relationLayout = parseRelationLayout(record)
    if (relationLayout !== null) {
      relationLayouts.set(relationLayoutKey(relationLayout), relationLayout)
      rateColumns[relationLayout.logicalName] = unique([
        ...(rateColumns[relationLayout.logicalName] ?? []),
        ...relationRateFields(relationLayout),
      ])
    } else if (layout !== null) {
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
    } else if (record.record === "relation") {
      const row = parseRelationRow(record, relationLayouts, segmentId, at)
      if (row === null) throw new Error("relation record is invalid")
      const rows = grouped[row.logicalName] ?? []
      rows.push(row)
      grouped[row.logicalName] = rows
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
        segmentId: requiredText(record.segment_id, "row segment id"),
        logicalName,
        typeId,
        ordinal: requiredText(record.ordinal, "row ordinal"),
        timestamp: record.timestamp === null ? at : integer(record.timestamp, "row timestamp"),
        values: rowValues(columns, values),
      })
      grouped[logicalName] = rows
    } else if (record.record === "snapshot_page") {
      const logicalName = requiredText(record.logical_name, "snapshot page logical name")
      if ((record.order_direction !== "asc" && record.order_direction !== "desc")
        || typeof record.has_more !== "boolean"
        || typeof record["truncated"] !== "boolean"
        || !Array.isArray(record.order_by)
        || (record.next_cursor !== null && typeof record.next_cursor !== "string")) {
        throw new Error(`snapshot page for ${logicalName} is invalid`)
      }
      snapshotRows.push({
        logicalName,
        eligible: integer(record["eligible"], "eligible row count"),
        returned: integer(record["returned"], "returned row count"),
        hasMore: record.has_more,
        truncated: record["truncated"],
        nextCursor: record.next_cursor,
        pageSize: integer(record.page_size, "snapshot page size"),
        orderBy: record.order_by.map((field) => requiredText(field, "snapshot order field")),
        orderDirection: record.order_direction,
        from: record.from === null ? null : integer(record.from, "snapshot interval start"),
        to: record.to === null ? null : integer(record.to, "snapshot interval end"),
        ...(record.group === undefined ? {} : { group: relationGroup(record.group) }),
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

export async function loadSnapshotGroups(
  groups: readonly SnapshotRequestGroup[],
  at: number,
  signal: AbortSignal,
  order?: SnapshotOrder | undefined,
): Promise<HourData> {
  const snapshots = await Promise.all(groups.map((group) => loadSnapshot(
    group.anchor.id,
    at,
    group.requests,
    signal,
    order,
  )))
  return snapshots.reduce((current, incoming) => mergeSnapshotData(current, incoming), emptyHour())
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
    const fields = request.fields
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
    && section.group === undefined
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
  const requestedOrder = section === undefined || order === undefined ? undefined : requestedSnapshotOrder(section, order)
  return [
    `at=${at}`,
    ...sections.map((request) => `section=${encodeURIComponent(request.section)}`),
    ...fields.map((field) => `field=${encodeURIComponent(field)}`),
    ...ordered.map((field) => `by=${encodeURIComponent(field)}`),
    ...(section?.group === undefined ? [] : [`group=${section.group}`]),
    ...(requestedOrder === undefined || order?.descending !== false ? [] : ["direction=asc"]),
    ...(section?.pageSize === undefined || options.rowOrdinal !== undefined ? [] : [`page_size=${section.pageSize}`]),
    ...(options.fullText === true ? [] : [`text=${CELL_TEXT}`]),
    ...(options.cursor === undefined ? [] : [`cursor=${encodeURIComponent(options.cursor)}`]),
    ...(options.search === undefined ? [] : [`search=${encodeURIComponent(options.search)}`]),
    ...Object.entries(options.filters ?? {}).map(([column, value]) =>
      `where.${encodeURIComponent(column)}=${encodeURIComponent(value)}`),
    ...(typeId === undefined ? [] : [`type_id=${encodeURIComponent(typeId)}`]),
    ...(options.rowOrdinal === undefined ? [] : [`row_ordinal=${encodeURIComponent(options.rowOrdinal)}`]),
  ].join("&")
}

function snapshotOrder(section: SectionRequest, chosen: SnapshotOrder | undefined): readonly string[] {
  const requested = chosen === undefined ? section.defaultOrder : requestedSnapshotOrder(section, chosen)
  if (requested !== undefined && requested.length > 0) return unique(requested)
  if (chosen !== undefined && section.defaultOrder !== undefined && section.defaultOrder.length > 0) {
    return unique(section.defaultOrder)
  }
  return unique(section.fallbackOrder ?? [])
}

function requestedSnapshotOrder(section: SectionRequest, chosen: SnapshotOrder): readonly string[] | undefined {
  return section.order === undefined
    ? section.fields?.includes(chosen.column) === true ? [chosen.column] : undefined
    : section.order[chosen.column]
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
    && (record["kind"] === "known_bad" || record["kind"] === "spike" || record["kind"] === "event")
}

function indexPoint(record: Record<string, unknown>, segmentId: string, logicalName: string): Point {
  const typeId = requiredText(record.type_id, "point type_id")
  return {
    segmentId,
    logicalName: logicalNameForTypeId(typeId) ?? logicalName,
    typeId,
    series: requiredText(record["series"], "point series"),
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
    kind: record["kind"] as Finding["kind"],
    typeId,
    timestamp: integer(record.ts, "finding timestamp"),
    category: typeof record["category"] === "number" ? record["category"] : null,
    rowOrdinal: requiredText(record.row_ordinal, "finding row ordinal"),
    fieldOrdinal: integer(record.field_ordinal, "finding field ordinal"),
  }
}

interface RowLayout {
  readonly typeId: string
  readonly logicalName: string | null
  readonly columns: readonly string[]
}

function layoutRecord(record: Record<string, unknown>): RowLayout | null {
  if (record.record !== "layout") return null
  const layout = record.layout as {
    readonly type_id: unknown
    readonly logical_name?: unknown
    readonly columns?: readonly { readonly name: unknown }[]
  }
  const typeId = requiredText(layout.type_id, "layout type_id")
  if (!Array.isArray(layout["columns"])) throw new Error(`layout ${typeId} has no columns`)
  return {
    typeId,
    logicalName: typeof layout.logical_name === "string" ? layout.logical_name : null,
    columns: layout["columns"].map((column) => requiredText(column.name, "column name")),
  }
}

async function request(path: string, signal: AbortSignal, onBytes?: (received: number) => void): Promise<readonly Record<string, unknown>[]> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    let response: Response
    try {
      response = await apiFetch(path, { headers: { Accept: "application/x-ndjson" }, signal })
    } catch (error) {
      signal.throwIfAborted()
      if (attempt === 0 && error instanceof TypeError) continue
      throw error
    }
    if (!response.ok) {
      throw new Error(`HTTP ${response.status} for ${path}`)
    }
    try {
      return await readNdjson(response, path, signal, onBytes)
    } catch (error) {
      signal.throwIfAborted()
      if (attempt === 0 && error instanceof TypeError) continue
      throw error
    }
  }
  throw new Error(`HTTP read failed for ${path}`)
}

function catalogSegments(records: readonly Record<string, unknown>[]): readonly Segment[] {
  return records.filter(
    (record) => record.record === "finished_segment" || record.record === "active_segment",
  ) as unknown as readonly Segment[]
}

function sourceConfigured(header: Record<string, unknown> | undefined, name: string): boolean {
  const families = header?.source_families
  return Array.isArray(families) && families.some((family) => family !== null
    && typeof family === "object"
    && (family as { readonly name?: unknown }).name === name
    && (family as { readonly configured?: unknown }).configured === true)
}

function sourceMetricsPresent(header: Record<string, unknown> | undefined, name: string): boolean {
  const families = header?.source_families
  return Array.isArray(families) && families.some((family) => family !== null
    && typeof family === "object"
    && (family as { readonly name?: unknown }).name === name
    && (family as { readonly metrics_present?: unknown }).metrics_present === true)
}

function segmentSectionNames(segment: Segment): string[] {
  const present = new Set(segment["sections"].flatMap((section) =>
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

function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}
