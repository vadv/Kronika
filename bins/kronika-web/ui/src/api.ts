import { registry } from "kronika:registry"

import { CGROUP_CPU_CUTS, CGROUP_IO_CUTS, DATABASE_CUTS, INDEX_CUTS, PLAN_CUTS, PROCESS_CUTS, STATEMENT_CUTS, TABLE_CUTS, type ActivityCut } from "./activity-cuts"
import { bundledFixtureHour, bundledFixtureRange } from "./fixture"
import { HOUR_MICROS, heatmap, heatmapEntityKey, type HeatmapSample, type HeatmapView, type HeatmapViewRow, type TopActivityDefinition, type TopActivityEntity, type TopActivityMetric, type TopActivityRelationLevel, type TopActivitySurface, type TopActivityUnit } from "./heatmap"
import { rowMatchesLocator } from "./locator"
import { decoratePostgresIntervalRow, intervalMetric, PG_STAT_STATEMENTS_TYPE_IDS, PG_STORE_PLANS_TYPE_IDS, postgresIdentity, supportsPostgresDerivedOrder, unique } from "./postgres-metrics"
import { parseRelationLayout, parseRelationRow, relationGroup, relationLayoutKey, relationRateFields, relationRowKey, type RelationGroup, type RelationLayout, type RelationRow } from "./postgres-relations"
import { apiFetch } from "./session"
import { readNdjson } from "./wire"
import { canonicalSearch } from "./search"

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
  postgresqlOverview: [...POSTGRESQL_OVERVIEW, "pg_wal_storage", "pg_stat_wal", "pg_stat_archiver"] as const,
  postgresqlSettings: ["pg_settings"] as const,
  postgresqlActivity: ["pg_stat_activity"] as const,
  postgresqlVacuum: ["pg_stat_progress_vacuum", "pg_stat_activity"] as const,
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
  readonly syntheticDemo?: boolean
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
  readonly processes: readonly DataRow[]
  readonly activities: readonly DataRow[]
  readonly load: readonly DataRow[]
  readonly memory: readonly DataRow[]
  readonly pressure: readonly DataRow[]
  readonly health: readonly DataRow[]
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly findings: readonly Finding[]
  readonly findingGroups: readonly FindingGroup[]
}

