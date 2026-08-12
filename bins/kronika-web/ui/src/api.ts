import { registry } from "kronika:registry"

import { bundledFixtureHour, bundledFixtureRange } from "./fixture"
import { rowMatchesLocator } from "./locator"
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
  postgresqlPlans: ["pg_store_plans", "pg_store_plans_info"] as const,
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
  /** Registry layouts this semantic request knows how to present. */
  readonly typeIds?: readonly string[]
  /** Curated physical projection for each supported registry layout. */
  readonly fieldsByType?: Readonly<Record<string, readonly string[]>>
  /** One exact physical layout, assigned for a request at the cursor. */
  readonly typeId?: string
  /** Maximum rows after the server has ordered the physical table. */
  readonly top?: number
  /** Physical candidates for the normal high-demand order. */
  readonly defaultOrder?: readonly string[]
  /** A semantic UI column mapped to the physical candidates that can carry it. */
  readonly order?: Readonly<Record<string, readonly string[]>>
  /** Used only when the current physical layout has none of the chosen candidates. */
  readonly fallbackOrder?: readonly string[]
}

/** Every view draws the timeline, so these four are the floor. */
/** The one series drawn end to end. The other lanes were three more sections
 *  times every segment in the hour, for lines beside the one being navigated
 *  by; they come from the snapshot under the cursor instead. */
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

export interface SourceFamily {
  readonly name: string
  readonly configured: boolean
  readonly present: boolean
}

export interface HourData {
  /** Rows keyed by their registry logical section name. */
  readonly sections: Readonly<Record<string, readonly DataRow[]>>
  /** Columns the server divided by the interval: they read per second. */
  readonly rateColumns: Readonly<Record<string, readonly string[]>>
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
  readonly pgPlans: readonly DataRow[]
  readonly pgLocks: readonly DataRow[]
  readonly pgDatabases: readonly DataRow[]
  readonly pgEvents: readonly DataRow[]
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly findings: readonly Finding[]
  readonly sourceFamilies: readonly SourceFamily[]
  readonly segmentCount: number
}

/** One lane of the timeline: a share of the ceiling this machine lived under,
 *  computed by the server against the environment it ran in. */
export interface LanePoint {
  readonly segmentId: string
  readonly lane: string
  readonly timestamp: number
  readonly value: number | null
}

