import type { Cell, DataRow, SectionRequest, SnapshotOptions } from "./api"
import { intervalMetric } from "./postgres-metrics"

export const RELATION_SECTIONS = ["pg_stat_user_tables", "pg_stat_user_indexes"] as const
export const RELATION_GROUPS = ["database", "schema", "object"] as const
export const TABLE_LENSES = ["access", "changes", "maintenance", "size_buffers", "freeze"] as const
export const INDEX_LENSES = ["usage", "low_activity", "size_buffers", "state"] as const

export type RelationSection = typeof RELATION_SECTIONS[number]
export type RelationGroup = typeof RELATION_GROUPS[number]
export type TableLens = typeof TABLE_LENSES[number]
export type IndexLens = typeof INDEX_LENSES[number]
export type RelationLens = TableLens | IndexLens

export type RelationColumnKind = "text" | "id" | "number" | "timestamp" | "bool"
export type RelationColumnUnit = "none" | "count" | "bytes" | "per_second" | "percent" | "milliseconds"

export interface RelationWireColumn {
  readonly name: string
  readonly kind: RelationColumnKind
  readonly unit: RelationColumnUnit
  readonly nullable: boolean
}

export interface RelationLayout {
  readonly logicalName: RelationSection
  readonly group: RelationGroup
  readonly columns: readonly RelationWireColumn[]
}

export interface RelationSource {
  readonly typeId: string
  readonly ordinal: string
  readonly timestamp: number
}

/** An explicit logical row. Aggregate rows deliberately have no physical locator. */
export interface RelationRow {
  readonly segmentId: string
  readonly logicalName: RelationSection
  readonly group: RelationGroup
  readonly key: Readonly<Record<string, string>>
  readonly values: Readonly<Record<string, Cell>>
  readonly sampleFrom: number | null
  readonly sampleTo: number | null
  readonly source: RelationSource | null
}

export interface RelationRequest extends SectionRequest {
  readonly group: RelationGroup
  readonly filters?: Readonly<Record<string, string>>
}

export interface RelationDetailTarget {
  readonly segmentId: string
  readonly at: number
  readonly request: SectionRequest
  readonly options: SnapshotOptions
}

export interface RelationNavigation {
  readonly section: RelationSection
  readonly group: RelationGroup
  readonly filters: Readonly<Record<string, string>>
  readonly selectedKey: string | null
}

interface LensSpec {
  readonly object: readonly string[]
  readonly aggregate: readonly string[]
  readonly order: string
  readonly history: string
}

const scanTimes = timestamps("last_seq_scan", "last_idx_scan")
const maintenanceTimes = timestamps("last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum")
const indexScanTime = timestamps("last_idx_scan")

const tableLenses: Readonly<Record<TableLens, LensSpec>> = {
  access: lens(
    ["seq_scan", "idx_scan", "sequential_share_pct", "seq_tup_read", "idx_tup_fetch", "seq_tuples_per_scan", "idx_tuples_per_scan", "last_seq_scan", "last_idx_scan"],
    ["seq_scan", "idx_scan", "sequential_share_pct", "seq_tup_read", "idx_tup_fetch", "seq_tuples_per_scan", "idx_tuples_per_scan", ...scanTimes],
    "seq_scan",
  ),
  changes: lens(
    ["n_tup_ins", "n_tup_upd", "n_tup_del", "n_tup_hot_upd", "n_tup_newpage_upd", "dead_pct", "hot_pct", "new_page_pct", "n_live_tup", "n_dead_tup", "n_mod_since_analyze", "n_ins_since_vacuum"],
    undefined,
    "n_tup_upd",
  ),
  maintenance: lens(
    ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", "last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum", "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", ...maintenanceTimes, "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    "autovacuum_count",
  ),
  size_buffers: lens(
    ["main_fork_bytes", "toast_bytes", "reltuples", "toast_n_live_tup", "toast_n_dead_tup", "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit", "buffer_hit_pct"],
    undefined,
    "main_fork_bytes",
  ),
  freeze: lens(
    ["xid_age", "mxid_age", "n_ins_since_vacuum", "last_vacuum", "last_autovacuum"],
    ["xid_age", "mxid_age", "n_ins_since_vacuum", ...timestamps("last_vacuum", "last_autovacuum")],
    "xid_age",
  ),
}

