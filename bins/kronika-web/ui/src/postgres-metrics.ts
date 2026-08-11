import { registry } from "kronika:registry"

import type { Cell, DataRow } from "./api"

export const PG_STAT_STATEMENTS_TYPE_IDS = [
  "1002001", "1002002", "1002003", "1002004", "1002005", "1002006",
] as const

export const PG_STORE_PLANS_TYPE_IDS = ["1003001", "1004001", "1018001"] as const
export const PG_STORE_PLANS_INFO_TYPE_IDS = ["1016001"] as const

export type PostgresSemanticField =
  | "calls_per_second"
  | "execution_ms_per_second"
  | "mean_exec_ms_per_call"
  | "rows_per_second"
  | "planning_ms_per_second"
  | "shared_blk_read_ms_per_second"
  | "shared_blk_write_ms_per_second"
  | "local_blk_read_ms_per_second"
  | "local_blk_write_ms_per_second"
  | "temp_blk_read_ms_per_second"
  | "temp_blk_write_ms_per_second"

type PhysicalAliases = Readonly<Partial<Record<PostgresSemanticField, string>>>

interface LayoutKind {
  readonly section: "pg_stat_statements" | "pg_store_plans" | "pg_store_plans_info"
  readonly aliases: PhysicalAliases
  readonly curated: readonly string[]
}

const BLOCK_COUNTERS = [
  "shared_blks_hit", "shared_blks_read", "shared_blks_dirtied", "shared_blks_written",
  "local_blks_hit", "local_blks_read", "local_blks_dirtied", "local_blks_written",
  "temp_blks_read", "temp_blks_written",
] as const

const STATEMENT_FIELDS = [
  "queryid", "userid", "dbid", "toplevel", "datname", "usename", "query",
  "calls", "rows", "plans", "total_time", "total_exec_time", "total_plan_time",
  ...BLOCK_COUNTERS,
  "blk_read_time", "blk_write_time", "shared_blk_read_time", "shared_blk_write_time",
  "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time",
  "wal_records", "wal_fpi", "wal_bytes", "wal_buffers_full",
  "stats_since",
] as const

const PLAN_FIELDS = [
  "userid", "dbid", "queryid", "planid", "queryid_stat_statements", "datname", "usename",
  "plan", "relids", "cmd_type", "calls", "slow_log_calls", "total_time", "rows", ...BLOCK_COUNTERS,
  "blk_read_time", "blk_write_time", "shared_blk_read_time", "shared_blk_write_time",
  "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time",
  "total_plan_time",
] as const

const BASE_ALIASES = {
  calls_per_second: "calls",
  rows_per_second: "rows",
} as const

function statementAliases(
  execution: "total_time" | "total_exec_time",
  shared: "blk" | "shared",
  local: boolean,
  temp: boolean,
  planning: boolean,
): PhysicalAliases {
  return {
    ...BASE_ALIASES,
    execution_ms_per_second: execution,
    ...(planning ? { planning_ms_per_second: "total_plan_time" } : {}),
    shared_blk_read_ms_per_second: shared === "blk" ? "blk_read_time" : "shared_blk_read_time",
    shared_blk_write_ms_per_second: shared === "blk" ? "blk_write_time" : "shared_blk_write_time",
    ...(local ? {
      local_blk_read_ms_per_second: "local_blk_read_time",
      local_blk_write_ms_per_second: "local_blk_write_time",
    } : {}),
    ...(temp ? {
      temp_blk_read_ms_per_second: "temp_blk_read_time",
      temp_blk_write_ms_per_second: "temp_blk_write_time",
    } : {}),
  }
}

function planAliases(splitTimings: boolean, planning: boolean): PhysicalAliases {
  return {
    ...BASE_ALIASES,
    execution_ms_per_second: "total_time",
    ...(planning ? { planning_ms_per_second: "total_plan_time" } : {}),
    shared_blk_read_ms_per_second: splitTimings ? "shared_blk_read_time" : "blk_read_time",
    shared_blk_write_ms_per_second: splitTimings ? "shared_blk_write_time" : "blk_write_time",
    ...(splitTimings ? {
      local_blk_read_ms_per_second: "local_blk_read_time",
      local_blk_write_ms_per_second: "local_blk_write_time",
      temp_blk_read_ms_per_second: "temp_blk_read_time",
      temp_blk_write_ms_per_second: "temp_blk_write_time",
    } : {}),
  }
}

