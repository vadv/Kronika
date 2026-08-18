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

export type QuantityKind = "bytes" | "count" | "count_rate" | "duration" | "percentage"
export type SearchFieldKind = "identifier" | "quantity" | "string"
export type SearchOperator = ":" | ">" | "<"

export interface SearchField {
  readonly aliases: readonly string[]
  readonly columns: readonly string[]
  readonly help: string
  readonly key: string
  readonly kind: SearchFieldKind
  readonly quantity?: QuantityKind | undefined
  readonly signed?: boolean | undefined
}

export interface SearchQuantity {
  readonly denominator: bigint
  readonly number: string
  readonly numerator: bigint
  readonly unit: string
}

export interface SearchClause {
  readonly canonical: string
  readonly end: number
  readonly field: SearchField
  readonly key: string
  readonly operator: SearchOperator
  readonly quantity?: SearchQuantity | undefined
  readonly start: number
  readonly value: string
}

export type SearchExpr =
  | { readonly kind: "predicate"; readonly predicate: SearchClause }
  | { readonly kind: "and"; readonly terms: readonly SearchExpr[] }

export interface SearchQuery {
  readonly canonical: string
  readonly clauses: readonly SearchClause[]
  readonly expr: SearchExpr | null
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
  | "invalid_number"
  | "invalid_unit"
  | "malformed_operator"
  | "missing_value"
  | "negative_not_allowed"
  | "non_integral_base_value"
  | "operator_not_allowed"
  | "out_of_range"
  | "quoted_quantity"
  | "too_many_clauses"
  | "unit_required"
  | "unknown_field"
  | "unsupported_operator"
  | "unsupported_syntax"
  | "unterminated_quote"
  | "value_too_long"
  | "whitespace_before_unit"

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
  | SearchFailure
type QuantityParseResult =
  | { readonly canonical: string; readonly ok: true; readonly quantity: SearchQuantity }
  | SearchFailure

const MAX_U128 = (1n << 128n) - 1n
const BYTE_UNITS = new Map<string, bigint>([
  ["B", 1n], ["kB", 1_000n], ["MB", 1_000_000n], ["GB", 1_000_000_000n],
  ["TB", 1_000_000_000_000n], ["PB", 1_000_000_000_000_000n], ["EB", 1_000_000_000_000_000_000n],
  ["KiB", 1_024n], ["MiB", 1_048_576n], ["GiB", 1_073_741_824n], ["TiB", 1_099_511_627_776n],
  ["PiB", 1_125_899_906_842_624n], ["EiB", 1_152_921_504_606_846_976n],
])
const DURATION_UNITS = new Map<string, readonly [bigint, bigint]>([
  ["ns", [1n, 1_000_000n]], ["us", [1n, 1_000n]], ["ms", [1n, 1n]],
  ["s", [1_000n, 1n]], ["min", [60_000n, 1n]], ["h", [3_600_000n, 1n]],
])

const text = (columns: readonly string[]): SearchField => ({ aliases: ["q"], columns, help: "filter.field.text.help", key: "text", kind: "string" })
const string = (key: string, columns: readonly string[], aliases: readonly string[] = []): SearchField => ({ aliases, columns, help: `filter.field.${key}.help`, key, kind: "string" })
const id = (key: string, columns: readonly string[], aliases: readonly string[] = [], signed = false): SearchField => ({ aliases, columns, help: `filter.field.${key}.help`, key, kind: "identifier", signed })
const quantity = (key: string, kind: QuantityKind): SearchField => ({ aliases: [], columns: [], help: `filter.field.${key}.help`, key, kind: "quantity", quantity: kind })

const SEARCH_FIELDS: Readonly<Record<SearchSurface, readonly SearchField[]>> = {
  events: [text(["category", "source", "logical_name"]), string("kind", ["kind"]), string("source", ["source", "logical_name"]), string("category", ["category"])],
  os_process: [text(["cmdline", "comm"]), id("pid", ["pid"]), id("parent_pid", ["ppid"]), string("command", ["cmdline", "comm"], ["cmd"]), id("user_id", ["uid"]), id("effective_user_id", ["euid"], ["euid"]), string("state", ["state"])],
  pg_stat_activity: [
    text(["query", "datname", "usename", "application_name", "client_addr", "state", "wait_event_type", "wait_event"]),
    id("query_id", ["query_id"], [], true), id("pid", ["pid"]), string("database", ["datname"], ["db"]), string("role", ["usename"], ["user"]),
    string("application", ["application_name"], ["app"]), string("client", ["client_addr"]), string("backend_type", ["backend_type"], ["backend"]),
    string("state", ["state"]), string("wait_type", ["wait_event_type"]), string("wait_event", ["wait_event"]),
  ],
  pg_stat_statements: [text(["query", "datname", "usename"]), id("query_id", ["queryid"], [], true), string("database", ["datname"], ["db"]), string("role", ["usename"], ["user"])],
  pg_store_plans: [text(["plan", "datname", "usename"]), id("query_id", ["queryid", "queryid_stat_statements"], [], true), id("plan_id", ["planid"], [], true), string("database", ["datname"], ["db"]), string("role", ["usename"], ["user"])],
  pg_stat_user_tables: [
    text(["datname", "schemaname", "relname", "tablespace"]), string("database", ["datname"], ["db"]), string("schema", ["schemaname"]),
    string("table_name", ["relname"], ["table"]), string("tablespace", ["tablespace"]), quantity("size", "bytes"), quantity("table_count", "count"),
    quantity("buffer_hit", "percentage"), quantity("seq_scan_rate", "count_rate"), quantity("change_rate", "count_rate"),
    quantity("autovacuum_rate", "count_rate"), quantity("autovacuum_mean", "duration"), quantity("xid_age", "count"),
  ],
  pg_stat_user_indexes: [
    text(["datname", "schemaname", "relname", "indexrelname", "tablespace", "amname", "indexdef"]),
    string("database", ["datname"], ["db"]), string("schema", ["schemaname"]), string("table_name", ["relname"], ["table"]),
    string("index_name", ["indexrelname"], ["index"]), string("access_method", ["amname"], ["method"]), string("definition", ["indexdef"]),
    string("tablespace", ["tablespace"]), quantity("size", "bytes"), quantity("index_count", "count"), quantity("buffer_hit", "percentage"), quantity("scan_rate", "count_rate"),
  ],
  pg_locks: [
    text(["query", "datname", "usename", "application_name", "state", "wait_event_type", "wait_event", "lock_target", "lock_relname", "lock_mode"]),
    id("pid", ["pid"]), string("database", ["datname"], ["db"]), string("role", ["usename"], ["user"]), string("state", ["state"]),
    string("wait_type", ["wait_event_type"]), string("wait_event", ["wait_event"]), string("lock", ["lock_target", "lock_relname", "lock_mode"]),
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
  if (!hasStructuredSyntax(input)) {
    const and = standaloneAnd(input, first)
    if (and !== null) return failure("expected_colon", and, and + 3, input.slice(and, and + 3))
    const freeText = input.trim()
    if ([...freeText].length > SEARCH_MAX_VALUE) return failure("value_too_long", first, input.length)
    return success(freeText, [], freeText, false)
  }

  const byName = new Map(searchFields(surface).flatMap((field) => [field.key, ...field.aliases].map((name) => [name, field] as const)))
  const clauses: SearchClause[] = []
  let cursor = first
  while (cursor < input.length) {
    if (clauses.length >= SEARCH_MAX_CLAUSES) return failure("too_many_clauses", cursor, input.length)
    const reserved = reservedAt(input, cursor)
    if (reserved !== null) return failure("unsupported_syntax", reserved.start, reserved.end, reserved.token)
    const start = cursor
    const keyStart = cursor
    while (cursor < input.length && /[A-Za-z0-9_]/.test(input[cursor]!)) cursor += 1
    if (cursor === keyStart) return failure("empty_clause", cursor, Math.min(input.length, cursor + 1))
    const rawKey = input.slice(keyStart, cursor).toLowerCase()
    const field = byName.get(rawKey)
    if (field === undefined) return failure("unknown_field", keyStart, cursor, rawKey)
    cursor = firstNonSpace(input, cursor)
    const operatorStart = cursor
    while (cursor < input.length && /[!<>=:]/.test(input[cursor]!)) cursor += 1
    const rawOperator = input.slice(operatorStart, cursor)
    if (rawOperator === "") return failure("expected_colon", operatorStart, Math.min(input.length, operatorStart + 1), rawKey)
    if ([">=", "<=", "==", "!=", "="].includes(rawOperator)) return failure("unsupported_operator", operatorStart, cursor, rawOperator)
    if (![":", ">", "<"].includes(rawOperator)) return failure("malformed_operator", operatorStart, cursor, rawOperator)
    const operator = rawOperator as SearchOperator
    if ((field.kind === "quantity") !== (operator !== ":")) return failure("operator_not_allowed", operatorStart, cursor, rawOperator)
    cursor = firstNonSpace(input, cursor)
    if (cursor >= input.length) return failure("missing_value", cursor, cursor, field.key)
    if (field.kind === "quantity" && input[cursor] === '"') {
      const quoted = quotedValue(input, cursor)
      return quoted.ok ? failure("quoted_quantity", quoted.start, quoted.end, field.key) : quoted
    }
    const parsed = input[cursor] === '"' ? quotedValue(input, cursor) : bareValue(input, cursor)
    if (!parsed.ok) return parsed
    cursor = parsed.end
    if (parsed.value === "") return failure("missing_value", parsed.start, parsed.end, field.key)
    if ([...parsed.value].length > SEARCH_MAX_VALUE) return failure("value_too_long", parsed.start, parsed.end)

    let clause: SearchClause
    if (field.kind === "quantity") {
      const parenthesis = parsed.value.search(/[()]/)
      if (parenthesis >= 0) {
        return failure("unsupported_syntax", parsed.start + parenthesis, parsed.start + parenthesis + 1, parsed.value[parenthesis])
      }
      const next = firstNonSpace(input, cursor)
      if (next > cursor) {
        const unitEnd = nextSpace(input, next)
        if (looksLikeUnit(input.slice(next, unitEnd))) return failure("whitespace_before_unit", next, unitEnd, input.slice(next, unitEnd))
      }
      const parsedQuantity = parseQuantity(parsed.value, field.quantity!, parsed.start)
      if (!parsedQuantity.ok) return parsedQuantity
      const canonical = `${field.key}${operator}${parsedQuantity.canonical}`
      clause = { canonical, end: cursor, field, key: field.key, operator, quantity: parsedQuantity.quantity, start, value: parsedQuantity.canonical }
    } else {
      if (field.kind === "identifier" && !validIdentifier(parsed.value, field.signed === true)) return failure("invalid_identifier", parsed.start, parsed.end, field.key)
      const canonical = `${field.key}:${canonicalValue(parsed.value, field.kind)}`
      clause = { canonical, end: cursor, field, key: field.key, operator, start, value: parsed.value }
    }
    clauses.push(clause)
    cursor = firstNonSpace(input, cursor)
    if (cursor >= input.length) break
    const future = reservedAt(input, cursor)
    if (future !== null) return failure("unsupported_syntax", future.start, future.end, future.token)
    if (input.slice(cursor, cursor + 3).toLowerCase() !== "and"
      || (cursor > 0 && !/\s/.test(input[cursor - 1]!))
      || (cursor + 3 < input.length && !/\s/.test(input[cursor + 3]!))) {
      const tokenEnd = nextSpace(input, cursor)
      return failure("expected_and", cursor, tokenEnd, input.slice(cursor, tokenEnd))
    }
    cursor = firstNonSpace(input, cursor + 3)
    if (cursor >= input.length) return failure("empty_clause", input.length - 3, input.length, "AND")
  }
  return success(clauses.map(({ canonical }) => canonical).join(" AND "), clauses, null, true)
}

export function withoutSearchClause(query: SearchQuery, index: number): string {
  if (!query.structured) return ""
  return query.clauses.filter((_clause, at) => at !== index).map(({ canonical }) => canonical).join(" AND ")
}

export function rowMatchesSearch(row: DataRow, query: SearchQuery, surface: SearchSurface): boolean {
  if (query.canonical === "") return true
  const clauses = query.structured ? query.clauses : [{ field: searchFields(surface)[0]!, value: query.freeText ?? "" }]
  return clauses.every((clause) => {
    if (clause.field.kind === "quantity") return false
    return clause.field.columns.some((column) => {
      if (surface === "pg_store_plans" && clause.field.key === "query_id") {
        const wanted = row.typeId === "1004001" ? "queryid_stat_statements" : "queryid"
        if (column !== wanted) return false
      }
      const stored = rawText(value(row, column))
      if (stored === null) return false
      if (surface === "pg_store_plans" && row.typeId === "1004001" && clause.field.key === "query_id" && stored === "0") return false
      if (clause.field.kind === "identifier") return stored === clause.value
      return globMatcher(clause.value)?.(stored) ?? true
    })
  })
}

export function canonicalSearch(clauses: readonly { readonly key: string; readonly operator?: SearchOperator | undefined; readonly value: string }[], surface: SearchSurface): string | null {
  const fields = new Map(searchFields(surface).map((field) => [field.key, field] as const))
  const expression = clauses.map(({ key, operator = ":", value }) => {
    const field = fields.get(key)
    return `${key}${operator}${operator === ":" ? canonicalValue(value, field?.kind ?? "string") : value}`
  }).join(" AND ")
  const parsed = parseSearch(expression, surface)
  return parsed.ok ? parsed.query.canonical : null
}

function parseQuantity(token: string, kind: QuantityKind, offset: number): QuantityParseResult {
  if (token.startsWith("-")) return failure("negative_not_allowed", offset, offset + Math.max(1, token.length), token)
  if (token.startsWith("+")) return failure("invalid_number", offset, offset + token.length, token)
  const match = /^(0|[1-9]\d*)(?:\.(\d+))?(.*)$/.exec(token)
  if (match === null || token.includes(",") || token.includes("_") || /^[eE][+-]?\d/.test(match?.[3] ?? "")) return failure("invalid_number", offset, offset + token.length, token)
  const whole = match[1]!
  const fraction = match[2] ?? ""
  const unit = match[3] ?? ""
  const significant = `${whole}${fraction}`.replace(/^0+/, "") || "0"
  if (significant.length > 38 || fraction.length > 9) return failure("out_of_range", offset, offset + token.length, token)
  const trimmedFraction = fraction.replace(/0+$/, "")
  const canonicalNumber = trimmedFraction === "" ? whole : `${whole}.${trimmedFraction}`
  const scale = 10n ** BigInt(fraction.length)
  const coefficient = BigInt(`${whole}${fraction}`)
  let numerator = coefficient
  let denominator = scale

  if (kind === "bytes") {
    if (unit === "") return failure("unit_required", offset + whole.length + (fraction === "" ? 0 : fraction.length + 1), offset + token.length, token)
    const multiplier = BYTE_UNITS.get(unit)
    if (multiplier === undefined) return failure("invalid_unit", offset + token.length - unit.length, offset + token.length, unit)
    numerator *= multiplier
    if (numerator % denominator !== 0n) return failure("non_integral_base_value", offset, offset + token.length, token)
    numerator /= denominator
    denominator = 1n
  } else if (kind === "duration") {
    if (unit === "") return failure("unit_required", offset + token.length, offset + token.length, token)
    const factors = DURATION_UNITS.get(unit)
    if (factors === undefined) return failure("invalid_unit", offset + token.length - unit.length, offset + token.length, unit)
    numerator *= factors[0]
    denominator *= factors[1]
  } else if (kind === "count") {
    if (unit !== "") return failure("invalid_unit", offset + token.length - unit.length, offset + token.length, unit)
    if (fraction !== "") return failure("non_integral_base_value", offset, offset + token.length, token)
  } else if (kind === "count_rate") {
    if (unit === "") return failure("unit_required", offset + token.length, offset + token.length, token)
    if (unit !== "/s") return failure("invalid_unit", offset + token.length - unit.length, offset + token.length, unit)
  } else {
    if (unit === "") return failure("unit_required", offset + token.length, offset + token.length, token)
    if (unit !== "%") return failure("invalid_unit", offset + token.length - unit.length, offset + token.length, unit)
    if (coefficient > 100n * scale) return failure("out_of_range", offset, offset + token.length, token)
  }
  const divisor = gcd(numerator, denominator)
  numerator /= divisor
  denominator /= divisor
  if (numerator > MAX_U128 || denominator > MAX_U128) return failure("out_of_range", offset, offset + token.length, token)
  return { canonical: `${canonicalNumber}${unit}`, ok: true, quantity: { denominator, number: canonicalNumber, numerator, unit } }
}

function quotedValue(input: string, quote: number): ValueParseResult {
  let cursor = quote + 1
  let result = ""
  while (cursor < input.length) {
    const character = input[cursor]!
    if (character === '"') return { end: cursor + 1, ok: true, start: quote, value: result }
    if (character !== "\\") { result += character; cursor += 1; continue }
    const escaped = input[cursor + 1]
    if (escaped !== '"' && escaped !== "\\") return failure("invalid_escape", cursor, Math.min(input.length, cursor + 2))
    result += escaped
    cursor += 2
  }
  return failure("unterminated_quote", quote, input.length)
}

function bareValue(input: string, start: number): Extract<ValueParseResult, { readonly ok: true }> {
  let end = start
  while (end < input.length && !/\s/.test(input[end]!)) end += 1
  return { end, ok: true, start, value: input.slice(start, end) }
}

function canonicalValue(value: string, kind: SearchFieldKind): string {
  if (kind === "identifier" || /^[^\s:"\\]+$/.test(value)) return value
  return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`
}

function validIdentifier(input: string, signed: boolean): boolean {
  if (!(signed ? /^-?(0|[1-9]\d*)$/.test(input) : /^(0|[1-9]\d*)$/.test(input)) || input === "-0") return false
  try {
    const parsed = BigInt(input)
    return signed ? parsed >= -9_223_372_036_854_775_808n && parsed <= 9_223_372_036_854_775_807n : parsed >= 0n && parsed <= 18_446_744_073_709_551_615n
  } catch { return false }
}

function success(canonical: string, clauses: readonly SearchClause[], freeText: string | null, structured: boolean): SearchParseResult {
  const terms: SearchExpr[] = clauses.map((predicate) => ({ kind: "predicate", predicate }))
  const expr: SearchExpr | null = terms.length === 0 ? null : terms.length === 1 ? terms[0]! : { kind: "and", terms }
  return { ok: true, query: { canonical, clauses, expr, freeText, structured } }
}

function failure(code: SearchErrorCode, start: number, end: number, token?: string): SearchFailure {
  return { error: { code, end: Math.max(start, end), start, ...(token === undefined ? {} : { token }) }, ok: false }
}

function firstNonSpace(input: string, start: number): number { let cursor = start; while (cursor < input.length && /\s/.test(input[cursor]!)) cursor += 1; return cursor }
function nextSpace(input: string, start: number): number { let cursor = start; while (cursor < input.length && !/\s/.test(input[cursor]!)) cursor += 1; return cursor }
function gcd(left: bigint, right: bigint): bigint { while (right !== 0n) { const next = left % right; left = right; right = next } return left }

function hasStructuredSyntax(input: string): boolean {
  let quoted = false
  let escaped = false
  for (const character of input) {
    if (escaped) { escaped = false; continue }
    if (quoted && character === "\\") { escaped = true; continue }
    if (character === '"') { quoted = !quoted; continue }
    if (!quoted && ":<>!=".includes(character)) return true
  }
  return false
}

function reservedAt(input: string, start: number): { readonly end: number; readonly start: number; readonly token: string } | null {
  const character = input[start]
  if (character === "(" || character === ")") return { end: start + 1, start, token: character }
  for (const token of ["NOT", "OR"] as const) {
    if (input.slice(start, start + token.length).toUpperCase() === token
      && (start === 0 || /\s|\(/.test(input[start - 1]!))
      && (start + token.length === input.length || /\s|\(|\)/.test(input[start + token.length]!))) return { end: start + token.length, start, token }
  }
  return null
}

function looksLikeUnit(token: string): boolean {
  if (/^(?:AND|OR)$/i.test(token)) return false
  return BYTE_UNITS.has(token) || DURATION_UNITS.has(token) || token === "/s" || token === "%" || /^[A-Za-z%/]+$/.test(token)
}

function standaloneAnd(input: string, start: number): number | null {
  const match = /(?:^|\s)AND(?:\s|$)/i.exec(input.slice(start))
  return match === null ? null : start + match.index + (match[0].startsWith(" ") ? 1 : 0)
}