const indexLenses: Readonly<Record<IndexLens, LensSpec>> = {
  usage: lens(
    ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", "last_idx_scan"],
    ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", ...indexScanTime],
    "idx_scan",
  ),
  low_activity: lens(
    ["no_scans", "idx_scan", "last_idx_scan", "main_fork_bytes"],
    ["no_scan_count", "known_scan_count", "idx_scan", ...indexScanTime, "main_fork_bytes"],
    "main_fork_bytes",
    "idx_scan",
  ),
  size_buffers: lens(
    ["main_fork_bytes", "idx_blks_read", "idx_blks_hit", "buffer_hit_pct"],
    undefined,
    "main_fork_bytes",
  ),
  state: lens(
    ["state_severity", "indisvalid", "indisready", "indisunique", "indisprimary", "indisexclusion", "amname", "tablespace"],
    ["state_severity", "invalid_count", "not_ready_count", "unique_count", "primary_count", "exclusion_count"],
    "state_severity",
    "idx_scan",
  ),
}

const identities: Readonly<Record<RelationSection, Readonly<Record<RelationGroup, readonly string[]>>>> = {
  pg_stat_user_tables: {
    database: ["datname", "datid", "table_count"],
    schema: ["schemaname", "datname", "datid", "table_count"],
    object: ["relname", "schemaname", "datname", "datid", "relid", "tablespace"],
  },
  pg_stat_user_indexes: {
    database: ["datname", "datid", "index_count"],
    schema: ["schemaname", "datname", "datid", "index_count"],
    object: ["indexrelname", "relname", "schemaname", "datname", "datid", "indexrelid", "relid", "tablespace", "amname"],
  },
}

const derived = new Set([
  "dead_pct", "hot_pct", "new_page_pct", "sequential_share_pct", "seq_tuples_per_scan",
  "idx_tuples_per_scan", "buffer_hit_pct", "vacuum_mean_ms", "autovacuum_mean_ms",
  "analyze_mean_ms", "autoanalyze_mean_ms", "tuples_per_scan", "fetches_per_scan", "state_severity",
])

const notSortable = new Set([
  "datname", "schemaname", "relname", "indexrelname", "tablespace", "amname", "no_scans",
  "indisvalid", "indisready", "indisunique", "indisprimary", "indisexclusion",
])

export function relationFields(section: RelationSection, lensName: RelationLens, group: RelationGroup): readonly string[] {
  const spec = lensSpec(section, lensName)
  return unique([...identities[section][group], ...(group === "object" ? spec.object : spec.aggregate)])
}

export function relationDefaultOrder(section: RelationSection, lensName: RelationLens): string {
  return lensSpec(section, lensName).order
}

export function relationHistoryField(section: RelationSection, lensName: RelationLens): string {
  return lensSpec(section, lensName).history
}

export function relationSortToken(field: string): string {
  return derived.has(field) ? `derived.${field}` : field
}

export function relationRequest(section: RelationSection, lensName: RelationLens, group: RelationGroup): RelationRequest {
  const fields = relationFields(section, lensName, group)
  const order = Object.fromEntries(fields.filter((field) => !notSortable.has(field)).map((field) => [field, [relationSortToken(field)]]))
  const chosen = relationDefaultOrder(section, lensName)
  return {
    section,
    group,
    fields,
    pageSize: 200,
    defaultOrder: [relationSortToken(chosen)],
    fallbackOrder: [relationSortToken(chosen)],
    order,
    ...(section === "pg_stat_user_indexes" && lensName === "low_activity" ? { filters: { no_scans: "true" } } : {}),
  }
}

export function isRelationSection(value: string): value is RelationSection {
  return (RELATION_SECTIONS as readonly string[]).includes(value)
}

export function isRelationGroup(value: unknown): value is RelationGroup {
  return typeof value === "string" && (RELATION_GROUPS as readonly string[]).includes(value)
}

export function isRelationLens(section: RelationSection, value: string): value is RelationLens {
  return (section === "pg_stat_user_tables" ? TABLE_LENSES : INDEX_LENSES as readonly RelationLens[]).includes(value as RelationLens)
}

