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

type FixturePoint = readonly [string, number | null]

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
    readonly oom: readonly (readonly [string, number, number])[]
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
  const processes = tableRows(fixture.os).filter((row) => within(row.timestamp))
  const activities = tableRows(fixture.pg).filter((row) => within(row.timestamp))
  const health = seriesRows(fixture.system.health, "fixture:health", { field: "os_health" }).filter((row) => within(row.timestamp))
  const load = seriesRows(fixture.system.load1, "fixture:load", { field: "load1" }).filter((row) => within(row.timestamp))
  const memory = seriesRows(fixture.system.memAvailable, "fixture:memory", { field: "mem_available" }).filter((row) => within(row.timestamp))
  const pressure = Object.entries(fixture.system.psi).flatMap(([resource, series]) =>
    seriesRows(series, `fixture:pressure:${resource}`, { field: "some_avg10", extra: { resource: Number(resource) } }),
  ).filter((row) => within(row.timestamp))
  const points = fixturePoints(fixture).filter((point) => within(point.timestamp))
  const findings = fixture.findings.map((finding) => ({
    segmentId: finding.segment_id,
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
  return {
    processes,
    activities,
    load,
    memory,
    pressure,
    health,
    points,
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

function tableRows(table: FixtureTable): readonly DataRow[] {
  const columns = new Map(table.columns.map((name, index) => [name, index]))
  const ordinalIndex = columns.get("ordinal")
  const timestampIndex = columns.get("ts")
  return table.snapshots.flatMap((snapshot) => snapshot.rows.map((cells, rowIndex) => {
    const values = Object.fromEntries(table.columns.flatMap((name, index) =>
      name === "ordinal" || name === "ts" ? [] : [[name, cells[index] ?? null]],
    ))
    return {
      segmentId: snapshot.segment_id,
      typeId: snapshot.type_id,
      ordinal: text(ordinalIndex === undefined ? rowIndex : cells[ordinalIndex]),
      timestamp: Number(timestampIndex === undefined ? snapshot.ts : cells[timestampIndex]),
      values,
    }
  }))
}

function seriesRows(
  series: readonly FixturePoint[],
  typeId: string,
  options: { readonly field: string; readonly extra?: Readonly<Record<string, Cell>> },
): readonly DataRow[] {
  return series.map(([timestamp, value], ordinal) => ({
    segmentId: "fixture",
    typeId,
    ordinal: String(ordinal),
    timestamp: Number(timestamp),
    values: { ...options.extra, [options.field]: value },
  }))
}

function fixturePoints(fixture: RealHourFixture): readonly Point[] {
  const series: readonly [string, readonly FixturePoint[]][] = [
    ["os_cpu_busy_percent", fixture.system.cpuBusy],
    ["os_health", fixture.system.health],
    ["os_load1", fixture.system.load1],
    ["os_mem_available_kib", fixture.system.memAvailable],
    ["os_min_filesystem_free_percent", fixture.system.minFsFree],
  ]
  return series.flatMap(([name, values]) => values.map(([timestamp, value]) => ({
    segmentId: "fixture",
    series: name,
    timestamp: Number(timestamp),
    value,
  })))
}

function text(value: Cell | undefined): string {
  if (typeof value === "string") return value
  if (typeof value === "number" || typeof value === "boolean") return String(value)
  return "0"
}
