import type { DataRow } from "./api"
import { asNumber, rawText } from "./model"

// The console shows one entry per group of identical recorded rows. Every
// group key is a value the rows themselves carry; nothing is synthesized
// across streams.

export type EventTier = "critical" | "notable" | "routine"

export const EVENT_TIERS: readonly EventTier[] = ["critical", "notable", "routine"]

export type EventStat =
  | { readonly kind: "pg.errors"; readonly severity: number; readonly category: number | null; readonly sqlstate: string | null; readonly database: string | null; readonly username: string | null }
  | { readonly kind: "pg.slow"; readonly maxMs: number; readonly totalMs: number }
  | { readonly kind: "pg.autovacuum"; readonly analyze: boolean; readonly runs: number; readonly totalMs: number | null; readonly tuplesRemoved: number | null; readonly tuplesDead: number | null }
  | { readonly kind: "pg.checkpoints"; readonly completes: number; readonly timed: number; readonly requested: number; readonly maxSyncMs: number | null; readonly buffers: number | null }
  | { readonly kind: "pg.checkpoint_warning"; readonly secondsApart: number | null }
  | { readonly kind: "pg.locks"; readonly holders: string | null; readonly waiters: number; readonly maxMs: number | null; readonly targets: readonly string[] }
  | { readonly kind: "pg.lifecycle"; readonly lifecycle: number; readonly pid: number | null; readonly signal: number | null; readonly mode: string | null }
  | { readonly kind: "pgbouncer.events"; readonly level: number; readonly database: string | null }

export interface EventEntry {
  // Unique within one hour's console.
  readonly key: string
  readonly section: string
  readonly tier: EventTier
  // The recorded text the entry leads with: a pattern, a statement, a
  // relation, a message. Null when the title is built from `stat` alone.
  readonly text: string | null
  // Recorded occurrences: the sum of stored counts, or the row count.
  readonly count: number
  readonly firstTs: number
  readonly lastTs: number
  // Per-minute occurrence counts across the hour.
  readonly minutes: readonly number[]
  readonly stat: EventStat
  // The raw rows behind the group, in time order.
  readonly rows: readonly DataRow[]
}

export const MINUTE_COLUMNS = 60
const MINUTE_US = 60_000_000

const TIER_ORDER: Readonly<Record<EventTier, number>> = { critical: 0, notable: 1, routine: 2 }
const ERROR_TIERS: readonly EventTier[] = ["notable", "critical", "critical", "routine", "routine"]
const LIFECYCLE_TIERS: readonly EventTier[] = ["critical", "notable", "notable"]
const PGBOUNCER_TIERS: readonly EventTier[] = ["critical", "notable", "notable", "routine", "routine", "routine"]

export interface EventStreams {
  readonly [section: string]: readonly DataRow[]
}

// The recorded error category of an entry, when the entry is an error group.
export function errorCategory(stat: EventStat): number | null {
  return stat.kind === "pg.errors" ? stat.category : null
}

export function groupEvents(streams: EventStreams, hour: number): readonly EventEntry[] {
  const entries = [
    ...groupErrors(streams.pg_log_errors ?? [], hour),
    ...groupSlowQueries(streams.pg_log_slow_queries ?? [], hour),
    ...groupAutovacuum(streams.pg_log_autovacuum ?? [], hour),
    ...groupCheckpoints(streams.pg_log_checkpoints ?? [], hour),
    ...groupLockEpisodes(streams.pg_log_lock_waits ?? [], hour),
    ...lifecycleEntries(streams.pg_log_lifecycle ?? [], hour),
    ...groupPgbouncer(streams.pgbouncer_events ?? [], hour),
  ]
  return entries.slice().sort((left, right) => TIER_ORDER[left.tier] - TIER_ORDER[right.tier]
    || right.count - left.count
    || right.lastTs - left.lastTs
    || left.key.localeCompare(right.key))
}

