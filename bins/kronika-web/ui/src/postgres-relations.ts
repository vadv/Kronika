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

export interface RelationWireColumn {
  readonly name: string
  readonly kind: "text" | "id" | "number" | "timestamp" | "bool"
  readonly unit: "none" | "count" | "bytes" | "per_second" | "percent" | "milliseconds"
  readonly nullable: boolean
}

export interface RelationLayout {
  readonly logicalName: RelationSection
  readonly group: RelationGroup
  readonly columns: readonly RelationWireColumn[]
}

/** An explicit logical row. Aggregate rows deliberately have no physical locator. */
export interface RelationRow {
  readonly group: RelationGroup
}

export interface RelationRequest extends SectionRequest {
  readonly group: RelationGroup
  readonly filters?: Readonly<Record<string, string>>
}

export interface RelationDetailTarget {
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
}

const scanTimes = timestamps("last_seq_scan", "last_idx_scan")
const maintenanceTimes = timestamps("last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum")
const indexScanTime = timestamps("last_idx_scan")

const tableLenses: Readonly<Record<TableLens, LensSpec>> = {
  access: lens(
    ["tuple_throughput", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", "last_seq_scan", "last_seq_scan_never", "last_idx_scan", "last_idx_scan_never"],
    ["tuple_throughput", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", ...scanTimes],
    "tuple_throughput",
  ),
  changes: lens(
    ["dml_total", "insert_share_pct", "update_share_pct", "delete_share_pct", "hot_pct", "new_page_pct", "dead_pct", "n_mod_since_analyze", "n_ins_since_vacuum"],
    undefined,
    "dml_total",
  ),
  maintenance: lens(
    ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", "last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum", "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", ...maintenanceTimes, "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    "autovacuum_count",
  ),
  size_buffers: lens(
    ["displayed_storage_bytes", "main_fork_bytes", "toast_bytes", "toast_share_pct", "reltuples", "toast_n_live_tup", "toast_n_dead_tup", "toast_dead_pct", "buffer_hit_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct", "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit"],
    undefined,
    "displayed_storage_bytes",
  ),
  freeze: lens(
    ["xid_age", "mxid_age", "n_ins_since_vacuum", "last_vacuum", "last_autovacuum"],
    ["xid_age", "mxid_age", "n_ins_since_vacuum", ...timestamps("last_vacuum", "last_autovacuum")],
    "xid_age",
  ),
}

const indexLenses: Readonly<Record<IndexLens, LensSpec>> = {
  usage: lens(
    ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", "last_idx_scan", "last_idx_scan_never"],
    ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", ...indexScanTime],
    "idx_scan",
  ),
  low_activity: lens(
    ["no_scans", "idx_scan", "last_idx_scan", "last_idx_scan_never", "main_fork_bytes"],
    ["no_scan_count", "known_scan_count", "idx_scan", ...indexScanTime, "main_fork_bytes"],
    "main_fork_bytes",
  ),
  size_buffers: lens(
    ["main_fork_bytes", "idx_blks_read", "idx_blks_hit", "buffer_hit_pct"],
    undefined,
    "main_fork_bytes",
  ),
  state: lens(
    ["state_severity", "indisvalid", "indisready", "indisunique", "indisprimary", "indisexclusion"],
    ["state_severity", "invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"],
    "state_severity",
  ),
}

export function relationFields(section: RelationSection, lensName: RelationLens, group: RelationGroup): readonly string[] {
  const spec = lensSpec(section, lensName)
  return [...identityFields(section, group), ...(group === "object" ? spec.object : spec.aggregate)]
}

export function relationDefaultOrder(section: RelationSection, lensName: RelationLens): string {
  return lensSpec(section, lensName).order
}

export function relationHistoryField(section: RelationSection, lensName: RelationLens): string {
  if (section === "pg_stat_user_tables") {
    if (lensName === "access") return "seq_scan"
    if (lensName === "changes") return "n_tup_upd"
    if (lensName === "size_buffers") return "main_fork_bytes"
  }
  return lensName === "low_activity" || lensName === "state" ? "idx_scan" : relationDefaultOrder(section, lensName)
}

export function relationSortToken(field: string): string {
  return field.endsWith("_pct") || field.endsWith("_per_scan") || field.endsWith("_mean_ms") || ["state_severity", "tuple_throughput", "dml_total", "displayed_storage_bytes"].includes(field) ? `derived.${field}` : field
}

const ESTIMATED_ROWS = new Set(["reltuples", "n_live_tup", "n_dead_tup", "toast_n_live_tup", "toast_n_dead_tup", "n_mod_since_analyze", "n_ins_since_vacuum"])

export function relationFieldKind(field: string): "id" | "text" | "boolean" | "timestamp" | "bytes" | "milliseconds" | "percent" | "estimated_rows" | "number" {
  if (ESTIMATED_ROWS.has(field)) return "estimated_rows"
  if (field === "no_scans" || field.startsWith("indis") || field.endsWith("_never")) return "boolean"
  if (field.endsWith("_never_count")) return "number"
  if (field.endsWith("id")) return "id"
  if (field.endsWith("name") || field === "tablespace" || field === "indexdef") return "text"
  if (field.includes("last_") || field.endsWith("_oldest") || field.endsWith("_latest")) return "timestamp"
  if (field.endsWith("_bytes")) return "bytes"
  if (field.endsWith("_mean_ms") || field.startsWith("total_") && field.endsWith("_time")) return "milliseconds"
  return field.endsWith("_pct") ? "percent" : "number"
}

export function relationRequest(section: RelationSection, lensName: RelationLens, group: RelationGroup): RelationRequest {
  const fields = relationFields(section, lensName, group)
  const order = Object.fromEntries(fields.filter((field) => {
    const kind = relationFieldKind(field)
    return kind !== "text" && kind !== "boolean" && !isRelationId(field)
  }).map((field) => [field, [relationSortToken(field)]]))
  const chosen = relationDefaultOrder(section, lensName)
  return {
    section,
    group,
    fields,
    pageSize: 200,
    defaultOrder: [relationSortToken(chosen)],
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
  if (!Array.isArray(record.columns)) invalid()
  const seen = new Set<string>()
  const columns = record.columns.map((stored) => {
    const column = object(stored)
    const name = text(column.name)
    const kind = column.kind
    const unit = column.unit
    if (typeof kind !== "string" || !/^(text|id|number|timestamp|bool)$/.test(kind)
      || typeof unit !== "string" || !/^(none|count|bytes|per_second|percent|milliseconds)$/.test(unit)) invalid()
    if (typeof column.nullable !== "boolean" || seen.has(name)) invalid()
    seen.add(name)
    if (group !== "object" && name === "indexdef") invalid("aggregate index definition")
    return { name, kind, unit, nullable: column.nullable } as RelationWireColumn
  })
  return { logicalName, group, columns }
}

export function relationRateFields(layout: RelationLayout): readonly string[] {
  return layout.columns.flatMap((column) => column.unit === "per_second" ? [column.name] : [])
}

export function relationLayoutKey(layout: Pick<RelationLayout, "logicalName" | "group">): string {
  return `${layout.logicalName}:${layout.group}`
}

export function parseRelationRow(
  record: Readonly<Record<string, unknown>>,
  layouts: ReadonlyMap<string, RelationLayout>,
  segmentId: string,
  at = 0,
): DataRow | null {
  if (record.record !== "relation") return null
  const logicalName = relationSection(record.logical_name)
  const group = relationGroup(record.group)
  const layout = layouts.get(relationLayoutKey({ logicalName, group }))
  if (layout === undefined) invalid()
  const key = relationKey(logicalName, group, object(record.key))
  const stored = object(record.values)
  const expected = new Set(layout.columns.map(({ name }) => name))
  if (Object.keys(stored).some((name) => !expected.has(name)) || layout.columns.some(({ name }) => !Object.hasOwn(stored, name))) {
    invalid("relation values")
  }
  const metrics = Object.fromEntries(layout.columns.map((column) => {
    const value = stored[column.name] as Cell
    if (value === null && !column.nullable) invalid("relation null")
    return [column.name, value]
  }))
  if (Object.keys(key).some((name) => Object.hasOwn(metrics, name))) invalid()
  const source = record.source === null ? null : object(record.source)
  if ((group === "object") !== (source !== null)) invalid("relation source")
  const sampleFrom = moment(record.sample_from)
  const sampleTo = moment(record.sample_to)
  if (sampleFrom !== null && sampleTo !== null && sampleFrom > sampleTo) invalid("relation sample interval")
  return {
    segmentId: source === null ? segmentId : text(source.segment_id),
    logicalName,
    typeId: source === null ? "" : decimal(source.type_id),
    ordinal: source === null ? "" : decimal(source.ordinal),
    timestamp: source === null ? sampleTo ?? at : moment(source.timestamp) ?? invalid(),
    values: { ...key, ...metrics },
    relation: { group },
  }
}

export function relationRowKey(row: DataRow): string {
  const group = row.relation!.group
  return relationKeyString(row.logicalName as RelationSection, group, row.values)
}

export function isRelationId(field: string): boolean {
  return field === "datid" || field === "relid" || field === "indexrelid"
}

export function relationDetailTarget(row: DataRow): RelationDetailTarget {
  if (row.logicalName !== "pg_stat_user_indexes" || row.relation?.group !== "object" || row.typeId === "" || row.ordinal === "") {
    invalid("index definition source")
  }
  return {
    at: row.timestamp,
    request: { section: row.logicalName, typeId: row.typeId, fields: ["indexdef"] },
    options: {
      typeId: row.typeId,
      rowOrdinal: row.ordinal,
      fullText: true,
    },
  }
}

export function relationDrill(row: DataRow): RelationNavigation | null {
  const relation = row.relation
  if (relation === undefined || relation.group === "object") return null
  const filters = relation.group === "database"
    ? { datid: scalarText(row.values.datid) }
    : { datid: scalarText(row.values.datid), schemaname: scalarText(row.values.schemaname) }
  return { section: row.logicalName as RelationSection, group: relation.group === "database" ? "schema" : "object", filters, selectedKey: null }
}

export function linkedRelation(row: DataRow): RelationNavigation | null {
  if (row.relation?.group !== "object") return null
  const datid = scalarText(row.values.datid)
  const relid = scalarText(row.values.relid)
  const section = row.logicalName === "pg_stat_user_tables" ? "pg_stat_user_indexes" : "pg_stat_user_tables"
  return {
    section,
    group: "object",
    filters: { datid, relid },
    selectedKey: section === "pg_stat_user_tables" ? relationKeyString(section, "object", row.values) : null,
  }
}

/** Builds gauge samples or reset-safe per-second counter rates for one exact object. */
export function relationHistory(rows: readonly DataRow[], field: string): readonly { readonly segmentId: string; readonly timestamp: number; readonly value: number | null }[] {
  const ordered = [...rows].sort((left, right) => left.timestamp - right.timestamp)
  const gauge = field === "main_fork_bytes" || field === "xid_age"
  return ordered.map((row, index) => {
    const before = ordered[index - 1]
    const value = gauge ? numeric(row.values[field]) : before === undefined || before.typeId !== row.typeId ? null : intervalMetric(before, row, field)
    return { segmentId: row.segmentId, timestamp: row.timestamp, value }
  })
}

function timestamps(...names: readonly string[]): readonly string[] {
  return names.flatMap((name) => [`${name}_oldest`, `${name}_latest`, `${name}_never_count`])
}

function lens(objectFields: readonly string[], aggregateFields: readonly string[] | undefined, order: string): LensSpec {
  return { object: objectFields, aggregate: aggregateFields ?? objectFields, order }
}

function identityFields(section: RelationSection, group: RelationGroup): readonly string[] {
  const count = section === "pg_stat_user_tables" ? "table_count" : "index_count"
  if (group === "database") return ["datname", "datid", count]
  if (group === "schema") return ["schemaname", "datname", "datid", count]
  return section === "pg_stat_user_tables"
    ? ["relname", "schemaname", "datname", "datid", "relid", "tablespace"]
    : ["indexrelname", "relname", "schemaname", "datname", "datid", "indexrelid", "relid", "tablespace", "amname"]
}

function lensSpec(section: RelationSection, value: RelationLens): LensSpec {
  if (!isRelationLens(section, value)) invalid()
  return section === "pg_stat_user_tables" ? tableLenses[value as TableLens] : indexLenses[value as IndexLens]
}

function relationSection(value: unknown): RelationSection {
  if (typeof value !== "string" || !isRelationSection(value)) invalid()
  return value
}

export function relationGroup(value: unknown): RelationGroup {
  if (!isRelationGroup(value)) invalid("relation group")
  return value
}

function relationKey(section: RelationSection, group: RelationGroup, stored: Readonly<Record<string, unknown>>): Readonly<Record<string, string>> {
  const names = relationKeyNames(section, group)
  if (Object.keys(stored).length !== names.length || names.some((name) => !Object.hasOwn(stored, name))) invalid("relation key")
  return Object.fromEntries(names.map((name) => [name, name.endsWith("id") ? decimal(stored[name]) : text(stored[name])]))
}

function relationKeyNames(section: RelationSection, group: RelationGroup): readonly string[] {
  return group === "database"
    ? ["datid", "datname"]
    : group === "schema"
      ? ["datid", "datname", "schemaname"]
      : section === "pg_stat_user_tables"
        ? ["datid", "datname", "schemaname", "relid", "relname"]
        : ["datid", "datname", "schemaname", "relid", "relname", "indexrelid", "indexrelname"]
}

function relationKeyString(section: RelationSection, group: RelationGroup, values: Readonly<Record<string, Cell>>): string {
  return JSON.stringify([section, group, ...relationKeyNames(section, group).map((name) => values[name])])
}

function moment(value: unknown): number | null {
  if (value === null) return null
  if (typeof value !== "string" || !/^-?\d+$/.test(value)) invalid()
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed)) invalid()
  return parsed
}

function object(value: unknown): Readonly<Record<string, unknown>> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) invalid()
  return value as Readonly<Record<string, unknown>>
}

function text(value: unknown): string {
  if (typeof value !== "string") invalid()
  return value
}

function scalarText(value: Cell | undefined): string {
  if (typeof value !== "string" && typeof value !== "number") invalid()
  return String(value)
}

function decimal(value: unknown): string {
  const stored = text(value)
  if (!/^\d+$/.test(stored)) invalid()
  return stored
}

function numeric(value: Cell | undefined): number | null {
  const stored = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN
  return Number.isFinite(stored) ? stored : null
}

function invalid(message = "invalid relation response"): never {
  throw new Error(message)
}
