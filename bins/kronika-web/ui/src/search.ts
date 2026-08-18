import type { DataRow } from "./api"
import { globMatcher } from "./glob"
import { rawText, value } from "./model"

export const SEARCH_MAX_EXPRESSION = 1_024
export const SEARCH_MAX_CLAUSES = 8
export const SEARCH_MAX_VALUE = 256

export type SearchSurface =
  | "events"
  | "os_process"
  | "pg_locks"
  | "pg_stat_activity"
  | "pg_stat_database"
  | "pg_stat_statements"
  | "pg_stat_user_indexes"
  | "pg_stat_user_tables"
  | "pg_store_plans"

export type SearchFieldKind = "identifier" | "string"

export interface SearchField {
  readonly aliases: readonly string[]
  readonly columns: readonly string[]
  readonly help: string
  readonly key: string
  readonly kind: SearchFieldKind
  readonly signed?: boolean | undefined
}

export interface SearchClause {
  readonly end: number
  readonly field: SearchField
  readonly key: string
  readonly start: number
  readonly value: string
}

export interface SearchQuery {
  readonly canonical: string
  readonly clauses: readonly SearchClause[]
  readonly freeText: string | null
  readonly structured: boolean
}

export type SearchErrorCode =
  | "empty_clause"
  | "expression_too_long"
  | "expected_and"
  | "expected_colon"
  | "invalid_escape"
  | "invalid_identifier"
  | "missing_value"
  | "too_many_clauses"
  | "unknown_field"
  | "unterminated_quote"
  | "value_too_long"

export interface SearchError {
  readonly code: SearchErrorCode
  readonly end: number
  readonly start: number
  readonly token?: string | undefined
}

export type SearchParseResult =
  | { readonly ok: true; readonly query: SearchQuery }
  | { readonly error: SearchError; readonly ok: false }

type SearchFailure = Extract<SearchParseResult, { readonly ok: false }>

type ValueParseResult =
  | { readonly end: number; readonly ok: true; readonly start: number; readonly value: string }
  | { readonly error: SearchError; readonly ok: false }

const text = (columns: readonly string[]): SearchField => ({
  aliases: ["q"], columns, help: "filter.field.text.help", key: "text", kind: "string",
})
const string = (key: string, columns: readonly string[], aliases: readonly string[] = []): SearchField => ({
  aliases, columns, help: `filter.field.${key}.help`, key, kind: "string",
})
const id = (key: string, columns: readonly string[], aliases: readonly string[] = [], signed = false): SearchField => ({
  aliases, columns, help: `filter.field.${key}.help`, key, kind: "identifier", signed,
})

const SEARCH_FIELDS: Readonly<Record<SearchSurface, readonly SearchField[]>> = {
  events: [
    text(["category", "source", "logical_name"]),
    string("kind", ["kind"]), string("source", ["source", "logical_name"]),
    string("category", ["category"]),
  ],
  os_process: [
    text(["cmdline", "comm", "user", "effective_user"]),
    string("user", ["user"], ["username"]),
    string("effective_user", ["effective_user"], ["euser"]),
    id("user_id", ["uid"], ["uid"]), id("effective_user_id", ["euid"], ["euid"]),
    id("pid", ["pid"]),
    id("parent_pid", ["ppid"]),
    string("command", ["cmdline", "comm"], ["cmd"]),
    string("state", ["state"]),
  ],
  pg_stat_activity: [
    text(["query", "datname", "usename", "application_name", "client_addr", "state", "wait_event_type", "wait_event"]),
    id("query_id", ["query_id"], [], true), id("pid", ["pid"]),
    string("database", ["datname"], ["db"]), string("role", ["usename"], ["user"]),
    string("application", ["application_name"], ["app"]), string("client", ["client_addr"]),
    string("backend_type", ["backend_type"], ["backend"]), string("state", ["state"]),
    string("wait_type", ["wait_event_type"]), string("wait_event", ["wait_event"]),
  ],
  pg_stat_statements: [
    text(["query", "datname", "usename"]), id("query_id", ["queryid"], [], true),
    string("database", ["datname"], ["db"]), string("role", ["usename"], ["user"]),
  ],
  pg_store_plans: [
    text(["plan", "datname", "usename"]), id("query_id", ["queryid", "queryid_stat_statements"], [], true),
    id("plan_id", ["planid"], [], true), string("database", ["datname"], ["db"]),
    string("role", ["usename"], ["user"]),
  ],
  pg_stat_user_tables: [
    text(["datname", "schemaname", "relname", "tablespace"]),
    string("database", ["datname"], ["db"]), string("schema", ["schemaname"]),
    string("table_name", ["relname"], ["table"]), string("tablespace", ["tablespace"]),
  ],
  pg_stat_user_indexes: [
    text(["datname", "schemaname", "relname", "indexrelname", "tablespace", "amname", "indexdef"]),
    string("database", ["datname"], ["db"]), string("schema", ["schemaname"]),
    string("table_name", ["relname"], ["table"]),
    string("index_name", ["indexrelname"], ["index"]),
    string("access_method", ["amname"], ["method"]), string("definition", ["indexdef"]),
    string("tablespace", ["tablespace"]),
  ],
  pg_locks: [
    text(["query", "datname", "usename", "application_name", "state", "wait_event_type", "wait_event", "lock_target", "lock_relname", "lock_mode"]),
    id("pid", ["pid"]), string("database", ["datname"], ["db"]),
    string("role", ["usename"], ["user"]), string("state", ["state"]),
    string("wait_type", ["wait_event_type"]), string("wait_event", ["wait_event"]),
    string("lock", ["lock_target", "lock_relname", "lock_mode"]),
  ],
  pg_stat_database: [text(["datname"]), string("database", ["datname"], ["db"])],
}