export function parseRelationLayout(record: Readonly<Record<string, unknown>>): RelationLayout | null {
  if (record.record !== "relation_layout") return null
  const logicalName = relationSection(record.logical_name)
  const group = relationGroup(record.group)
  if (!Array.isArray(record.columns)) invalid("relation columns")
  const seen = new Set<string>()
  const columns = record.columns.map((stored) => {
    const column = object(stored, "relation column")
    const name = text(column.name, "relation column name")
    const kind = oneOf(column.kind, ["text", "id", "number", "timestamp", "bool"] as const, "relation column kind")
    const unit = oneOf(column.unit, ["none", "count", "bytes", "per_second", "percent", "milliseconds"] as const, "relation column unit")
    if (typeof column.nullable !== "boolean" || seen.has(name)) invalid("relation column")
    seen.add(name)
    if (group !== "object" && name === "indexdef") invalid("aggregate index definition")
    return { name, kind, unit, nullable: column.nullable }
  })
  return { logicalName, group, columns }
}

export function relationLayoutKey(layout: Pick<RelationLayout, "logicalName" | "group">): string {
  return `${layout.logicalName}:${layout.group}`
}

export function parseRelationRow(
  record: Readonly<Record<string, unknown>>,
  layouts: ReadonlyMap<string, RelationLayout>,
  segmentId: string,
): RelationRow | null {
  if (record.record !== "relation") return null
  const logicalName = relationSection(record.logical_name)
  const group = relationGroup(record.group)
  const layout = layouts.get(relationLayoutKey({ logicalName, group }))
  if (layout === undefined) invalid("relation before layout")
  const key = relationKey(logicalName, group, object(record.key, "relation key"))
  const stored = object(record.values, "relation values")
  const expected = new Set(layout.columns.map(({ name }) => name))
  if (Object.keys(stored).some((name) => !expected.has(name)) || layout.columns.some(({ name }) => !Object.hasOwn(stored, name))) {
    invalid("relation values")
  }
  const metrics = Object.fromEntries(layout.columns.map((column) => {
    const value = stored[column.name] as Cell
    if (value === null && !column.nullable) invalid("relation null")
    return [column.name, value]
  }))
  if (Object.keys(key).some((name) => Object.hasOwn(metrics, name))) invalid("relation identity collision")
  const source = record.source === null ? null : relationSource(record.source)
  if ((group === "object") !== (source !== null)) invalid("relation source")
  const sampleFrom = moment(record.sample_from, "relation sample start")
  const sampleTo = moment(record.sample_to, "relation sample end")
  if (sampleFrom !== null && sampleTo !== null && sampleFrom > sampleTo) invalid("relation sample interval")
  return {
    segmentId,
    logicalName,
    group,
    key,
    values: { ...key, ...metrics },
    sampleFrom,
    sampleTo,
    source,
  }
}

export function relationRowKey(row: RelationRow): string {
  const identity = row.group === "database"
    ? [row.key.datid, row.key.datname]
    : row.group === "schema"
      ? [row.key.datid, row.key.schemaname]
      : [row.key.datid, row.logicalName === "pg_stat_user_tables" ? row.key.relid : row.key.indexrelid]
  return JSON.stringify([row.logicalName, row.group, ...identity])
}

export function relationDetailTarget(row: RelationRow): RelationDetailTarget {
  if (row.group !== "object" || row.source === null) invalid("relation detail source")
  return {
    segmentId: row.segmentId,
    at: row.source.timestamp,
    request: { section: row.logicalName, typeId: row.source.typeId },
    options: {
      typeId: row.source.typeId,
      rowOrdinal: row.source.ordinal,
      ...(row.logicalName === "pg_stat_user_indexes" ? { fullText: true } : {}),
    },
  }
}

export function relationDrill(row: RelationRow): RelationNavigation | null {
  if (row.group === "object") return null
  const filters = row.group === "database"
    ? { datid: row.key.datid ?? "" }
    : { datid: row.key.datid ?? "", schemaname: row.key.schemaname ?? "" }
  return { section: row.logicalName, group: row.group === "database" ? "schema" : "object", filters, selectedKey: null }
}

export function linkedRelation(row: RelationRow): RelationNavigation | null {
  if (row.group !== "object") return null
  const datid = row.key.datid ?? ""
  const relid = scalarText(row.values.relid, "linked table oid")
  if (row.logicalName === "pg_stat_user_tables") {
    return { section: "pg_stat_user_indexes", group: "object", filters: { datid, relid }, selectedKey: null }
  }
  return {
    section: "pg_stat_user_tables",
    group: "object",
    filters: { datid, relid },
    selectedKey: JSON.stringify(["pg_stat_user_tables", "object", datid, relid]),
  }
}

