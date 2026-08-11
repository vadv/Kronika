import type { Cell, DataRow, Finding, HourData, Point, SourceFamily } from "./api"

interface FixtureTable {
  readonly columns: readonly string[]
  readonly snapshots: readonly {
    readonly rows: readonly (readonly Cell[])[]
    readonly segment_id: string
    readonly ts: string
    readonly type_id: string
  }[]
}

type FixturePoint = readonly [string, number | null, string?]

const FIXTURE_TYPE_NAMES: Readonly<Record<string, string>> = {
  "1001003": "pg_stat_activity",
  "1002003": "pg_stat_statements",
  "1100001": "os_process",
  "1102001": "os_cpu",
  "2001001": "pg_log_errors",
  "2002001": "pg_log_checkpoints",
  "2003001": "pg_log_autovacuum",
  "2004001": "pg_log_slow_queries",
  "2007001": "pg_log_temp_files",
}

interface RealHourFixture {
  readonly findings: readonly {
    readonly category?: number
    readonly field_ordinal: number
    readonly kind: Finding["kind"]
    readonly row_ordinal: number
    readonly segment_id: string
    readonly t: string
    readonly type_id: string
  }[]
  readonly meta: {
    readonly captureFromUs: string
    readonly captureToUs: string
    readonly segments: number
  }
  readonly os: FixtureTable
  readonly pg: FixtureTable
  readonly system: {
    readonly cpuBusy: readonly FixturePoint[]
    readonly health: readonly FixturePoint[]
    readonly load1: readonly FixturePoint[]
    readonly memAvailable: readonly FixturePoint[]
    readonly minFsFree: readonly FixturePoint[]
    readonly oom: readonly FixturePoint[]
    readonly psi: Readonly<Record<string, readonly FixturePoint[]>>
  }
}

export function bundledFixtureRange(): { readonly from: number; readonly to: number } | null {
  const fixture = rawFixture()
  return fixture === null ? null : {
    from: Number(fixture.meta.captureFromUs),
    to: Number(fixture.meta.captureToUs),
  }
}

export function bundledFixtureHour(start: number): HourData | null {
  const fixture = rawFixture()
  if (fixture === null) return null
  const end = start + 3_600_000_000
  const within = (timestamp: number) => timestamp >= start && timestamp < end
  const segmentAt = segmentForTimestamp(fixture.os)
  const processes = tableRows(fixture.os, "os_process").filter((row) => within(row.timestamp))
  const activities = tableRows(fixture.pg, "pg_stat_activity").filter((row) => within(row.timestamp))
  const health = seriesRows(fixture.system.health, "health", "0", { field: "os_health", segmentAt })
    .filter((row) => within(row.timestamp))
  const load = seriesRows(fixture.system.load1, "os_loadavg", "1105001", { field: "load1", segmentAt })
    .filter((row) => within(row.timestamp))
  const memory = seriesRows(fixture.system.memAvailable, "os_meminfo", "1104001", {
    field: "mem_available_percent",
    segmentAt,
  }).filter((row) => within(row.timestamp))
  const pressure = Object.entries(fixture.system.psi).flatMap(([resource, series]) =>
    seriesRows(series, "os_psi", "1107001", {
      field: "some_avg10",
      extra: { resource: Number(resource) },
      segmentAt,
    }),
  ).filter((row) => within(row.timestamp))
  const points = fixturePoints(fixture, segmentAt).filter((point) => within(point.timestamp))
  const findings = fixture.findings.map((finding) => ({
    segmentId: finding.segment_id,
    logicalName: fixtureLogicalName(finding.type_id),
    kind: finding.kind,
    typeId: finding.type_id,
    timestamp: Number(finding.t),
    category: finding.category ?? null,
    rowOrdinal: String(finding.row_ordinal),
    fieldOrdinal: finding.field_ordinal,
  })).filter((finding) => within(finding.timestamp))
  const sourceFamilies: readonly SourceFamily[] = [
    { name: "os", configured: true, present: processes.length !== 0 },
    { name: "postgresql", configured: true, present: activities.length !== 0 },
    { name: "pgbouncer", configured: false, present: false },
    { name: "clickhouse", configured: false, present: false },
  ]
  const sections = {
    os_process: processes,
    pg_stat_activity: activities,
    health,
  }
  return {
    sections,
    rateColumns: {},
    availableSections: ["os_process", "pg_stat_activity", "health"],
    processes,
    activities,
    load,
    memory,
    pressure,
    health,
    pgOverview: [],
    pgStatements: [],
    pgLocks: [],
    pgDatabases: [],
    pgEvents: [],
    points,
    lanePoints: [],
    findings,
    sourceFamilies,
    segmentCount: fixture.meta.segments,
  }
}

function rawFixture(): RealHourFixture | null {
  const candidate = (globalThis as { readonly __KRONIKA_REAL_HOUR__?: unknown }).__KRONIKA_REAL_HOUR__
  return isFixture(candidate) ? candidate : null
}