export interface SnapshotRows {
  readonly logicalName: string
  readonly eligible: number | null
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
    syntheticDemo: current.syntheticDemo === true || incoming.syntheticDemo === true,
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
  readonly syntheticDemo?: boolean
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
    syntheticDemo: timeline.syntheticDemo ?? false,
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
    syntheticDemo: timeline.syntheticDemo ?? false,
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
      syntheticDemo: false,
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
    syntheticDemo: catalog?.demo === "synthetic",
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

export async function loadHeatmap(
  selectedHour: number,
  surface: TopActivitySurface,
  metric: TopActivityMetric,
  top: number,
  signal: AbortSignal,
  level?: TopActivityRelationLevel,
): Promise<HeatmapView> {
  signal.throwIfAborted()
  const hour = floorHour(selectedHour)
  const effectiveLevel = surface === "postgresql_tables" || surface === "postgresql_indexes"
    ? level ?? "object"
    : undefined
  if (bundledFixtureRange() !== null) return fixtureHeatmap(hour, surface, metric, top, effectiveLevel)
  const query = [
    `hour=${hour}`,
    `surface=${surface}`,
    `metric=${metric}`,
    ...(effectiveLevel === undefined ? [] : [`level=${effectiveLevel}`]),
    `top=${top}`,
  ].join("&")
  const stored = await requestJson(`/api/heatmap?${query}`, signal)
  return parseTopActivityResult(stored, { hour, surface, metric, level: effectiveLevel ?? null })
}

function fixtureHeatmap(
  hour: number,
  surface: TopActivitySurface,
  metric: TopActivityMetric,
  top: number,
  level?: TopActivityRelationLevel,
): HeatmapView {
  const plan = fixtureTopActivityPlan(surface, level)
  const cut = plan.cuts.find((candidate) => candidate.id === metric)
  if (cut === undefined) throw new Error(`metric ${metric} is invalid for ${surface}`)
  const fixture = bundledFixtureHour(hour)
  const rows = fixture === null ? [] : fixture.sections[plan.section] ?? []
  const conversion = fixtureConversion(cut, fixture, hour + HOUR_MICROS - 1)
  const samples: HeatmapSample[] = []
  const firstRow = new Map<string, DataRow>()
  const lastValues = new Map<string, Map<string, { readonly timestamp: number; readonly value: Cell }>>()
  const groupOf = new Map<string, readonly (string | null)[]>()
  for (const row of rows) {
    const layout = REGISTRY_BY_TYPE_ID.get(row.typeId)
    if (layout === undefined || layout.logicalName !== plan.section) continue
    const identity = (layout.identity ?? []).map((name) => fixtureText(row.values[name]))
    const entity = heatmapEntityKey([row.typeId, ...identity])
    if (!firstRow.has(entity)) firstRow.set(entity, row)
    const labels = lastValues.get(entity) ?? new Map()
    for (const [name, value] of Object.entries(row.values)) {
      if (value === null || value === undefined || typeof value === "object") continue
      const stored = labels.get(name)
      if (stored === undefined || row.timestamp >= stored.timestamp) labels.set(name, { timestamp: row.timestamp, value })
    }
    lastValues.set(entity, labels)
    let numeric: number | null = null
    for (const field of cut.fields) {
      const raw = row.values[field]
      const parsed = typeof raw === "number" ? raw : typeof raw === "string" ? Number(raw) : null
      if (parsed !== null && Number.isFinite(parsed)) numeric = (numeric ?? 0) + parsed
    }
    if (numeric !== null && plan.group.length > 0 && !groupOf.has(entity)) {
      groupOf.set(entity, plan.group.map((name) => fixtureText(row.values[name])))
    }
    const scaled = numeric === null ? null : numeric * conversion.scale
    samples.push({ entity, timestamp: row.timestamp, value: scaled !== null && Number.isFinite(scaled) ? scaled : null })
  }
  const cumulative = cut.class === "cumulative"
  const all = heatmap(samples, cumulative, hour, plan.intervals, Number.MAX_SAFE_INTEGER)
  let ranked: HeatmapViewRow[]
  if (plan.group.length === 0) {
    ranked = all.rows.map((row) => {
      const first = firstRow.get(row.entity)
      const source = first === undefined ? undefined : {
        ...first,
        values: {
          ...first.values,
          ...Object.fromEntries([...(lastValues.get(row.entity)?.entries() ?? [])].map(([name, stored]) => [name, stored.value])),
        },
      }
      return {
        recorded_layout: fixtureLayout(source?.typeId),
        entity: fixtureTopActivityEntity(surface, source),
        members: null,
        total: finiteOrNull(row.total),
        cells: row.cells.map(finiteOrNull),
      }
    })
  } else {
    const grouped = new Map<string, { values: readonly (string | null)[]; members: number; total: number | null; cells: (number | null)[] }>()
    for (const row of all.rows) {
      const values = groupOf.get(row.entity) ?? plan.group.map(() => null)
      const key = heatmapEntityKey(values)
      const slot = grouped.get(key) ?? { values, members: 0, total: null, cells: new Array<number | null>(plan.intervals).fill(null) }
      slot.members += 1
      if (row.total !== null) slot.total = finiteSum(slot.total, row.total)
      for (const [index, cell] of row.cells.entries()) {
        if (cell !== null) slot.cells[index] = finiteSum(slot.cells[index] ?? null, cell)
      }
      grouped.set(key, slot)
    }
    ranked = [...grouped.entries()]
      .sort((left, right) => compareFixtureTotals(left[1].total, right[1].total))
      .map(([, slot]) => ({
        recorded_layout: null,
        entity: fixtureGroupEntity(level ?? (surface === "processes" ? null : "object"), slot.values),
        members: slot.members,
        total: finiteOrNull(slot.total),
        cells: slot.cells.map(finiteOrNull),
      }))
  }
  const kept = ranked.slice(0, top)
  const rest = ranked.slice(top)
  const othersCells = new Array<number | null>(plan.intervals).fill(null)
  let othersTotal: number | null = null
  for (const row of rest) {
    if (cumulative && row.total !== null) othersTotal = finiteSum(othersTotal, row.total)
    for (const [index, cell] of row.cells.entries()) {
      if (cell !== null) othersCells[index] = finiteSum(othersCells[index] ?? null, cell)
    }
  }
  if (!cumulative) othersTotal = finiteMaximum(othersCells)
  const intervals = heatmapIntervalsAsStrings(hour, plan.intervals)
  return {
    hour_start: String(hour),
    hour_end: String(hour + HOUR_MICROS - 1),
    surface,
    metric,
    level: surface === "postgresql_tables" || surface === "postgresql_indexes" ? level ?? "object" : null,
    definition: fixtureDefinition(cut, conversion.unit, plan.group.length > 0),
    intervals,
    rows: kept,
    totals: {
      total: cumulative ? finiteOrNull(all.totalsTotal) : finiteMaximum(all.totals),
      cells: all.totals.map(finiteOrNull),
    },
    others: { total: finiteOrNull(othersTotal), cells: othersCells.map(finiteOrNull) },
    others_count: rest.length,
    entity_count: ranked.length,
    top: kept.length,
    out_of_order: "0",
  }
}

interface FixtureTopActivityPlan {
  readonly section: string
  readonly cuts: readonly ActivityCut[]
  readonly intervals: number
  readonly group: readonly string[]
}

function fixtureTopActivityPlan(surface: TopActivitySurface, level?: TopActivityRelationLevel): FixtureTopActivityPlan {
  if (surface === "postgresql_statements") return { section: "pg_stat_statements", cuts: STATEMENT_CUTS, intervals: 60, group: [] }
  if (surface === "postgresql_plans") return { section: "pg_store_plans", cuts: PLAN_CUTS, intervals: 60, group: [] }
  if (surface === "processes") return { section: "os_process", cuts: PROCESS_CUTS, intervals: 60, group: ["comm"] }
  if (surface === "postgresql_databases") return { section: "pg_stat_database", cuts: DATABASE_CUTS, intervals: 60, group: [] }
  if (surface === "cgroup_cpu") return { section: "os_cgroup_cpu", cuts: CGROUP_CPU_CUTS, intervals: 60, group: [] }
  if (surface === "cgroup_io") return { section: "os_cgroup_io", cuts: CGROUP_IO_CUTS, intervals: 60, group: [] }
  const group = level === "schema"
    ? ["datname", "schemaname"]
    : level === "database" ? ["datname"] : level === "tablespace" ? ["tablespace"] : []
  return surface === "postgresql_tables"
    ? { section: "pg_stat_user_tables", cuts: TABLE_CUTS, intervals: 12, group }
    : { section: "pg_stat_user_indexes", cuts: INDEX_CUTS, intervals: 12, group }
}

function fixtureConversion(
  cut: ActivityCut,
  fixture: HourData | null,
  hourEnd: number,
): { readonly scale: number; readonly unit: ActivityCut["kind"] | "count" } {
  if (cut.scaleBy === "kib") return { scale: 1_024, unit: cut.kind }
  if (cut.scaleBy === undefined) return { scale: 1, unit: cut.kind }
  const candidates = cut.scaleBy === "block_size"
    ? (fixture?.sections.pg_settings ?? []).flatMap((row) => fixtureText(row.values.name) === "block_size"
      ? [{ at: row.timestamp, value: fixturePositive(row.values.setting) }]
      : [])
    : (fixture?.sections.instance_metadata ?? []).map((row) => ({
      at: row.timestamp,
      value: fixturePositive(row.values.clock_ticks_per_sec),
    }))
  const latest = candidates
    .filter(({ at, value }) => at <= hourEnd && value !== null)
    .sort((left, right) => right.at - left.at)[0]?.value ?? null
  if (latest === null) return { scale: 1, unit: "count" }
  return cut.scaleBy === "block_size"
    ? { scale: latest, unit: cut.kind }
    : { scale: 1 / latest, unit: cut.kind }
}

function fixtureDefinition(
  cut: ActivityCut,
  unit: ActivityCut["kind"] | "count",
  grouped: boolean,
): TopActivityDefinition {
  const totalUnit = unit as TopActivityUnit
  const cellUnit = (cut.class === "cumulative" ? `${unit}_per_second` : unit) as TopActivityUnit
  const ranking = cut.class === "cumulative"
    ? grouped ? "sum_member_window_delta_desc" : "whole_window_delta_desc"
    : grouped ? "sum_member_window_max_desc" : "whole_window_max_desc"
  return {
    class: cut.class,
    cell_unit: cellUnit,
    total_unit: totalUnit,
    ranking,
    metric_description: `Recorded ${cut.id.replaceAll("_", " ")}.`,
    cell_formula: cut.class === "cumulative"
      ? "Nonnegative endpoint delta divided by positive observed seconds; null without two usable endpoints."
      : "The last usable reading assigned to the interval; null without a usable reading.",
    total_formula: cut.class === "cumulative"
      ? "Nonnegative whole-hour endpoint delta."
      : "Maximum usable reading in the hour.",
  }
}

function fixtureTopActivityEntity(surface: TopActivitySurface, row: DataRow | undefined): TopActivityEntity {
  const values = row?.values ?? {}
  if (surface === "postgresql_statements") return {
    kind: "postgresql_statement",
    query_id: fixtureI64(values.queryid, true),
    role_oid: fixtureU32(values.userid),
    database_oid: fixtureU32(values.dbid),
    top_level: typeof values.toplevel === "boolean" ? values.toplevel : null,
    database_name: fixtureText(values.datname),
    role_name: fixtureText(values.usename),
  }
  if (surface === "postgresql_plans") return {
    kind: "postgresql_plan",
    role_oid: fixtureU32(values.userid),
    database_oid: fixtureU32(values.dbid),
    entry_query_id: fixtureI64(values.queryid) ?? "0",
    plan_id: fixtureI64(values.planid) ?? "0",
    database_name: fixtureText(values.datname),
    role_name: fixtureText(values.usename),
  }
  if (surface === "postgresql_tables") return {
    kind: "postgresql_table",
    database_oid: fixtureU32(values.datid),
    relation_oid: fixtureU32(values.relid),
    database_name: fixtureText(values.datname) ?? "",
    schema_name: fixtureText(values.schemaname) ?? "",
    relation_name: fixtureText(values.relname) ?? "",
  }
  if (surface === "postgresql_indexes") return {
    kind: "postgresql_index",
    database_oid: fixtureU32(values.datid),
    index_oid: fixtureU32(values.indexrelid),
    database_name: fixtureText(values.datname) ?? "",
    schema_name: fixtureText(values.schemaname) ?? "",
    table_name: fixtureText(values.relname) ?? "",
    index_name: fixtureText(values.indexrelname) ?? "",
  }
  if (surface === "postgresql_databases") return {
    kind: "postgresql_database",
    database_oid: fixtureU32(values.datid),
    database_name: fixtureText(values.datname),
  }
  if (surface === "cgroup_cpu") return { kind: "cgroup_cpu", path: fixtureText(values.cgroup_path) ?? "" }
  if (surface === "cgroup_io") return {
    kind: "cgroup_io_device",
    path: fixtureText(values.cgroup_path) ?? "",
    major: fixtureU32(values.major),
    minor: fixtureU32(values.minor),
  }
  return { kind: "process_command", command: fixtureText(values.comm) ?? "" }
}

function fixtureGroupEntity(
  level: TopActivityRelationLevel | null,
  values: readonly (string | null)[],
): TopActivityEntity {
  if (level === "schema") return {
    kind: "postgresql_relation_schema",
    database_name: values[0] ?? "",
    schema_name: values[1] ?? "",
  }
  if (level === "database") return { kind: "postgresql_relation_database", database_name: values[0] ?? "" }
  if (level === "tablespace") return { kind: "postgresql_tablespace", tablespace_name: values[0] ?? null }
  return { kind: "process_command", command: values[0] ?? "" }
}

function heatmapIntervalsAsStrings(hour: number, columns: number): readonly { readonly start: string; readonly end: string }[] {
  return Array.from({ length: columns }, (_, index) => ({
    start: String(hour + Math.floor((index * HOUR_MICROS) / columns)),
    end: String(hour + Math.floor(((index + 1) * HOUR_MICROS) / columns) - 1),
  }))
}

function fixturePositive(stored: Cell | undefined): number | null {
  const value = typeof stored === "number" ? stored : typeof stored === "string" ? Number(stored) : Number.NaN
  return Number.isFinite(value) && value > 0 ? value : null
}

function fixtureU32(stored: Cell | undefined): number {
  const value = typeof stored === "number" ? stored : typeof stored === "string" ? Number(stored) : 0
  return Number.isInteger(value) && value >= 0 && value <= 4_294_967_295 ? value : 0
}

function fixtureI64(stored: Cell | undefined, nullable = false): string | null {
  const value = fixtureText(stored)
  return value !== null && /^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$/.test(value) ? value : nullable ? null : "0"
}

function fixtureLayout(typeId: string | undefined): number | null {
  const parsed = typeId === undefined ? Number.NaN : Number(typeId)
  return Number.isInteger(parsed) && parsed > 0 && parsed <= 4_294_967_295 ? parsed : null
}

function finiteSum(left: number | null, right: number): number | null {
  const sum = (left ?? 0) + right
  return Number.isFinite(sum) ? sum : null
}

function finiteOrNull(value: number | null): number | null {
  return value !== null && Number.isFinite(value) ? value : null
}

function finiteMaximum(values: readonly (number | null)[]): number | null {
  let maximum: number | null = null
  for (const value of values) if (value !== null && Number.isFinite(value)) maximum = Math.max(maximum ?? value, value)
  return maximum
}

function compareFixtureTotals(left: number | null, right: number | null): number {
  if (left === null && right === null) return 0
  if (left === null) return 1
  if (right === null) return -1
  return right - left
}

const TOP_ACTIVITY_SURFACES = new Set<TopActivitySurface>([
  "postgresql_statements", "postgresql_plans", "postgresql_tables", "postgresql_indexes",
  "processes", "postgresql_databases", "cgroup_cpu", "cgroup_io",
])
const TOP_ACTIVITY_METRICS = new Set<TopActivityMetric>([
  "exec_time", "calls", "rows", "shared_read", "shared_dirtied", "temp_written", "wal_bytes",
  "writes", "seq_read", "heap_read", "dead_tuples", "autovacuum_time",
  "idx_scan", "idx_tup_read", "idx_blks_read", "cpu", "rss", "io_read", "io_write", "majflt", "run_delay",
  "commits", "rollbacks", "db_read", "temp_bytes", "deadlocks",
  "cg_cpu", "cg_throttled", "cg_read", "cg_write", "cg_rios", "cg_wios",
])
const TOP_ACTIVITY_UNITS = new Set<TopActivityUnit>([
  "count", "count_per_second", "bytes", "bytes_per_second",
  "milliseconds", "milliseconds_per_second", "seconds", "seconds_per_second",
  "microseconds", "microseconds_per_second", "nanoseconds", "nanoseconds_per_second",
])
const TOP_ACTIVITY_RANKINGS = new Set([
  "whole_window_delta_desc", "whole_window_max_desc",
  "sum_member_window_delta_desc", "sum_member_window_max_desc",
] as const)

export function parseTopActivityResult(
  stored: unknown,
  expected?: {
    readonly hour: number
    readonly surface: TopActivitySurface
    readonly metric: TopActivityMetric
    readonly level: TopActivityRelationLevel | null
  },
): HeatmapView {
  const value = topObject(stored, "top activity result")
  const hourStart = topI64(value.hour_start, "top activity hour_start")
  const hourEnd = topI64(value.hour_end, "top activity hour_end")
  if (BigInt(hourEnd) !== BigInt(hourStart) + BigInt(HOUR_MICROS - 1)) throw new Error("top activity hour bounds are invalid")
  const surface = topEnum(value.surface, TOP_ACTIVITY_SURFACES, "top activity surface")
  const metric = topEnum(value.metric, TOP_ACTIVITY_METRICS, "top activity metric")
  const level = value.level === null
    ? null
    : topEnum(value.level, new Set<TopActivityRelationLevel>(["object", "schema", "database", "tablespace"]), "top activity level")
  if ((surface === "postgresql_tables" || surface === "postgresql_indexes") !== (level !== null)) {
    throw new Error("top activity level is invalid for its surface")
  }
  if (expected !== undefined && (
    hourStart !== String(expected.hour)
    || surface !== expected.surface
    || metric !== expected.metric
    || level !== expected.level
  )) throw new Error("top activity response does not match its request")

  const definitionValue = topObject(value.definition, "top activity definition")
  const metricClass = topEnum(definitionValue.class, new Set(["cumulative", "gauge"] as const), "top activity class")
  const definition: TopActivityDefinition = {
    class: metricClass,
    cell_unit: topEnum(definitionValue.cell_unit, TOP_ACTIVITY_UNITS, "top activity cell unit"),
    total_unit: topEnum(definitionValue.total_unit, TOP_ACTIVITY_UNITS, "top activity total unit"),
    ranking: topEnum(definitionValue.ranking, TOP_ACTIVITY_RANKINGS, "top activity ranking"),
    metric_description: topNonemptyText(definitionValue.metric_description, "top activity metric description"),
    cell_formula: topNonemptyText(definitionValue.cell_formula, "top activity cell formula"),
    total_formula: topNonemptyText(definitionValue.total_formula, "top activity total formula"),
  }
  const expectedIntervals = surface === "postgresql_tables" || surface === "postgresql_indexes" ? 12 : 60
  const intervals = topArray(value.intervals, "top activity intervals").map((interval, index) => {
    const item = topObject(interval, `top activity interval ${index}`)
    return {
      start: topI64(item.start, `top activity interval ${index} start`),
      end: topI64(item.end, `top activity interval ${index} end`),
    }
  })
  if (intervals.length !== expectedIntervals) throw new Error("top activity interval count is invalid")
  for (let index = 0; index < intervals.length; index += 1) {
    const interval = intervals[index]!
    const expectedStart = BigInt(hourStart) + (BigInt(HOUR_MICROS) * BigInt(index)) / BigInt(expectedIntervals)
    const expectedEnd = BigInt(hourStart) + (BigInt(HOUR_MICROS) * BigInt(index + 1)) / BigInt(expectedIntervals) - 1n
    if (BigInt(interval.start) !== expectedStart || BigInt(interval.end) !== expectedEnd) {
      throw new Error(`top activity interval ${index} bounds are invalid`)
    }
  }
  const rows = topArray(value.rows, "top activity rows").map((row, index) => parseTopActivityRow(row, index, expectedIntervals))
  if (rows.some((row) => !topActivityRowMatches(surface, level, row))) {
    throw new Error("top activity row is invalid for its surface")
  }
  const totals = parseTopActivityBand(value.totals, "top activity totals", expectedIntervals)
  const others = parseTopActivityBand(value.others, "top activity others", expectedIntervals)
  const entityCount = topNonNegativeInteger(value.entity_count, "top activity entity_count")
  const othersCount = topNonNegativeInteger(value.others_count, "top activity others_count")
  const top = topNonNegativeInteger(value.top, "top activity top")
  if (top !== rows.length || entityCount < rows.length || othersCount !== entityCount - rows.length) {
    throw new Error("top activity result counts are inconsistent")
  }
  return {
    hour_start: hourStart,
    hour_end: hourEnd,
    surface,
    metric,
    level,
    definition,
    intervals,
    rows,
    totals,
    others,
    entity_count: entityCount,
    others_count: othersCount,
    top,
    out_of_order: topU64(value.out_of_order, "top activity out_of_order"),
  }
}

function topActivityRowMatches(
  surface: TopActivitySurface,
  level: TopActivityRelationLevel | null,
  row: HeatmapViewRow,
): boolean {
  if (surface === "postgresql_statements") return row.entity.kind === "postgresql_statement" && row.recorded_layout !== null && row.members === null
  if (surface === "postgresql_plans") return row.entity.kind === "postgresql_plan" && row.recorded_layout !== null && row.members === null
  if (surface === "postgresql_databases") return row.entity.kind === "postgresql_database" && row.recorded_layout !== null && row.members === null
  if (surface === "cgroup_cpu") return row.entity.kind === "cgroup_cpu" && row.recorded_layout !== null && row.members === null
  if (surface === "cgroup_io") return row.entity.kind === "cgroup_io_device" && row.recorded_layout !== null && row.members === null
  if (surface === "processes") return row.entity.kind === "process_command" && row.recorded_layout === null && row.members !== null
  if (level === "object") {
    const kind = surface === "postgresql_tables" ? "postgresql_table" : "postgresql_index"
    return row.entity.kind === kind && row.recorded_layout !== null && row.members === null
  }
  const kind = level === "schema" ? "postgresql_relation_schema"
    : level === "database" ? "postgresql_relation_database" : "postgresql_tablespace"
  return row.entity.kind === kind && row.recorded_layout === null && row.members !== null
}

function parseTopActivityRow(stored: unknown, index: number, intervals: number): HeatmapViewRow {
  const value = topObject(stored, `top activity row ${index}`)
  const recordedLayout = value.recorded_layout === null
    ? null
    : topPositiveU32(value.recorded_layout, `top activity row ${index} recorded_layout`)
  const members = value.members === null
    ? null
    : topPositiveU32(value.members, `top activity row ${index} members`)
  return {
    recorded_layout: recordedLayout,
    entity: parseTopActivityEntity(value.entity, index),
    members,
    total: topNullableFinite(value.total, `top activity row ${index} total`),
    cells: topCells(value.cells, `top activity row ${index} cells`, intervals),
  }
}

function parseTopActivityBand(stored: unknown, name: string, intervals: number) {
  const value = topObject(stored, name)
  return {
    total: topNullableFinite(value.total, `${name} total`),
    cells: topCells(value.cells, `${name} cells`, intervals),
  }
}

function parseTopActivityEntity(stored: unknown, index: number): TopActivityEntity {
  const value = topObject(stored, `top activity row ${index} entity`)
  const name = `top activity row ${index} entity`
  const kind = topNonemptyText(value.kind, `${name} kind`)
  if (kind === "postgresql_statement") return {
    kind,
    query_id: value.query_id === null ? null : topI64(value.query_id, `${name} query_id`),
    role_oid: topU32(value.role_oid, `${name} role_oid`),
    database_oid: topU32(value.database_oid, `${name} database_oid`),
    top_level: value.top_level === null ? null : topBoolean(value.top_level, `${name} top_level`),
    database_name: topNullableText(value.database_name, `${name} database_name`),
    role_name: topNullableText(value.role_name, `${name} role_name`),
  }
  if (kind === "postgresql_plan") return {
    kind,
    role_oid: topU32(value.role_oid, `${name} role_oid`),
    database_oid: topU32(value.database_oid, `${name} database_oid`),
    entry_query_id: topI64(value.entry_query_id, `${name} entry_query_id`),
    plan_id: topI64(value.plan_id, `${name} plan_id`),
    database_name: topNullableText(value.database_name, `${name} database_name`),
    role_name: topNullableText(value.role_name, `${name} role_name`),
  }
  if (kind === "postgresql_table") return {
    kind,
    database_oid: topU32(value.database_oid, `${name} database_oid`),
    relation_oid: topU32(value.relation_oid, `${name} relation_oid`),
    database_name: topText(value.database_name, `${name} database_name`),
    schema_name: topText(value.schema_name, `${name} schema_name`),
    relation_name: topText(value.relation_name, `${name} relation_name`),
  }
  if (kind === "postgresql_index") return {
    kind,
    database_oid: topU32(value.database_oid, `${name} database_oid`),
    index_oid: topU32(value.index_oid, `${name} index_oid`),
    database_name: topText(value.database_name, `${name} database_name`),
    schema_name: topText(value.schema_name, `${name} schema_name`),
    table_name: topText(value.table_name, `${name} table_name`),
    index_name: topText(value.index_name, `${name} index_name`),
  }
  if (kind === "process_command") return { kind, command: topText(value.command, `${name} command`) }
  if (kind === "postgresql_database") return {
    kind,
    database_oid: topU32(value.database_oid, `${name} database_oid`),
    database_name: topNullableText(value.database_name, `${name} database_name`),
  }
  if (kind === "cgroup_cpu") return { kind, path: topText(value.path, `${name} path`) }
  if (kind === "cgroup_io_device") return {
    kind,
    path: topText(value.path, `${name} path`),
    major: topU32(value.major, `${name} major`),
    minor: topU32(value.minor, `${name} minor`),
  }
  if (kind === "postgresql_relation_database") return {
    kind,
    database_name: topText(value.database_name, `${name} database_name`),
  }
  if (kind === "postgresql_relation_schema") return {
    kind,
    database_name: topText(value.database_name, `${name} database_name`),
    schema_name: topText(value.schema_name, `${name} schema_name`),
  }
  if (kind === "postgresql_tablespace") return {
    kind,
    tablespace_name: topNullableText(value.tablespace_name, `${name} tablespace_name`),
  }
  throw new Error(`${name} kind is invalid`)
}

function topCells(stored: unknown, name: string, length: number): readonly (number | null)[] {
  const cells = topArray(stored, name).map((cell, index) => topNullableFinite(cell, `${name} ${index}`))
  if (cells.length !== length) throw new Error(`${name} length is invalid`)
  return cells
}

function topObject(stored: unknown, name: string): Record<string, unknown> {
  if (stored === null || typeof stored !== "object" || Array.isArray(stored)) throw new Error(`${name} is invalid`)
  return stored as Record<string, unknown>
}

function topArray(stored: unknown, name: string): readonly unknown[] {
  if (!Array.isArray(stored)) throw new Error(`${name} is invalid`)
  return stored
}

function topEnum<T extends string>(stored: unknown, values: ReadonlySet<T>, name: string): T {
  if (typeof stored !== "string" || !values.has(stored as T)) throw new Error(`${name} is invalid`)
  return stored as T
}

function topText(stored: unknown, name: string): string {
  if (typeof stored !== "string") throw new Error(`${name} is invalid`)
  return stored
}

function topNonemptyText(stored: unknown, name: string): string {
  const text = topText(stored, name)
  if (text.length === 0) throw new Error(`${name} is invalid`)
  return text
}

function topNullableText(stored: unknown, name: string): string | null {
  return stored === null ? null : topText(stored, name)
}

function topBoolean(stored: unknown, name: string): boolean {
  if (typeof stored !== "boolean") throw new Error(`${name} is invalid`)
  return stored
}

function topNullableFinite(stored: unknown, name: string): number | null {
  if (stored === null) return null
  if (typeof stored !== "number" || !Number.isFinite(stored)) throw new Error(`${name} is invalid`)
  return stored
}

function topNonNegativeInteger(stored: unknown, name: string): number {
  if (typeof stored !== "number" || !Number.isSafeInteger(stored) || stored < 0) throw new Error(`${name} is invalid`)
  return stored
}

function topU32(stored: unknown, name: string): number {
  const value = topNonNegativeInteger(stored, name)
  if (value > 4_294_967_295) throw new Error(`${name} is invalid`)
  return value
}

function topPositiveU32(stored: unknown, name: string): number {
  const value = topU32(stored, name)
  if (value === 0) throw new Error(`${name} is invalid`)
  return value
}

function topI64(stored: unknown, name: string): string {
  if (typeof stored !== "string" || !/^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$/.test(stored)) throw new Error(`${name} is invalid`)
  const value = BigInt(stored)
  if (value < -(1n << 63n) || value > (1n << 63n) - 1n) throw new Error(`${name} is invalid`)
  return stored
}

function topU64(stored: unknown, name: string): string {
  if (typeof stored !== "string" || !/^(?:0|[1-9][0-9]*)$/.test(stored)) throw new Error(`${name} is invalid`)
  if (BigInt(stored) > (1n << 64n) - 1n) throw new Error(`${name} is invalid`)
  return stored
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
  readonly syntheticDemo?: boolean
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly findings: readonly Finding[]
  readonly findingGroups?: readonly FindingGroup[]
}): HourData {
  const rows = (name: string) => input.sections[name] ?? []
  return {
    ...input,
    syntheticDemo: input.syntheticDemo ?? false,
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
  }
}

