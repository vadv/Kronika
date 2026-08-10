import { registry } from "kronika:registry"

import { bundledFixtureHour, bundledFixtureRange } from "./fixture"
import { parseNdjson } from "./wire"

export type Cell = null | boolean | number | string | { readonly [key: string]: unknown }

const BUNDLED_TYPE_IDS = new Set(registry.map((layout) => layout.typeId))

export interface DataRow {
  readonly segmentId: string
  readonly typeId: string
  readonly ordinal: string
  readonly timestamp: number
  readonly values: Readonly<Record<string, Cell>>
}

export interface Point {
  readonly segmentId: string
  readonly series: string
  readonly timestamp: number
  readonly value: number | null
}

export interface Finding {
  readonly segmentId: string
  readonly kind: "known_bad" | "spike" | "event"
  readonly typeId: string
  readonly timestamp: number
  readonly category: number | null
  readonly rowOrdinal: string
  readonly fieldOrdinal: number
}

interface Section {
  readonly logical_name: string | null
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
  readonly processes: readonly DataRow[]
  readonly activities: readonly DataRow[]
  readonly load: readonly DataRow[]
  readonly memory: readonly DataRow[]
  readonly pressure: readonly DataRow[]
  readonly health: readonly DataRow[]
  readonly points: readonly Point[]
  readonly findings: readonly Finding[]
  readonly sourceFamilies: readonly SourceFamily[]
  readonly segmentCount: number
}

export const PROCESS_FIELDS = [
  "pid", "starttime", "ppid", "uid", "euid", "gid", "egid", "state", "num_threads",
  "tty", "comm", "cmdline", "utime", "stime", "nice", "prio", "rtprio", "policy",
  "curcpu", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "minflt", "majflt",
  "vmem_kb", "rmem_kb", "vswap_kb", "syscr", "syscw", "rchar", "wchar", "read_bytes",
  "write_bytes", "cancelled_write_bytes", "exit_signal", "scope",
] as const

export const ACTIVITY_FIELDS = [
  "pid", "leader_pid", "datname", "usename", "application_name", "client_addr", "backend_type",
  "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age",
  "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change",
] as const

export async function discoverLatestHour(signal: AbortSignal): Promise<number> {
  const fixture = bundledFixtureRange()
  if (fixture !== null) return floorHour(fixture.from)
  const records = await request("/api/catalog", signal)
  const segments = catalogSegments(records)
  const latest = segments.reduce((current, segment) => Math.max(current, Number(segment.max_ts)), 0)
  return latest > 0 ? floorHour(latest) : floorHour(Date.now() * 1_000)
}

export async function loadHour(start: number, signal: AbortSignal): Promise<HourData> {
  const fixture = bundledFixtureHour(start)
  if (fixture !== null) return fixture
  const end = start + 3_600_000_000
  const catalog = await request(`/api/catalog?from=${start}&to=${end - 1}`, signal)
  const segments = catalogSegments(catalog).filter(
    (segment) => Number(segment.min_ts) < end && Number(segment.max_ts) >= start,
  )
  const sourceFamilies = catalog
    .find((record) => record.record === "catalog")?.source_families as readonly SourceFamily[] | undefined
  const loaded = []
  for (const segment of segments) loaded.push(await loadSegment(segment, signal))
  const within = (row: { readonly timestamp: number }) => row.timestamp >= start && row.timestamp < end
  return {
    processes: loaded.flatMap((part) => part.processes).filter(within),
    activities: loaded.flatMap((part) => part.activities).filter(within),
    load: loaded.flatMap((part) => part.load).filter(within),
    memory: loaded.flatMap((part) => part.memory).filter(within),
    pressure: loaded.flatMap((part) => part.pressure).filter(within),
    health: loaded.flatMap((part) => part.health).filter(within),
    points: loaded.flatMap((part) => part.points).filter(within),
    findings: loaded.flatMap((part) => part.findings).filter(within),
    sourceFamilies: sourceFamilies ?? [],
    segmentCount: segments.length,
  }
}