function groupErrors(rows: readonly DataRow[], hour: number): readonly EventEntry[] {
  return grouped(rows, (row) => `${text(row, "severity") ?? ""}\u{1f}${text(row, "category") ?? ""}\u{1f}${text(row, "pattern") ?? ""}`)
    .map(({ key, first, members }) => {
      const weights = members.map((row) => number(row, "count") ?? 1)
      return build(`errors:${key}`, "pg_log_errors", members, hour, weights, {
        tier: ERROR_TIERS[number(first, "severity") ?? 0] ?? "notable",
        text: text(first, "pattern"),
        stat: {
          kind: "pg.errors",
          severity: number(first, "severity") ?? 0,
          category: number(first, "category"),
          sqlstate: text(first, "sqlstate"),
          database: shared(members, "database"),
          username: shared(members, "username"),
        },
      })
    })
}

function groupSlowQueries(rows: readonly DataRow[], hour: number): readonly EventEntry[] {
  return grouped(rows, (row) => text(row, "pattern") ?? "")
    .map(({ key, first, members }) => {
      const weights = members.map((row) => number(row, "count") ?? 1)
      const slowest = members.reduce((left, right) => (number(right, "max_duration_ms") ?? 0) > (number(left, "max_duration_ms") ?? 0) ? right : left, first)
      return build(`slow:${key}`, "pg_log_slow_queries", members, hour, weights, {
        tier: "notable",
        text: text(slowest, "sample") ?? key,
        stat: {
          kind: "pg.slow",
          maxMs: number(slowest, "max_duration_ms") ?? 0,
          totalMs: sum(members, "total_duration_ms") ?? 0,
        },
      })
    })
}

function groupAutovacuum(rows: readonly DataRow[], hour: number): readonly EventEntry[] {
  return grouped(rows, (row) => `${text(row, "kind") ?? ""}\u{1f}${text(row, "relation") ?? ""}`)
    .map(({ key, first, members }) => {
      const last = members[members.length - 1] ?? first
      return build(`autovacuum:${key}`, "pg_log_autovacuum", members, hour, null, {
        tier: "routine",
        text: text(first, "relation"),
        stat: {
          kind: "pg.autovacuum",
          analyze: number(first, "kind") === 1,
          runs: members.length,
          totalMs: sum(members, "elapsed_ms"),
          tuplesRemoved: sum(members, "tuples_removed"),
          tuplesDead: number(last, "tuples_dead_not_removable"),
        },
      })
    })
}

function groupCheckpoints(rows: readonly DataRow[], hour: number): readonly EventEntry[] {
  const warnings = rows.filter((row) => number(row, "phase") === 2)
  const ordinary = rows.filter((row) => number(row, "phase") !== 2)
  const entries: EventEntry[] = []
  if (ordinary.length > 0) {
    const starts = ordinary.filter((row) => number(row, "phase") === 0)
    const timed = starts.filter((row) => (text(row, "reason") ?? "").includes("time")).length
    const completes = ordinary.filter((row) => number(row, "phase") === 1)
    entries.push(build("checkpoints", "pg_log_checkpoints", ordinary, hour, null, {
      tier: "routine",
      text: null,
      count: Math.max(completes.length, starts.length),
      stat: {
        kind: "pg.checkpoints",
        completes: completes.length,
        timed,
        requested: starts.length - timed,
        maxSyncMs: max(completes, "sync_ms"),
        buffers: sum(completes, "buffers_written"),
      },
    }))
  }
  if (warnings.length > 0) {
    entries.push(build("checkpoints:warning", "pg_log_checkpoints", warnings, hour, null, {
      tier: "notable",
      text: null,
      stat: { kind: "pg.checkpoint_warning", secondsApart: min(warnings, "seconds_apart") },
    }))
  }
  return entries
}

function groupLockEpisodes(rows: readonly DataRow[], hour: number): readonly EventEntry[] {
  return grouped(rows, (row) => text(row, "holding_pids") ?? "")
    .map(({ key, members }) => {
      const waits = members.filter((row) => number(row, "kind") === 0)
      const waiters = new Set(members.map((row) => text(row, "pid") ?? "")).size
      return build(`locks:${key}`, "pg_log_lock_waits", members, hour, null, {
        tier: "notable",
        text: null,
        count: Math.max(waits.length, 1),
        stat: {
          kind: "pg.locks",
          holders: key === "" ? null : key,
          waiters,
          maxMs: max(members, "duration_ms"),
          targets: unique(members.map((row) => text(row, "lock_target") ?? "")).filter((target) => target !== ""),
        },
      })
    })
}