const CELL_TEXT = 160
const RELATED_STATEMENT_TEXT_PAGE_SIZE = 1
const RELATED_STATEMENT_TEXT_FIELDS = ["query"] as const
const RELATED_STATEMENT_TEXT_FIELDS_BY_TYPE = Object.fromEntries(
  PG_STAT_STATEMENTS_TYPE_IDS.map((typeId) => [typeId, RELATED_STATEMENT_TEXT_FIELDS]),
)

// The Plans panel on a selected statement: identity plus the whole-snapshot
// counters the Plans table's totals lens shows for the same rows.
const RELATED_PLAN_FIELDS = ["planid", "calls", "total_time"] as const
const RELATED_PLAN_FIELDS_BY_TYPE = Object.fromEntries(
  PG_STORE_PLANS_TYPE_IDS.map((typeId) => [typeId, RELATED_PLAN_FIELDS]),
)
const RELATED_PLAN_PAGE_SIZE = 20

// The Statement panel on a selected plan: the text plus the counters every
// pg_stat_statements layout carries under one name or the other.
const RELATED_STATEMENT_FIELDS_BY_TYPE = Object.fromEntries(PG_STAT_STATEMENTS_TYPE_IDS.map((typeId) => [
  typeId,
  typeId === "1002001"
    ? ["query", "calls", "rows", "total_time", "mean_time"]
    : ["query", "calls", "rows", "total_exec_time", "mean_exec_time"],
]))