function isFixture(value: unknown): value is RealHourFixture {
  if (value === null || typeof value !== "object") return false
  const candidate = value as Partial<RealHourFixture>
  return candidate.meta !== undefined
    && typeof candidate.meta.captureFromUs === "string"
    && typeof candidate.meta.captureToUs === "string"
    && Number.isSafeInteger(candidate.meta.segments)
    && table(candidate.os) && table(candidate.pg)
    && Array.isArray(candidate.findings)
    && candidate.system !== undefined
    && Array.isArray(candidate.system.health)
    && Array.isArray(candidate.system.load1)
    && Array.isArray(candidate.system.memAvailable)
    && candidate.system.psi !== null
    && typeof candidate.system.psi === "object"
}

function table(value: unknown): value is FixtureTable {
  if (value === null || typeof value !== "object") return false
  const candidate = value as Partial<FixtureTable>
  return Array.isArray(candidate.columns) && Array.isArray(candidate.snapshots)
}

function tableRows(table: FixtureTable, logicalName: string): readonly DataRow[] {
  const columns = new Map(table.columns.map((name, index) => [name, index]))
  const ordinalIndex = columns.get("ordinal")
  const timestampIndex = columns.get("ts")
  return table.snapshots.flatMap((snapshot) => snapshot.rows.map((cells, rowIndex) => {
    const values = Object.fromEntries(table.columns.flatMap((name, index) =>
      name === "ordinal" || name === "ts" ? [] : [[name, cells[index] ?? null]],
    ))
    return {
      segmentId: snapshot.segment_id,
      logicalName,
      typeId: snapshot.type_id,
      ordinal: text(ordinalIndex === undefined ? rowIndex : cells[ordinalIndex]),
      timestamp: Number(timestampIndex === undefined ? snapshot.ts : cells[timestampIndex]),
      values,
    }
  }))
}

function seriesRows(
  series: readonly FixturePoint[],
  logicalName: string,
  typeId: string,
  options: {
    readonly field: string
    readonly extra?: Readonly<Record<string, Cell>>
    readonly segmentAt: (timestamp: number) => string
  },
): readonly DataRow[] {
  return series.map(([timestamp, value, explicitSegment], ordinal) => ({
    segmentId: explicitSegment ?? options.segmentAt(Number(timestamp)),
    logicalName,
    typeId,
    ordinal: String(ordinal),
    timestamp: Number(timestamp),
    values: { ...options.extra, [options.field]: value },
  }))
}

function fixturePoints(fixture: RealHourFixture, segmentAt: (timestamp: number) => string): readonly Point[] {
  const series: readonly [string, string, string, readonly FixturePoint[]][] = [
    ["os_cpu", "1102001", "os_cpu_busy_percent", fixture.system.cpuBusy],
    ["health", "0", "os_health", fixture.system.health],
    ["os_loadavg", "1105001", "os_load1", fixture.system.load1],
    ["os_meminfo", "1104001", "os_mem_available_percent", fixture.system.memAvailable],
    ["os_mountinfo", "1112001", "os_min_filesystem_free_percent", fixture.system.minFsFree],
    ["os_vmstat", "1106001", "os_oom_kills", fixture.system.oom],
  ]
  const points = series.flatMap(([logicalName, typeId, name, values]) =>
    values.map(([timestamp, value, segmentId]) => ({
      segmentId: segmentId ?? segmentAt(Number(timestamp)),
      logicalName,
      typeId,
      series: name,
      timestamp: Number(timestamp),
      identity: {},
      value,
    })),
  )
  return points.concat(Object.entries(fixture.system.psi).flatMap(([resource, values]) =>
    values.map(([timestamp, value]) => ({
      segmentId: segmentAt(Number(timestamp)),
      logicalName: "os_psi",
      typeId: "1107001",
      series: "os_psi_some_avg10",
      timestamp: Number(timestamp),
      identity: { resource: Number(resource) },
      value,
    })),
  ))
}

function segmentForTimestamp(table: FixtureTable): (timestamp: number) => string {
  const snapshots = table.snapshots
    .map((snapshot) => ({ segmentId: snapshot.segment_id, timestamp: Number(snapshot.ts) }))
    .sort((left, right) => left.timestamp - right.timestamp)
  return (timestamp) => {
    let selected = snapshots[0]
    let distance = selected === undefined ? Number.POSITIVE_INFINITY : Math.abs(selected.timestamp - timestamp)
    for (const candidate of snapshots) {
      const candidateDistance = Math.abs(candidate.timestamp - timestamp)
      if (candidateDistance < distance || (candidateDistance === distance && candidate.timestamp < (selected?.timestamp ?? Number.POSITIVE_INFINITY))) {
        selected = candidate
        distance = candidateDistance
      }
    }
    return selected?.segmentId ?? "fixture"
  }
}

function fixtureLogicalName(typeId: string): string {
  const logicalName = FIXTURE_TYPE_NAMES[typeId]
  if (logicalName === undefined) throw new Error(`fixture type ${typeId} is not recognized`)
  return logicalName
}

function text(value: Cell | undefined): string {
  if (typeof value === "string") return value
  if (typeof value === "number" || typeof value === "boolean") return String(value)
  return "0"
}