async function loadSegment(segment: Segment, signal: AbortSignal) {
  const names = new Set(segment.sections.flatMap((section) => section.logical_name === null ? [] : [section.logical_name]))
  const history = (section: string, fields: readonly string[]) => names.has(section)
    ? readHistory(segment.id, section, fields, signal)
    : Promise.resolve([])
  const indexed = [
    "os_process", "os_cpu", "os_meminfo", "os_loadavg", "os_vmstat", "os_mountinfo",
    "pg_stat_activity", "pg_stat_database", "pg_stat_statements", "pg_log_errors",
    "pg_log_checkpoints", "pg_log_autovacuum", "pg_log_slow_queries", "pg_log_lock_waits",
    "pg_log_lifecycle", "pg_log_temp_files",
  ]
    .filter((section) => names.has(section))
  if ([...names].some((name) => name.startsWith("os_"))) indexed.push("health")
  const [processes, activities, load, memory, pressure, health, indexes] = await Promise.all([
    history("os_process", PROCESS_FIELDS),
    history("pg_stat_activity", ACTIVITY_FIELDS),
    history("os_loadavg", ["load1", "load5", "load15", "running", "total"]),
    history("os_meminfo", ["mem_total", "mem_available"]),
    history("os_psi", ["resource", "some_avg10", "full_avg10"]),
    readHistory(segment.id, "health", ["health"], signal).catch((error: unknown) => absent(error)),
    mapLimited(indexed, 2, (section) => readIndex(segment.id, section, signal).catch((error: unknown) => absentIndex(error))),
  ])
  return {
    processes,
    activities,
    load,
    memory,
    pressure,
    health,
    points: indexes.flatMap((index) => index.points),
    findings: indexes.flatMap((index) => index.findings),
  }
}

async function readHistory(
  segmentId: string,
  section: string,
  fields: readonly string[],
  signal: AbortSignal,
): Promise<readonly DataRow[]> {
  const query = fields.map((field) => `field=${encodeURIComponent(field)}`).join("&")
  const records = await request(`/api/segments/${segmentId}/sections/${section}/history?${query}`, signal)
  const layouts = new Map<string, readonly string[]>()
  const rows: DataRow[] = []
  for (const record of records) {
    if (record.record === "layout") {
      const layout = record.layout as { readonly type_id: string; readonly columns: readonly { readonly name: string }[] }
      if (layout.type_id !== "0" && !BUNDLED_TYPE_IDS.has(layout.type_id)) {
        throw new Error(`layout ${layout.type_id} is not in the bundled registry`)
      }
      layouts.set(layout.type_id, layout.columns.map((column) => column.name))
    } else if (record.record === "row") {
      const typeId = text(record.type_id)
      const names = layouts.get(typeId)
      const values = record.values as readonly Cell[]
      if (names === undefined || !Array.isArray(values)) continue
      rows.push({
        segmentId,
        typeId,
        ordinal: text(record.ordinal),
        timestamp: Number(record.timestamp),
        values: Object.fromEntries(names.map((name, index) => [name, values[index] ?? null])),
      })
    }
  }
  return rows
}

async function readIndex(segmentId: string, section: string, signal: AbortSignal) {
  const records = await request(`/api/segments/${segmentId}/sections/${section}/index`, signal)
  const points: Point[] = []
  const findings: Finding[] = []
  for (const record of records) {
    if (record.record === "point") {
      points.push({
        segmentId,
        series: text(record.series),
        timestamp: Number(record.ts),
        value: record.value === null ? null : Number(record.value),
      })
    } else if (record.record === "finding"
      && (record.kind === "known_bad" || record.kind === "spike" || record.kind === "event")) {
      findings.push({
        segmentId,
        kind: record.kind,
        typeId: text(record.type_id),
        timestamp: Number(record.ts),
        category: typeof record.category === "number" ? record.category : null,
        rowOrdinal: text(record.row_ordinal),
        fieldOrdinal: typeof record.field_ordinal === "number" ? record.field_ordinal : 0,
      })
    }
  }
  return { points, findings }
}

async function request(path: string, signal: AbortSignal): Promise<readonly Record<string, unknown>[]> {
  const response = await fetch(path, { headers: { Accept: "application/x-ndjson" }, signal })
  if (!response.ok) {
    const error = new Error(`HTTP ${response.status} for ${path}`) as Error & { status?: number }
    error.status = response.status
    throw error
  }
  const body = await response.text()
  return parseNdjson(body, path)
}

function catalogSegments(records: readonly Record<string, unknown>[]): readonly Segment[] {
  return records.filter(
    (record) => record.record === "finished_segment" || record.record === "active_segment",
  ) as unknown as readonly Segment[]
}

function absent(error: unknown): readonly DataRow[] {
  if (status(error) === 404) return []
  throw error
}

function absentIndex(error: unknown): { readonly points: readonly Point[]; readonly findings: readonly Finding[] } {
  if (status(error) === 404) return { points: [], findings: [] }
  throw error
}

function status(error: unknown): number | undefined {
  return error instanceof Error ? (error as Error & { readonly status?: number }).status : undefined
}

function text(value: unknown): string {
  return typeof value === "string" ? value : String(value)
}

async function mapLimited<Input, Output>(
  inputs: readonly Input[],
  limit: number,
  run: (input: Input) => Promise<Output>,
): Promise<readonly Output[]> {
  const outputs = new Array<Output>(inputs.length)
  let next = 0
  const worker = async () => {
    while (next < inputs.length) {
      const index = next
      next += 1
      const input = inputs[index]
      if (input !== undefined) outputs[index] = await run(input)
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, inputs.length) }, worker))
  return outputs
}

function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}