export interface SnapshotOptions {
  readonly filters?: Readonly<Record<string, string>>
  readonly typeId?: string
  readonly rowOrdinal?: string
  readonly fullText?: boolean
  readonly cursor?: string
  readonly search?: string
  readonly firstMatch?: boolean
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
      || chosen.cursor !== undefined || chosen.search !== undefined || chosen.firstMatch === true)
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

// The recorded plans matching a statement's identity expression, from the
// newest snapshot at or before the moment. One page is the whole answer: a
// statement with more distinct plans than the page holds is itself the story,
// and the Plans view shows the rest.
export async function loadRelatedPlanRows(
  segments: readonly SegmentBound[],
  at: number,
  search: string,
  signal: AbortSignal,
): Promise<readonly DataRow[]> {
  const [group] = snapshotRequestGroups(segments, at, [{
    section: "pg_store_plans",
    typeIds: PG_STORE_PLANS_TYPE_IDS,
    fieldsByType: RELATED_PLAN_FIELDS_BY_TYPE,
    pageSize: RELATED_PLAN_PAGE_SIZE,
  }])
  const [request] = group?.requests ?? []
  if (group === undefined || request === undefined) return []
  signal.throwIfAborted()
  const page = await loadSnapshot(group.anchor.id, at, [request], signal, undefined, { search })
  return page.sections.pg_store_plans ?? []
}