const LAYOUT_KINDS: Readonly<Record<string, LayoutKind>> = {
  "1002001": { section: "pg_stat_statements", aliases: statementAliases("total_time", "blk", false, false, false), curated: STATEMENT_FIELDS },
  "1002002": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "blk", false, false, true), curated: STATEMENT_FIELDS },
  "1002003": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "blk", false, false, true), curated: STATEMENT_FIELDS },
  "1002004": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "blk", false, true, true), curated: STATEMENT_FIELDS },
  "1002005": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "shared", true, true, true), curated: STATEMENT_FIELDS },
  "1002006": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "shared", true, true, true), curated: STATEMENT_FIELDS },
  "1003001": { section: "pg_store_plans", aliases: planAliases(true, false), curated: PLAN_FIELDS },
  "1004001": { section: "pg_store_plans", aliases: planAliases(false, true), curated: PLAN_FIELDS },
  "1018001": { section: "pg_store_plans", aliases: planAliases(true, false), curated: PLAN_FIELDS },
  "1016001": { section: "pg_store_plans_info", aliases: {}, curated: ["dealloc", "stats_reset"] },
}

const REGISTRY_BY_TYPE_ID = new Map(registry.map((layout) => [layout.typeId, layout]))

export interface PostgresLayout {
  readonly typeId: string
  readonly section: LayoutKind["section"]
  readonly identity: readonly string[]
  readonly fields: readonly string[]
  readonly aliases: PhysicalAliases
}

export function postgresLayout(typeId: string): PostgresLayout | null {
  const kind = LAYOUT_KINDS[typeId]
  const layout = REGISTRY_BY_TYPE_ID.get(typeId)
  if (kind === undefined || layout === undefined || layout.logicalName !== kind.section) return null
  const available = new Set(layout.columns.map((column) => column.name))
  return {
    typeId,
    section: kind.section,
    identity: layout.identity,
    fields: unique([...layout.identity, ...kind.curated]).filter((field) => available.has(field)),
    aliases: kind.aliases,
  }
}

export function postgresIdentity(typeId: string): readonly string[] {
  return postgresLayout(typeId)?.identity ?? []
}

export function postgresProjection(typeId: string): readonly string[] {
  return postgresLayout(typeId)?.fields ?? []
}

export function physicalField(typeId: string, semantic: PostgresSemanticField): string | null {
  if (semantic === "mean_exec_ms_per_call") return null
  const layout = postgresLayout(typeId)
  const physical = layout?.aliases[semantic]
  return physical !== undefined && layout?.fields.includes(physical) ? physical : null
}

export function physicalFields(
  typeIds: readonly string[],
  semantic: PostgresSemanticField,
): Readonly<Record<string, string>> {
  return Object.fromEntries(typeIds.flatMap((typeId) => {
    const physical = semantic === "mean_exec_ms_per_call"
      ? physicalField(typeId, "execution_ms_per_second")
      : physicalField(typeId, semantic)
    return physical === null ? [] : [[typeId, physical]]
  }))
}

export function findingSemanticField(typeId: string, physical: string): PostgresSemanticField | null {
  const execution = physicalField(typeId, "execution_ms_per_second")
  if (physical === execution) return "mean_exec_ms_per_call"
  const layout = postgresLayout(typeId)
  if (layout === null) return null
  for (const [semantic, candidate] of Object.entries(layout.aliases)) {
    if (candidate === physical) return semantic as PostgresSemanticField
  }
  return null
}

export function postgresOrder(
  typeId: string,
  semantic: PostgresSemanticField = "execution_ms_per_second",
): readonly string[] {
  const physical = semantic === "mean_exec_ms_per_call"
    ? physicalField(typeId, "execution_ms_per_second")
    : physicalField(typeId, semantic)
  return physical === null ? ["calls"] : [physical]
}

function fieldsByType(typeIds: readonly string[]): Readonly<Record<string, readonly string[]>> {
  return Object.fromEntries(typeIds.map((typeId) => [typeId, postgresProjection(typeId)]))
}