/** Builds gauge samples or reset-safe per-second counter rates for one exact object. */
export function relationHistory(rows: readonly DataRow[], field: string): readonly { readonly segmentId: string; readonly timestamp: number; readonly value: number | null }[] {
  const ordered = [...rows].sort((left, right) => left.timestamp - right.timestamp)
  if (!counters.has(field)) return ordered.map((row) => ({ segmentId: row.segmentId, timestamp: row.timestamp, value: numeric(row.values[field]) }))
  return ordered.map((row, index) => {
    const before = ordered[index - 1]
    const value = before === undefined || before.typeId !== row.typeId ? null : intervalMetric(before, row, field)
    return { segmentId: row.segmentId, timestamp: row.timestamp, value }
  })
}

export function intervalHasNoScans(row: RelationRow): boolean | null {
  const scans = numeric(row.values.idx_scan)
  return scans === null ? null : scans === 0
}

const counters = new Set([
  "seq_scan", "seq_tup_read", "idx_scan", "idx_tup_fetch", "n_tup_ins", "n_tup_upd", "n_tup_del", "n_tup_hot_upd", "n_tup_newpage_upd",
  "vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", "total_vacuum_time", "total_autovacuum_time", "total_analyze_time", "total_autoanalyze_time",
  "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit", "idx_tup_read",
])

function timestamps(...names: readonly string[]): readonly string[] {
  return names.flatMap((name) => [`${name}_oldest`, `${name}_latest`, `${name}_never_count`])
}

function lens(objectFields: readonly string[], aggregateFields: readonly string[] | undefined, order: string, history = order): LensSpec {
  return { object: objectFields, aggregate: aggregateFields ?? objectFields, order, history }
}

function lensSpec(section: RelationSection, value: RelationLens): LensSpec {
  if (!isRelationLens(section, value)) invalid("relation lens")
  return section === "pg_stat_user_tables" ? tableLenses[value as TableLens] : indexLenses[value as IndexLens]
}

function relationSection(value: unknown): RelationSection {
  if (typeof value !== "string" || !isRelationSection(value)) invalid("relation section")
  return value
}

function relationGroup(value: unknown): RelationGroup {
  if (!isRelationGroup(value)) invalid("relation group")
  return value
}

function relationKey(section: RelationSection, group: RelationGroup, stored: Readonly<Record<string, unknown>>): Readonly<Record<string, string>> {
  const names = group === "database"
      ? ["datid", "datname"]
    : group === "schema"
      ? ["datid", "datname", "schemaname"]
      : section === "pg_stat_user_tables"
        ? ["datid", "datname", "schemaname", "relid", "relname"]
        : ["datid", "datname", "schemaname", "relid", "relname", "indexrelid", "indexrelname"]
  if (Object.keys(stored).length !== names.length || names.some((name) => !Object.hasOwn(stored, name))) invalid("relation key")
  return Object.fromEntries(names.map((name) => [name, name.endsWith("id") ? decimal(stored[name], name) : text(stored[name], name)]))
}

function relationSource(value: unknown): RelationSource {
  const source = object(value, "relation source")
  return {
    typeId: decimal(source.type_id, "source type id"),
    ordinal: decimal(source.ordinal, "source ordinal"),
    timestamp: moment(source.timestamp, "source timestamp") ?? invalid("source timestamp"),
  }
}

function moment(value: unknown, label: string): number | null {
  if (value === null) return null
  if (typeof value !== "string" || !/^-?\d+$/.test(value)) invalid(label)
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed)) invalid(label)
  return parsed
}

function object(value: unknown, label: string): Readonly<Record<string, unknown>> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) invalid(label)
  return value as Readonly<Record<string, unknown>>
}

function text(value: unknown, label: string): string {
  if (typeof value !== "string") invalid(label)
  return value
}

function scalarText(value: Cell | undefined, label: string): string {
  if (typeof value !== "string" && typeof value !== "number") invalid(label)
  return String(value)
}

function decimal(value: unknown, label: string): string {
  const stored = text(value, label)
  if (!/^\d+$/.test(stored)) invalid(label)
  return stored
}

function oneOf<const T extends string>(value: unknown, values: readonly T[], label: string): T {
  if (typeof value !== "string" || !(values as readonly string[]).includes(value)) invalid(label)
  return value as T
}

function numeric(value: Cell | undefined): number | null {
  const stored = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN
  return Number.isFinite(stored) ? stored : null
}

function unique(values: readonly string[]): readonly string[] {
  return [...new Set(values)]
}

function invalid(label: string): never {
  throw new Error(`${label} is invalid`)
}