// The one statement matching a plan's identity expression. An ordinary paged
// search, not `first_match`: the server pins that shortcut to a text-only
// projection, and this panel wants the counters too.
export async function loadRelatedStatementRow(
  segments: readonly SegmentBound[],
  at: number,
  search: string,
  signal: AbortSignal,
): Promise<DataRow | null> {
  const [group] = snapshotRequestGroups(segments, at, [{
    section: "pg_stat_statements",
    typeIds: PG_STAT_STATEMENTS_TYPE_IDS,
    fieldsByType: RELATED_STATEMENT_FIELDS_BY_TYPE,
    pageSize: RELATED_STATEMENT_TEXT_PAGE_SIZE,
  }])
  const [request] = group?.requests ?? []
  if (group === undefined || request === undefined) return null
  signal.throwIfAborted()
  const page = await loadSnapshot(group.anchor.id, at, [request], signal, undefined, { fullText: true, search })
  return page.sections.pg_stat_statements?.[0] ?? null
}

export async function loadRelatedStatementTextRow(
  segments: readonly SegmentBound[],
  at: number,
  queryId: string,
  signal: AbortSignal,
): Promise<DataRow | null> {
  const search = canonicalSearch([{ key: "query_id", value: queryId }], "pg_stat_statements")
  if (search === null) return null
  const [group] = snapshotRequestGroups(segments, at, [{
    section: "pg_stat_statements",
    typeIds: PG_STAT_STATEMENTS_TYPE_IDS,
    fieldsByType: RELATED_STATEMENT_TEXT_FIELDS_BY_TYPE,
    pageSize: RELATED_STATEMENT_TEXT_PAGE_SIZE,
  }])
  const [request] = group?.requests ?? []
  if (group === undefined || request === undefined) return null

  signal.throwIfAborted()
  const page = await loadSnapshot(
    group.anchor.id,
    at,
    [request],
    signal,
    undefined,
    { firstMatch: true, fullText: true, search },
  )
  return page.sections.pg_stat_statements?.[0] ?? null
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
    ...(options.firstMatch === true ? ["first_match=1"] : []),
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
    || "cursor" in value || "search" in value || "firstMatch" in value) {
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

async function requestJson(path: string, signal: AbortSignal): Promise<unknown> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    let response: Response
    try {
      response = await apiFetch(path, { headers: { Accept: "application/json" }, signal })
    } catch (error) {
      signal.throwIfAborted()
      if (attempt === 0 && error instanceof TypeError) continue
      throw error
    }
    if (!response.ok) throw new Error(`HTTP ${response.status} for ${path}`)
    try {
      return await response.json()
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