function orderCandidates(typeIds: readonly string[], semantic: PostgresSemanticField): readonly string[] {
  return unique(typeIds.flatMap((typeId) => {
    const physical = semantic === "mean_exec_ms_per_call"
      ? physicalField(typeId, "execution_ms_per_second")
      : physicalField(typeId, semantic)
    return physical === null ? [] : [physical]
  }))
}

function orderMap(typeIds: readonly string[]): Readonly<Record<PostgresSemanticField, readonly string[]>> {
  return Object.fromEntries([
    "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second",
    "planning_ms_per_second", "shared_blk_read_ms_per_second", "shared_blk_write_ms_per_second",
    "local_blk_read_ms_per_second", "local_blk_write_ms_per_second",
    "temp_blk_read_ms_per_second", "temp_blk_write_ms_per_second",
  ].map((semantic) => [semantic, orderCandidates(typeIds, semantic as PostgresSemanticField)])) as unknown as Readonly<Record<PostgresSemanticField, readonly string[]>>
}

export interface PostgresSectionRequest {
  readonly section: LayoutKind["section"]
  readonly typeIds: readonly string[]
  readonly fieldsByType: Readonly<Record<string, readonly string[]>>
  readonly top?: number
  readonly defaultOrder?: readonly string[]
  readonly order?: Readonly<Record<string, readonly string[]>>
  readonly fallbackOrder?: readonly string[]
}

function denseRequest(
  section: "pg_stat_statements" | "pg_store_plans",
  typeIds: readonly string[],
): PostgresSectionRequest {
  const order = orderMap(typeIds)
  return {
    section,
    typeIds,
    fieldsByType: fieldsByType(typeIds),
    top: 200,
    defaultOrder: order.execution_ms_per_second,
    order,
    fallbackOrder: ["calls"],
  }
}

export const POSTGRES_SECTION_REQUESTS: readonly PostgresSectionRequest[] = [
  denseRequest("pg_stat_statements", PG_STAT_STATEMENTS_TYPE_IDS),
  denseRequest("pg_store_plans", PG_STORE_PLANS_TYPE_IDS),
  {
    section: "pg_store_plans_info",
    typeIds: PG_STORE_PLANS_INFO_TYPE_IDS,
    fieldsByType: fieldsByType(PG_STORE_PLANS_INFO_TYPE_IDS),
  },
] as const

export interface PostgresIntervalValues extends Readonly<Record<PostgresSemanticField, number | null>> {
  readonly calls_per_second: number | null
  readonly execution_ms_per_second: number | null
  readonly mean_exec_ms_per_call: number | null
  readonly rows_per_second: number | null
  readonly planning_ms_per_second: number | null
  readonly shared_blk_read_ms_per_second: number | null
  readonly shared_blk_write_ms_per_second: number | null
  readonly local_blk_read_ms_per_second: number | null
  readonly local_blk_write_ms_per_second: number | null
  readonly temp_blk_read_ms_per_second: number | null
  readonly temp_blk_write_ms_per_second: number | null
}

const SEMANTIC_FIELDS = Object.keys(orderMap([])) as readonly PostgresSemanticField[]

export function decoratePostgresIntervalRow(row: DataRow): DataRow {
  const aliases = postgresLayout(row.typeId)?.aliases ?? {}
  const values: Record<string, Cell> = { ...row.values }
  for (const semantic of SEMANTIC_FIELDS) {
    if (semantic === "mean_exec_ms_per_call") continue
    const physical = aliases[semantic]
    values[semantic] = physical === undefined ? null : finiteRate(row.values[physical])
  }
  const calls = finiteRate(values.calls_per_second)
  const execution = finiteRate(values.execution_ms_per_second)
  values.mean_exec_ms_per_call = calls !== null && calls > 0 && execution !== null && execution >= 0
    ? execution / calls
    : null
  return { ...row, values }
}

export function exactCounterDelta(earlier: Cell, later: Cell): bigint | null {
  const before = exactInteger(earlier)
  const after = exactInteger(later)
  if (before === null || after === null || after < before) return null
  return after - before
}