export function searchFields(surface: SearchSurface): readonly SearchField[] {
  return SEARCH_FIELDS[surface]
}

export function parseSearch(input: string, surface: SearchSurface): SearchParseResult {
  if (input.length > SEARCH_MAX_EXPRESSION) return failure("expression_too_long", 0, input.length)
  const first = firstNonSpace(input, 0)
  if (first === input.length) return success("", [], null, false)
  if (!input.includes(":")) {
    const and = standaloneAnd(input, first)
    if (and !== null) return failure("expected_colon", and, and + 3, input.slice(and, and + 3))
    const freeText = input.trim()
    if ([...freeText].length > SEARCH_MAX_VALUE) return failure("value_too_long", first, input.length)
    return success(freeText, [], freeText, false)
  }

  const fields = searchFields(surface)
  const byName = new Map(fields.flatMap((field) => [field.key, ...field.aliases].map((name) => [name, field] as const)))
  const clauses: SearchClause[] = []
  let cursor = first
  while (cursor < input.length) {
    if (clauses.length >= SEARCH_MAX_CLAUSES) return failure("too_many_clauses", cursor, input.length)
    const start = cursor
    const keyStart = cursor
    while (cursor < input.length && /[A-Za-z0-9_]/.test(input[cursor]!)) cursor += 1
    if (cursor === keyStart) return failure("empty_clause", cursor, Math.min(input.length, cursor + 1))
    const rawKey = input.slice(keyStart, cursor).toLowerCase()
    const field = byName.get(rawKey)
    if (field === undefined) return failure("unknown_field", keyStart, cursor, rawKey)
    cursor = firstNonSpace(input, cursor)
    if (input[cursor] !== ":") return failure("expected_colon", cursor, Math.min(input.length, cursor + 1), rawKey)
    cursor = firstNonSpace(input, cursor + 1)
    if (cursor >= input.length) return failure("missing_value", cursor, cursor, field.key)
    const parsed = input[cursor] === '"' ? quotedValue(input, cursor) : bareValue(input, cursor)
    if (!parsed.ok) return parsed
    cursor = parsed.end
    if (parsed.value === "") return failure("missing_value", parsed.start, parsed.end, field.key)
    if ([...parsed.value].length > SEARCH_MAX_VALUE) return failure("value_too_long", parsed.start, parsed.end)
    if (field.kind === "identifier" && !validIdentifier(parsed.value, field.signed === true)) {
      return failure("invalid_identifier", parsed.start, parsed.end, field.key)
    }
    const end = cursor
    clauses.push({ end, field, key: field.key, start, value: parsed.value })
    cursor = firstNonSpace(input, cursor)
    if (cursor >= input.length) break
    if (input.slice(cursor, cursor + 3).toLowerCase() !== "and"
      || (cursor > 0 && !/\s/.test(input[cursor - 1]!))
      || (cursor + 3 < input.length && !/\s/.test(input[cursor + 3]!))) {
      const tokenEnd = nextSpace(input, cursor)
      return failure("expected_and", cursor, tokenEnd, input.slice(cursor, tokenEnd))
    }
    cursor = firstNonSpace(input, cursor + 3)
    if (cursor >= input.length) return failure("empty_clause", input.length - 3, input.length, "AND")
  }
  const canonical = clauses.map((clause) => `${clause.key}:${canonicalValue(clause.value, clause.field.kind)}`).join(" AND ")
  return success(canonical, clauses, null, true)
}

