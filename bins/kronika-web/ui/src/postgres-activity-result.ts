import type { Cell, DataRow, HourData, SnapshotRows } from "./api"
import type { PostgresqlActivityDirection, PostgresqlActivitySort } from "./postgres-activity-query"

export interface PostgresqlActivityPage {
  readonly nextCursor: string | null
  readonly observedAt: number | null
  readonly requestedAt: number
  readonly rows: readonly DataRow[]
}

const RESULT_FIELDS = ["requested_at", "observed_at", "rows", "next_cursor"] as const
const ROW_FIELDS = [
  "observed_at", "pid", "leader_pid", "datid", "datname", "usename", "application_name", "client_addr",
  "backend_type", "state", "wait_event_type", "wait_event", "query_preview", "query_id", "backend_xid_age",
  "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change", "backend_age_ms",
  "query_duration_ms", "transaction_duration_ms", "state_duration_ms",
] as const

export function parsePostgresqlActivityPage(stored: unknown, expectedAt: number): PostgresqlActivityPage {
  const result = exactObject(stored, RESULT_FIELDS, "Activity result")
  const requestedText = timestamp(result.requested_at, "Activity requested_at")
  const requestedAt = safeTimestamp(requestedText, "Activity requested_at")
  if (requestedAt !== expectedAt) throw new Error("Activity result does not match its request")
  const observedText = nullableTimestamp(result.observed_at, "Activity observed_at")
  const observedAt = observedText === null ? null : safeTimestamp(observedText, "Activity observed_at")
  const nextCursor = nullableCursor(result.next_cursor)
  if (!Array.isArray(result.rows) || result.rows.length > 5_000) throw new Error("Activity rows are invalid")
  if (observedAt === null && (result.rows.length !== 0 || nextCursor !== null)) {
    throw new Error("Activity empty observation is inconsistent")
  }
  const rows = result.rows.map((row, index) => activityRow(row, observedText, observedAt, index))
  return { nextCursor, observedAt, requestedAt, rows }
}

export function postgresqlActivityHourData(
  page: PostgresqlActivityPage,
  sort: PostgresqlActivitySort,
  direction: PostgresqlActivityDirection,
  pageSize: number,
): HourData {
  const metadata: SnapshotRows = {
    logicalName: "pg_stat_activity",
    eligible: null,
    returned: page.rows.length,
    hasMore: page.nextCursor !== null,
    truncated: false,
    nextCursor: page.nextCursor,
    pageSize,
    orderBy: [sort],
    orderDirection: direction,
    from: page.observedAt,
    to: page.observedAt,
  }
  return {
    sections: { pg_stat_activity: page.rows },
    rateColumns: {},
    snapshotRows: [metadata],
    availableSections: ["pg_stat_activity"],
    syntheticDemo: false,
    postgresqlConfigured: true,
    postgresqlPresent: true,
    processes: [],
    activities: page.rows,
    load: [],
    memory: [],
    pressure: [],
    health: [],
    points: [],
    lanePoints: [],
    findings: [],
    findingGroups: [],
  }
}