export function intervalMetric(earlier: DataRow, later: DataRow, physical: string): number | null {
  const elapsed = later.timestamp - earlier.timestamp
  if (elapsed <= 0) return null
  const delta = nonnegativeDelta(earlier.values[physical] ?? null, later.values[physical] ?? null)
  if (delta === null) return null
  const rate = numeric(delta) / (elapsed / 1_000_000)
  return Number.isFinite(rate) ? rate : null
}

export function postgresInterval(earlier: DataRow, later: DataRow): PostgresIntervalValues {
  const layout = earlier.typeId === later.typeId ? postgresLayout(later.typeId) : null
  const rate = (semantic: PostgresSemanticField): number | null => {
    const physical = semantic === "mean_exec_ms_per_call" ? undefined : layout?.aliases[semantic]
    return physical === undefined ? null : intervalMetric(earlier, later, physical)
  }
  const callsPhysical = layout?.aliases.calls_per_second
  const executionPhysical = layout?.aliases.execution_ms_per_second
  const elapsed = later.timestamp - earlier.timestamp
  const calls = callsPhysical === undefined ? null : exactCounterDelta(
    earlier.values[callsPhysical] ?? null,
    later.values[callsPhysical] ?? null,
  )
  const execution = executionPhysical === undefined ? null : nonnegativeDelta(
    earlier.values[executionPhysical] ?? null,
    later.values[executionPhysical] ?? null,
  )
  const mean = elapsed > 0 && calls !== null && calls > 0n && execution !== null
    ? numeric(execution) / Number(calls)
    : null
  return {
    calls_per_second: rate("calls_per_second"),
    execution_ms_per_second: rate("execution_ms_per_second"),
    mean_exec_ms_per_call: mean !== null && Number.isFinite(mean) ? mean : null,
    rows_per_second: rate("rows_per_second"),
    planning_ms_per_second: rate("planning_ms_per_second"),
    shared_blk_read_ms_per_second: rate("shared_blk_read_ms_per_second"),
    shared_blk_write_ms_per_second: rate("shared_blk_write_ms_per_second"),
    local_blk_read_ms_per_second: rate("local_blk_read_ms_per_second"),
    local_blk_write_ms_per_second: rate("local_blk_write_ms_per_second"),
    temp_blk_read_ms_per_second: rate("temp_blk_read_ms_per_second"),
    temp_blk_write_ms_per_second: rate("temp_blk_write_ms_per_second"),
  }
}

export interface PostgresHistoryPoint extends PostgresIntervalValues {
  readonly timestamp: number
  readonly segmentId: string
}

export function postgresHistory(rows: readonly DataRow[]): readonly PostgresHistoryPoint[] {
  const ordered = [...rows].sort((left, right) => left.timestamp - right.timestamp)
  return ordered.map((row, index) => ({
    timestamp: row.timestamp,
    segmentId: row.segmentId,
    ...(index === 0 ? emptyInterval() : postgresInterval(ordered[index - 1] as DataRow, row)),
  }))
}

function emptyInterval(): PostgresIntervalValues {
  return Object.fromEntries(SEMANTIC_FIELDS.map((field) => [field, null])) as unknown as PostgresIntervalValues
}

function finiteRate(cell: Cell | undefined): number | null {
  return typeof cell === "number" && Number.isFinite(cell) ? cell : null
}

function exactInteger(cell: Cell): bigint | null {
  if (typeof cell === "string" && /^-?\d+$/.test(cell)) {
    try {
      return BigInt(cell)
    } catch {
      return null
    }
  }
  return typeof cell === "number" && Number.isSafeInteger(cell) ? BigInt(cell) : null
}

function nonnegativeDelta(earlier: Cell, later: Cell): bigint | number | null {
  const exactBefore = exactInteger(earlier)
  const exactAfter = exactInteger(later)
  if (exactBefore !== null && exactAfter !== null) {
    return exactAfter < exactBefore ? null : exactAfter - exactBefore
  }
  if (typeof earlier !== "number" || typeof later !== "number") return null
  const delta = later - earlier
  return delta >= 0 && Number.isFinite(delta) ? delta : null
}

function numeric(value: bigint | number): number {
  return typeof value === "bigint" ? Number(value) : value
}

function unique<T>(values: readonly T[]): T[] {
  return [...new Set(values)]
}