export function withoutSearchClause(query: SearchQuery, index: number): string {
  if (!query.structured) return ""
  return query.clauses.filter((_clause, at) => at !== index)
    .map((clause) => `${clause.key}:${canonicalValue(clause.value, clause.field.kind)}`).join(" AND ")
}

export function rowMatchesSearch(row: DataRow, query: SearchQuery, surface: SearchSurface): boolean {
  if (query.canonical === "") return true
  const clauses = query.structured
    ? query.clauses
    : [{ field: searchFields(surface)[0]!, value: query.freeText ?? "" }]
  return clauses.every((clause) => clause.field.columns.some((column) => {
    if (surface === "pg_store_plans" && clause.field.key === "query_id") {
      const wanted = row.typeId === "1004001" ? "queryid_stat_statements" : "queryid"
      if (column !== wanted) return false
    }
    const stored = rawText(value(row, column))
    if (stored === null) return false
    if (surface === "pg_store_plans" && row.typeId === "1004001" && clause.field.key === "query_id" && stored === "0") return false
    if (clause.field.kind === "identifier") return stored === clause.value
    return globMatcher(clause.value)?.(stored) ?? true
  }))
}

export function canonicalSearch(clauses: readonly { readonly key: string; readonly value: string }[], surface: SearchSurface): string | null {
  const byName = new Map(searchFields(surface).map((field) => [field.key, field] as const))
  const expression = clauses.map(({ key, value }) => `${key}:${canonicalValue(value, byName.get(key)?.kind ?? "string")}`).join(" AND ")
  const parsed = parseSearch(expression, surface)
  return parsed.ok ? parsed.query.canonical : null
}

function quotedValue(input: string, quote: number): ValueParseResult {
  let cursor = quote + 1
  let value = ""
  while (cursor < input.length) {
    const character = input[cursor]!
    if (character === '"') return { end: cursor + 1, ok: true, start: quote, value }
    if (character !== "\\") {
      value += character
      cursor += 1
      continue
    }
    const escaped = input[cursor + 1]
    if (escaped !== '"' && escaped !== "\\") return failure("invalid_escape", cursor, Math.min(input.length, cursor + 2))
    value += escaped
    cursor += 2
  }
  return failure("unterminated_quote", quote, input.length)
}

function bareValue(input: string, start: number): { readonly end: number; readonly ok: true; readonly start: number; readonly value: string } {
  let end = start
  while (end < input.length && !/\s/.test(input[end]!)) end += 1
  return { end, ok: true, start, value: input.slice(start, end) }
}

function canonicalValue(value: string, kind: SearchFieldKind): string {
  if (kind === "identifier" || /^[^\s:"\\]+$/.test(value)) return value
  return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`
}

function validIdentifier(value: string, signed: boolean): boolean {
  if (!(signed ? /^-?(0|[1-9]\d*)$/.test(value) : /^(0|[1-9]\d*)$/.test(value)) || value === "-0") return false
  try {
    const parsed = BigInt(value)
    return signed
      ? parsed >= -9_223_372_036_854_775_808n && parsed <= 9_223_372_036_854_775_807n
      : parsed >= 0n && parsed <= 18_446_744_073_709_551_615n
  } catch {
    return false
  }
}

function success(canonical: string, clauses: readonly SearchClause[], freeText: string | null, structured: boolean): SearchParseResult {
  return { ok: true, query: { canonical, clauses, freeText, structured } }
}

function failure(code: SearchErrorCode, start: number, end: number, token?: string): SearchFailure {
  return { error: { code, end: Math.max(start, end), start, ...(token === undefined ? {} : { token }) }, ok: false }
}

function firstNonSpace(input: string, start: number): number {
  let cursor = start
  while (cursor < input.length && /\s/.test(input[cursor]!)) cursor += 1
  return cursor
}

function nextSpace(input: string, start: number): number {
  let cursor = start
  while (cursor < input.length && !/\s/.test(input[cursor]!)) cursor += 1
  return cursor
}

function standaloneAnd(input: string, start: number): number | null {
  const match = /(?:^|\s)AND(?:\s|$)/i.exec(input.slice(start))
  if (match === null) return null
  return start + match.index + (match[0].startsWith(" ") ? 1 : 0)
}