function activityRow(stored: unknown, resultObserved: string | null, observedAt: number | null, index: number): DataRow {
  const row = exactObject(stored, ROW_FIELDS, `Activity row ${index}`)
  const rowObserved = timestamp(row.observed_at, `Activity row ${index} observed_at`)
  if (resultObserved === null || rowObserved !== resultObserved || observedAt === null) {
    throw new Error(`Activity row ${index} observation is inconsistent`)
  }
  const pid = positiveI32(row.pid, `Activity row ${index} pid`)
  const queryPreview = nullableText(row.query_preview, `Activity row ${index} query_preview`)
  if (queryPreview !== null && [...queryPreview].length > 161) throw new Error(`Activity row ${index} query_preview is invalid`)
  const values: Readonly<Record<string, Cell>> = {
    observed_at: rowObserved,
    pid,
    leader_pid: nullablePositiveI32(row.leader_pid, `Activity row ${index} leader_pid`),
    datid: nullableU32(row.datid, `Activity row ${index} datid`),
    datname: nullableText(row.datname, `Activity row ${index} datname`),
    usename: nullableText(row.usename, `Activity row ${index} usename`),
    application_name: text(row.application_name, `Activity row ${index} application_name`),
    client_addr: text(row.client_addr, `Activity row ${index} client_addr`),
    backend_type: text(row.backend_type, `Activity row ${index} backend_type`),
    state: nullableText(row.state, `Activity row ${index} state`),
    wait_event_type: nullableText(row.wait_event_type, `Activity row ${index} wait_event_type`),
    wait_event: nullableText(row.wait_event, `Activity row ${index} wait_event`),
    query_preview: queryPreview,
    query_id: nullableI64(row.query_id, `Activity row ${index} query_id`),
    backend_xid_age: nullableI64(row.backend_xid_age, `Activity row ${index} backend_xid_age`),
    backend_xmin_age: nullableI64(row.backend_xmin_age, `Activity row ${index} backend_xmin_age`),
    backend_start: timestamp(row.backend_start, `Activity row ${index} backend_start`),
    xact_start: nullableTimestamp(row.xact_start, `Activity row ${index} xact_start`),
    query_start: nullableTimestamp(row.query_start, `Activity row ${index} query_start`),
    state_change: nullableTimestamp(row.state_change, `Activity row ${index} state_change`),
    backend_age_ms: nullableDuration(row.backend_age_ms, `Activity row ${index} backend_age_ms`),
    query_duration_ms: nullableDuration(row.query_duration_ms, `Activity row ${index} query_duration_ms`),
    transaction_duration_ms: nullableDuration(row.transaction_duration_ms, `Activity row ${index} transaction_duration_ms`),
    state_duration_ms: nullableDuration(row.state_duration_ms, `Activity row ${index} state_duration_ms`),
  }
  return {
    segmentId: `postgresql_activity:${rowObserved}`,
    logicalName: "pg_stat_activity",
    typeId: "postgresql_activity",
    ordinal: String(pid),
    timestamp: observedAt,
    values,
  }
}

function exactObject<const F extends readonly string[]>(stored: unknown, fields: F, label: string): Record<F[number], unknown> {
  if (stored === null || typeof stored !== "object" || Array.isArray(stored)) throw new Error(`${label} is invalid`)
  const keys = Object.keys(stored)
  if (keys.length !== fields.length || keys.some((key) => !fields.includes(key))) throw new Error(`${label} is invalid`)
  return stored as Record<F[number], unknown>
}

function text(stored: unknown, label: string): string {
  if (typeof stored !== "string") throw new Error(`${label} is invalid`)
  return stored
}

function nullableText(stored: unknown, label: string): string | null {
  return stored === null ? null : text(stored, label)
}

function timestamp(stored: unknown, label: string): string {
  const raw = text(stored, label)
  if (!canonicalI64(raw)) throw new Error(`${label} is invalid`)
  return raw
}

function nullableTimestamp(stored: unknown, label: string): string | null {
  return stored === null ? null : timestamp(stored, label)
}

function nullableI64(stored: unknown, label: string): string | null {
  return stored === null ? null : timestamp(stored, label)
}

function safeTimestamp(raw: string, label: string): number {
  const value = Number(raw)
  if (!Number.isSafeInteger(value)) throw new Error(`${label} is outside the UI timestamp range`)
  return value
}

function canonicalI64(raw: string): boolean {
  if (!/^(?:0|-[1-9][0-9]{0,18}|[1-9][0-9]{0,18})$/.test(raw) || raw.length > 20) return false
  try {
    const value = BigInt(raw)
    return value >= -9_223_372_036_854_775_808n && value <= 9_223_372_036_854_775_807n
  } catch {
    return false
  }
}

function positiveI32(stored: unknown, label: string): number {
  if (!Number.isInteger(stored) || (stored as number) < 1 || (stored as number) > 2_147_483_647) throw new Error(`${label} is invalid`)
  return stored as number
}

function nullablePositiveI32(stored: unknown, label: string): number | null {
  return stored === null ? null : positiveI32(stored, label)
}

function nullableU32(stored: unknown, label: string): number | null {
  if (stored === null) return null
  if (!Number.isInteger(stored) || (stored as number) < 0 || (stored as number) > 4_294_967_295) throw new Error(`${label} is invalid`)
  return stored as number
}

function nullableDuration(stored: unknown, label: string): number | null {
  if (stored === null) return null
  if (typeof stored !== "number" || !Number.isFinite(stored) || stored < 0) throw new Error(`${label} is invalid`)
  return stored
}

function nullableCursor(stored: unknown): string | null {
  if (stored === null) return null
  if (typeof stored !== "string" || stored.length === 0 || stored.length > 4_096) throw new Error("Activity next_cursor is invalid")
  return stored
}
