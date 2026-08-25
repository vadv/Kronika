import type { TableOrder } from "./entity-table"
import { parseSearch, type SearchClause, type SearchExpr } from "./search"

export const POSTGRESQL_ACTIVITY_PAGE_SIZE = 200

export type PostgresqlActivityDirection = "asc" | "desc"

export type PostgresqlActivitySort =
  | "pid"
  | "database"
  | "role"
  | "query_preview"
  | "query_duration_ms"
  | "transaction_duration_ms"
  | "application"
  | "client"
  | "state"
  | "wait_type"
  | "wait_event"
  | "backend_type"

type ActivityTextField =
  | "text"
  | "database"
  | "role"
  | "application"
  | "client"
  | "backend_type"
  | "state"
  | "wait_type"
  | "wait_event"

export interface PostgresqlActivityTextMatch {
  readonly all_of: readonly string[]
}

export interface PostgresqlActivityClause {
  readonly text?: PostgresqlActivityTextMatch | undefined
  readonly pid?: { readonly any_of: readonly number[] } | undefined
  readonly query_id?: { readonly any_of: readonly string[] } | undefined
  readonly database?: PostgresqlActivityTextMatch | undefined
  readonly role?: PostgresqlActivityTextMatch | undefined
  readonly application?: PostgresqlActivityTextMatch | undefined
  readonly client?: PostgresqlActivityTextMatch | undefined
  readonly backend_type?: PostgresqlActivityTextMatch | undefined
  readonly state?: PostgresqlActivityTextMatch | undefined
  readonly wait_type?: PostgresqlActivityTextMatch | undefined
  readonly wait_event?: PostgresqlActivityTextMatch | undefined
}

export interface PostgresqlActivityRequest {
  readonly direction: PostgresqlActivityDirection
  readonly filter?: readonly PostgresqlActivityClause[] | undefined
  readonly pageSize: number
  readonly path: string
  readonly sort: PostgresqlActivitySort
}

const SORT_BY_COLUMN: Readonly<Record<string, PostgresqlActivitySort>> = {
  pid: "pid",
  datname: "database",
  usename: "role",
  query: "query_preview",
  query_preview: "query_preview",
  query_duration_ms: "query_duration_ms",
  transaction_duration_ms: "transaction_duration_ms",
  application_name: "application",
  client_addr: "client",
  state: "state",
  wait_event_type: "wait_type",
  wait_event: "wait_event",
  backend_type: "backend_type",
}

const TEXT_FIELD_BY_SEARCH_KEY: Readonly<Record<string, ActivityTextField>> = {
  text: "text",
  database: "database",
  role: "role",
  application: "application",
  client: "client",
  backend_type: "backend_type",
  state: "state",
  wait_type: "wait_type",
  wait_event: "wait_event",
}

export function postgresqlActivityRequest(
  at: number,
  search: string,
  order?: TableOrder | undefined,
  cursor?: string | undefined,
): PostgresqlActivityRequest {
  if (!Number.isSafeInteger(at)) throw new Error("Activity at must be a safe integer")
  if (cursor !== undefined && (cursor.length === 0 || cursor.length > 4_096)) {
    throw new Error("Activity cursor is invalid")
  }
  const requestedSort = order === undefined ? undefined : SORT_BY_COLUMN[order.column]
  const sort = requestedSort ?? "query_duration_ms"
  const direction = requestedSort !== undefined && order?.descending === false ? "asc" : "desc"
  const filter = postgresqlActivityFilter(search)
  const values: readonly (readonly [string, string])[] = [
    ["at", String(at)],
    ...(filter === undefined ? [] : [["filter", JSON.stringify(filter)] as const]),
    ["sort", sort],
    ["direction", direction],
    ["page_size", String(POSTGRESQL_ACTIVITY_PAGE_SIZE)],
    ...(cursor === undefined ? [] : [["cursor", cursor] as const]),
  ]
  const query = values.map(([name, value]) => `${encodeURIComponent(name)}=${encodeURIComponent(value)}`).join("&")
  return {
    direction,
    ...(filter === undefined ? {} : { filter }),
    pageSize: POSTGRESQL_ACTIVITY_PAGE_SIZE,
    path: `/api/postgresql/activity?${query}`,
    sort,
  }
}

export function postgresqlActivityFilter(search: string): readonly PostgresqlActivityClause[] | undefined {
  const parsed = parseSearch(search, "pg_stat_activity")
  if (!parsed.ok) throw new Error("Activity search is invalid")
  if (parsed.query.canonical === "") return undefined
  if (!parsed.query.structured) {
    return [{ text: { all_of: [parsed.query.freeText ?? ""] } }]
  }
  if (parsed.query.expr === null) return undefined
  return expressionTerms(parsed.query.expr)
    .map(activityClause)
    .filter((clause): clause is PostgresqlActivityClause => clause !== null)
}

function expressionTerms(expr: SearchExpr): readonly (readonly SearchClause[])[] {
  if (expr.kind === "predicate") return [[expr.predicate]]
  const left = expressionTerms(expr.left)
  const right = expressionTerms(expr.right)
  if (expr.kind === "or") return [...left, ...right]
  return left.flatMap((leftTerm) => right.map((rightTerm) => [...leftTerm, ...rightTerm]))
}

function activityClause(predicates: readonly SearchClause[]): PostgresqlActivityClause | null {
  const patterns = new Map<ActivityTextField, string[]>()
  let pid: number | undefined
  let queryId: string | undefined
  for (const predicate of predicates) {
    if (predicate.key === "pid") {
      const parsed = Number(predicate.value)
      if (!Number.isInteger(parsed) || parsed < 1 || parsed > 2_147_483_647) return null
      if (pid !== undefined && pid !== parsed) return null
      pid = parsed
      continue
    }
    if (predicate.key === "query_id") {
      if (queryId !== undefined && queryId !== predicate.value) return null
      queryId = predicate.value
      continue
    }
    const field = TEXT_FIELD_BY_SEARCH_KEY[predicate.key]
    if (field === undefined) return null
    const values = patterns.get(field) ?? []
    if (!values.includes(predicate.value)) values.push(predicate.value)
    patterns.set(field, values)
  }
  const clause: Record<string, PostgresqlActivityTextMatch | { readonly any_of: readonly number[] } | { readonly any_of: readonly string[] }> = {}
  for (const [field, allOf] of patterns) clause[field] = { all_of: allOf }
  if (pid !== undefined) clause.pid = { any_of: [pid] }
  if (queryId !== undefined) clause.query_id = { any_of: [queryId] }
  return clause as PostgresqlActivityClause
}