function lifecycleEntries(rows: readonly DataRow[], hour: number): readonly EventEntry[] {
  return rows.map((row, index) => build(`lifecycle:${index}:${row.ordinal}`, "pg_log_lifecycle", [row], hour, null, {
    tier: LIFECYCLE_TIERS[number(row, "kind") ?? 0] ?? "notable",
    text: text(row, "message"),
    stat: {
      kind: "pg.lifecycle",
      lifecycle: number(row, "kind") ?? 0,
      pid: number(row, "pid"),
      signal: number(row, "signal"),
      mode: text(row, "shutdown_mode"),
    },
  }))
}

function groupPgbouncer(rows: readonly DataRow[], hour: number): readonly EventEntry[] {
  return grouped(rows, (row) => `${text(row, "level") ?? ""}\u{1f}${text(row, "text") ?? ""}`)
    .map(({ key, first, members }) => {
      return build(`pgbouncer:${key}`, "pgbouncer_events", members, hour, null, {
        tier: PGBOUNCER_TIERS[number(first, "level") ?? 3] ?? "routine",
        text: text(first, "text"),
        stat: {
          kind: "pgbouncer.events",
          level: number(first, "level") ?? 3,
          database: shared(members, "database"),
        },
      })
    })
}

interface Group {
  readonly key: string
  // The earliest member; every group has at least one row.
  readonly first: DataRow
  readonly members: readonly DataRow[]
}

function grouped(rows: readonly DataRow[], keyOf: (row: DataRow) => string): readonly Group[] {
  const groups = new Map<string, DataRow[]>()
  for (const row of rows.slice().sort((left, right) => left.timestamp - right.timestamp)) {
    const key = keyOf(row)
    const members = groups.get(key)
    if (members === undefined) groups.set(key, [row])
    else members.push(row)
  }
  return [...groups.entries()].flatMap(([key, members]) => {
    const [first] = members
    return first === undefined ? [] : [{ key, first, members }]
  })
}

function build(
  key: string,
  section: string,
  members: readonly DataRow[],
  hour: number,
  weights: readonly number[] | null,
  shape: { readonly tier: EventTier; readonly text: string | null; readonly count?: number; readonly stat: EventStat },
): EventEntry {
  const minutes = Array.from({ length: MINUTE_COLUMNS }, () => 0)
  members.forEach((row, index) => {
    const bucket = Math.floor((row.timestamp - hour) / MINUTE_US)
    if (bucket >= 0 && bucket < MINUTE_COLUMNS) minutes[bucket] = (minutes[bucket] ?? 0) + (weights?.[index] ?? 1)
  })
  const timestamps = members.map((row) => row.timestamp)
  return {
    key,
    section,
    tier: shape.tier,
    text: shape.text,
    count: shape.count ?? (weights === null ? members.length : weights.reduce((total, weight) => total + weight, 0)),
    firstTs: Math.min(...timestamps),
    lastTs: Math.max(...timestamps),
    minutes,
    stat: shape.stat,
    rows: members,
  }
}

function text(row: DataRow, field: string): string | null {
  return rawText(row.values[field] ?? null)
}

function number(row: DataRow, field: string): number | null {
  return asNumber(row.values[field] ?? null)
}

// The value every member shares, or null when they differ.
function shared(members: readonly DataRow[], field: string): string | null {
  const values = unique(members.map((row) => text(row, field) ?? ""))
  const [only] = values
  return values.length === 1 && only !== undefined && only !== "" ? only : null
}

function sum(members: readonly DataRow[], field: string): number | null {
  const values = members.map((row) => number(row, field)).filter((value): value is number => value !== null)
  return values.length === 0 ? null : values.reduce((total, value) => total + value, 0)
}

function max(members: readonly DataRow[], field: string): number | null {
  const values = members.map((row) => number(row, field)).filter((value): value is number => value !== null)
  return values.length === 0 ? null : Math.max(...values)
}

function min(members: readonly DataRow[], field: string): number | null {
  const values = members.map((row) => number(row, field)).filter((value): value is number => value !== null)
  return values.length === 0 ? null : Math.min(...values)
}

function unique(values: readonly string[]): readonly string[] {
  return [...new Set(values)]
}