export interface TimelineData {
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  /** Whole-hour rows keyed by section: the line and the lanes beside it. */
  readonly lanes: Readonly<Record<string, readonly DataRow[]>>
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

export const ACTIVITY_FIELDS = [
  "pid", "leader_pid", "datname", "usename", "application_name", "client_addr", "backend_type",
  "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age",
  "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change",
] as const

/** The hour as the rest of the screen holds it. The line goes in beside every
 *  other section, or the first snapshot to arrive merges it away. */
export function hourOf(timeline: TimelineData): HourData {
  return hourData({
    sections: timeline.lanes,
    availableSections: timeline.availableSections,
    points: timeline.points,
    lanePoints: timeline.lanePoints,
    findings: timeline.findings,
    sourceFamilies: timeline.sourceFamilies,
    segmentCount: timeline.segments.length,
  })
}

/** A snapshot replaces the sections it carries: it is one moment, and keeping
 *  the moments visited before it grows without bound and lets a table draw a
 *  moment the cursor has left. */
/** The stored sample at or before a moment. The line carries every sample of
 *  the hour, so it is what says where a cursor actually landed. */
export function sampleAt(line: readonly DataRow[], cursor: number): number | null {
  let chosen: number | null = null
  for (const row of line) {
    if (row.timestamp <= cursor && (chosen === null || row.timestamp > chosen)) chosen = row.timestamp
  }
  return chosen ?? (line.length === 0 ? null : Math.min(...line.map((row) => row.timestamp)))
}

export function replaceSections(before: HourData, after: HourData): HourData {
  const sections: Record<string, readonly DataRow[]> = { ...before.sections }
  for (const [name, rows] of Object.entries(after.sections)) sections[name] = rows
  return hourData({
    sections,
    rateColumns: mergeRateColumns(before.rateColumns, after.rateColumns ?? {}),
    availableSections: unique([...before.availableSections, ...after.availableSections]),
    points: before.points,
    lanePoints: before.lanePoints,
    findings: before.findings,
    sourceFamilies: before.sourceFamilies,
    segmentCount: before.segmentCount,
  })
}

export function replaceFindings(before: HourData, logicalName: string, findings: readonly Finding[]): HourData {
  return {
    ...before,
    findings: [...before.findings.filter((finding) => finding.logicalName !== logicalName), ...findings]
      .sort(compareFindings),
  }
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

/** Finding indexes are already sparse. A PostgreSQL table asks for its own
 * marks when opened instead of adding every high-cardinality section to the
 * hour response. */
export async function loadSectionFindings(
  segments: readonly SegmentBound[],
  logicalName: string,
  signal: AbortSignal,
): Promise<readonly Finding[]> {
  const relevant = segments.filter((segment) => segment.sections.some((section) => section.logicalName === logicalName))
  const batches = await Promise.all(relevant.map(async (segment) => {
    const records = await request(
      `/api/segments/${encodeURIComponent(segment.id)}/sections/${encodeURIComponent(logicalName)}/index`,
      signal,
    )
    return records.filter(isFindingRecord).map((record) => indexFinding(record, segment.id, logicalName))
  }))
  return batches.flat().sort(compareFindings)
}

function compareFindings(left: Finding, right: Finding): number {
  return left.timestamp - right.timestamp
    || left.segmentId.localeCompare(right.segmentId)
    || left.typeId.localeCompare(right.typeId)
    || left.rowOrdinal.localeCompare(right.rowOrdinal)
    || left.fieldOrdinal - right.fieldOrdinal
    || left.kind.localeCompare(right.kind)
}

/** The whole timeline of one hour in one request: which segments it touches,
 *  the health line and the marks. A round trip over the debugging link costs
 *  more than a second, so the count of requests is what the wait is made of. */
export async function loadTimeline(start: number | null, signal: AbortSignal): Promise<TimelineData> {
  const range = bundledFixtureRange()
  const requested = floorHour(start ?? range?.from ?? 0)
  const fixture = bundledFixtureHour(requested)
  if (fixture !== null && range !== null) {
    return {
      hour: requested, availableHours: unique([floorHour(range.from), floorHour(range.to)]),
      segments: [], lanePoints: fixture.lanePoints, lanes: fixture.sections, health: fixture.health, points: fixture.points,
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
  const lanePoints: LanePoint[] = []
  const lanes: Record<string, DataRow[]> = { [HEALTH]: [] }
  const layouts = new Map<string, readonly string[]>()
  let segmentId = ""
  for (const record of records) {
    if (record.record === "index") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "index segment id")
    } else if (record.record === "point") {
      points.push(indexPoint(record, segmentId, HEALTH))
    } else if (isFindingRecord(record)) {
      findings.push(indexFinding(record, segmentId, HEALTH))
    } else if (record.record === "layout") {
      const layout = record.layout as { readonly type_id: unknown; readonly columns: readonly { readonly name: unknown }[] }
      if (Array.isArray(layout.columns)) {
        layouts.set(
          requiredText(layout.type_id, "layout type_id"),
          layout.columns.map((column) => requiredText(column.name, "column name")),
        )
      }
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
    sourceFamilies: sourceFamiliesOf(records),
    availableSections: availableSectionNames(all),
  }
}

/** One object's rows across the whole hour: what a chart in the detail panel
 *  draws. A snapshot is one moment, and a moment is not a line. */
export async function loadSeries(
  hour: number,
  section: string,
  where: Readonly<Record<string, string>>,
  fields: readonly string[],
  signal: AbortSignal,
  typeId?: string | undefined,
): Promise<readonly DataRow[]> {
  signal.throwIfAborted()
  const fixture = bundledFixtureHour(hour)
  if (fixture !== null) {
    const fieldsToKeep = unique([...fields, ...Object.keys(where)])
    return (fixture.sections[section] ?? [])
      .filter((row) => row.typeId === (typeId ?? row.typeId) && fixtureMatches(row, where))
      .map((row) => projectFixtureRow(row, fieldsToKeep))
  }
  const query = [
    `from=${hour}`,
    `to=${hour + 3_600_000_000 - 1}`,
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
    if (record.record === "series_segment") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "series segment id")
    } else if (record.record === "layout") {
      const layout = record.layout as { readonly type_id: unknown; readonly columns: readonly { readonly name: unknown }[] }
      if (Array.isArray(layout.columns)) {
        layouts.set(
          requiredText(layout.type_id, "layout type_id"),
          layout.columns.map((column) => requiredText(column.name, "column name")),
        )
      }
    } else if (record.record === "row") {
      const row = laneRow(record, segmentId, layouts)
      if (row !== null) rows.push(row)
    }
  }
  return rows
}

/** A lane row of the hour. Its section comes from the layout the row names,
 *  the same way history reads one. */
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
    values: Object.fromEntries(names.map((name, index) => [
      name,
      index < values.length ? values[index] as Cell : null,
    ])),
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

/** The segment holding a moment, or the last one before it. A table shows one
 *  snapshot, so it needs one segment and not the hour around it. */
export function segmentAt(segments: readonly SegmentBound[], at: number): string | null {
  return segmentBoundAt(segments, at)?.id ?? null
}

export function segmentBoundAt(segments: readonly SegmentBound[], at: number): SegmentBound | null {
  return segments.find((segment) => segment.minTs <= at && segment.maxTs >= at)
    ?? segments.filter((segment) => segment.maxTs <= at).at(-1)
    ?? null
}

/** Restrict a curated projection and its order aliases to physical columns
 *  carried by the registry layouts in the segment under the cursor. */
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
    const { fieldsByType: _fieldsByType, typeIds: _typeIds, ...base } = request
    return unique(typeIds).flatMap((typeId) => {
      const physical = new Set(
        REGISTRY_BY_TYPE_ID.get(typeId)?.columns.map((column) => column.name) ?? [],
      )
      const projection = request.fieldsByType?.[typeId] ?? request.fields ?? []
      const fields = unique(projection.filter((field) => physical.has(field)))
      // Sending no field parameter means every physical column. A projected
      // request with no matching column must therefore be omitted.
      if (fields.length === 0) return []
      const keep = (candidates: readonly string[]) => candidates.filter((field) => physical.has(field))
      const order = request.order === undefined
        ? undefined
        : Object.fromEntries(Object.entries(request.order).map(([name, candidates]) => [name, keep(candidates)]))
      return [{
        ...base,
        typeId,
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
  return REGISTRY_BY_TYPE_ID.get(locator.typeId)?.columns[locator.fieldOrdinal]?.name ?? null
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
  readonly rateColumns?: Readonly<Record<string, readonly string[]>>
  readonly availableSections: readonly string[]
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly findings: readonly Finding[]
  readonly sourceFamilies: readonly SourceFamily[]
  readonly segmentCount: number
}): HourData {
  const rows = (name: string) => input.sections[name] ?? []
  const flatten = (names: readonly string[]) => names.flatMap(rows)
  return {
    ...input,
    rateColumns: input.rateColumns ?? {},
    processes: rows("os_process"),
    activities: rows("pg_stat_activity"),
    load: rows("os_loadavg"),
    memory: rows("os_meminfo"),
    pressure: rows("os_psi"),
    health: rows("health"),
    pgOverview: flatten(PRODUCT_SECTION_GROUPS.postgresqlOverview),
    pgStatements: flatten(PRODUCT_SECTION_GROUPS.postgresqlStatements),
    pgPlans: flatten(PRODUCT_SECTION_GROUPS.postgresqlPlans),
    pgLocks: flatten(PRODUCT_SECTION_GROUPS.postgresqlLocks),
    pgDatabases: flatten(PRODUCT_SECTION_GROUPS.postgresqlDatabases),
    pgEvents: flatten(PRODUCT_SECTION_GROUPS.events),
  }
}

/** Characters of a text a table cell can show; a query is fetched whole on demand. */
const CELL_TEXT = 160

export interface SnapshotOptions {
  readonly filters?: Readonly<Record<string, string>>
  readonly typeId?: string
  readonly rowOrdinal?: string
  readonly fullText?: boolean
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
  if ((chosen.filters !== undefined || chosen.typeId !== undefined || chosen.rowOrdinal !== undefined)
    && requests.length !== 1) {
    throw new Error("a filtered or exact snapshot needs one section")
  }
  if (chosen.rowOrdinal !== undefined && chosen.typeId === undefined && requests[0]?.typeId === undefined) {
    throw new Error("an exact snapshot row needs typeId")
  }
  if (chosen.rowOrdinal !== undefined && chosen.filters !== undefined) {
    throw new Error("an exact snapshot row cannot carry filters")
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
  const plain = requests.filter((section) => plainSnapshotSection(section))
  const individual = requests.filter((section) => !plainSnapshotSection(section))
  const batches = chosen.filters !== undefined || chosen.typeId !== undefined || chosen.rowOrdinal !== undefined
    ? requests.map((section) => [section] as const)
    : [
        ...(plain.length === 0 ? [] : [plain]),
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
  for (const section of requests) grouped[section.section] = []
  for (const record of records) {
    if (record.record === "layout") {
      const layout = record.layout as {
        readonly type_id: unknown
        readonly logical_name: unknown
        readonly columns: readonly { readonly name: unknown }[]
      }
      const typeId = requiredText(layout.type_id, "layout type_id")
      const logicalName = requiredText(layout.logical_name, "logical name")
      if (!Array.isArray(layout.columns)) throw new Error(`layout ${typeId} has no columns`)
      layouts.set(typeId, {
        logicalName,
        columns: layout.columns.map((column) => requiredText(column.name, "column name")),
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
        values: Object.fromEntries(columns.map((name, index) => [
          name,
          index < values.length ? values[index] as Cell : null,
        ])),
      })
      grouped[logicalName] = rows
    }
  }
  return hourData({
    sections: grouped,
    rateColumns,
    availableSections: unique(requests.map((section) => section.section)),
    points: [],
    lanePoints: [],
    findings: [],
    sourceFamilies: [],
    segmentCount: 1,
  })
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
  for (const request of requests) {
    const typeId = request.typeId ?? options.typeId
    let rows = (fixture.sections[request.section] ?? [])
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
    const orderFields = snapshotOrder(request, order)
    const orderField = orderFields.find((field) => rows.some((row) => row.values[field] !== undefined))
    if (orderField !== undefined) {
      rows = rows.slice().sort((left, right) => fixtureOrder(right.values[orderField], left.values[orderField]))
    }
    if (request.top !== undefined) rows = rows.slice(0, request.top)
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
    availableSections: unique(requests.map((request) => request.section)),
    points: [],
    lanePoints: [],
    findings: [],
    sourceFamilies: [],
    segmentCount: 1,
  })
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

function plainSnapshotSection(section: SectionRequest): boolean {
  return section.fields === undefined
    && section.typeId === undefined
    && section.top === undefined
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
  const ordered = section === undefined || options.rowOrdinal !== undefined
    ? []
    : snapshotOrder(section, order)
  return [
    `at=${at}`,
    ...sections.map((request) => `section=${encodeURIComponent(request.section)}`),
    ...(section?.fields ?? []).map((field) => `field=${encodeURIComponent(field)}`),
    ...ordered.map((field) => `by=${encodeURIComponent(field)}`),
    ...(section?.top === undefined || options.rowOrdinal !== undefined ? [] : [`top=${section.top}`]),
    ...(options.fullText === true ? [] : [`text=${CELL_TEXT}`]),
    ...Object.entries(options.filters ?? {}).map(([column, value]) =>
      `where.${encodeURIComponent(column)}=${encodeURIComponent(value)}`),
    ...(typeId === undefined ? [] : [`type_id=${encodeURIComponent(typeId)}`]),
    ...(options.rowOrdinal === undefined ? [] : [`row_ordinal=${encodeURIComponent(options.rowOrdinal)}`]),
  ].join("&")
}

function snapshotOrder(section: SectionRequest, chosen: SnapshotOrder | undefined): readonly string[] {
  // The server exposes the largest rows of a cut table. When a header cycles
  // to ascending, keep the normal demand order instead of claiming that the
  // smallest visible slice is the smallest slice of the physical table.
  const requested = chosen !== undefined && chosen.descending
    ? section.order?.[chosen.column]
      ?? (section.fields?.includes(chosen.column) === true ? [chosen.column] : undefined)
    : section.defaultOrder
  if (requested !== undefined && requested.length > 0) return unique(requested)
  return unique(section.fallbackOrder ?? [])
}

function snapshotOptions(
  value: SnapshotOptions | Readonly<Record<string, string>> | undefined,
): SnapshotOptions {
  if (value === undefined) return {}
  if ("filters" in value || "typeId" in value || "rowOrdinal" in value || "fullText" in value) {
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
    sourceFamilies: [],
    segmentCount: 1,
  })
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
    throw new Error(`HTTP ${response.status} for ${path}`)
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

function mergeRateColumns(
  left: Readonly<Record<string, readonly string[]>>,
  right: Readonly<Record<string, readonly string[]>>,
): Readonly<Record<string, readonly string[]>> {
  const merged: Record<string, readonly string[]> = { ...left }
  for (const [section, fields] of Object.entries(right)) {
    merged[section] = unique([...(merged[section] ?? []), ...fields])
  }
  return merged
}

function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}
