import type { DataRow } from "./api"
import { asNumber, rawText } from "./model"

export type EventTier = "critical" | "notable" | "routine"

export const EVENT_TIERS: readonly EventTier[] = ["critical", "notable", "routine"]

export type EventStat =
  | { readonly kind: "pg.errors"; readonly severity: number; readonly category: number | null; readonly sqlstate: string | null; readonly database: string | null; readonly username: string | null }
  | { readonly kind: "pg.slow"; readonly maxMs: number; readonly totalMs: number; readonly thresholdMs: number | null }
  | { readonly kind: "pg.autovacuum"; readonly analyze: boolean; readonly runs: number; readonly totalMs: number | null; readonly tuplesRemoved: number | null; readonly tuplesDead: number | null }
  | { readonly kind: "pg.checkpoints"; readonly completes: number; readonly timed: number; readonly requested: number; readonly maxSyncMs: number | null; readonly buffers: number | null }
  | { readonly kind: "pg.checkpoint_warning"; readonly secondsApart: number | null }
  | { readonly kind: "pg.locks"; readonly holders: string | null; readonly acquired: boolean; readonly waiters: number; readonly maxMs: number | null; readonly targets: readonly string[] }
  | { readonly kind: "pg.lifecycle"; readonly lifecycle: number; readonly pid: number | null; readonly signal: number | null; readonly mode: string | null }
  | { readonly kind: "pgbouncer.events"; readonly level: number; readonly database: string | null }

export interface EventEntry {
  readonly key: string
  readonly section: string
  readonly tier: EventTier
  // Pattern, statement, relation, or message; null for stat-only titles.
  readonly text: string | null
  // Sum of stored counts, or the row count when no count is stored.
  readonly count: number
  readonly firstTs: number
  readonly lastTs: number
  readonly minutes: readonly number[]
  readonly stat: EventStat
  // Source rows in timestamp order.
  readonly rows: readonly DataRow[]
}

// The recorded log_min_duration_statement, in milliseconds. Its unit is
// recorded beside it; a negative value means the server logs no statement for
// its duration at all.
export function slowThresholdMs(rows: readonly DataRow[]): number | null {
  const last = rows
    .filter((row) => text(row, "name") === "log_min_duration_statement")
    .reduce<DataRow | null>((chosen, row) => chosen === null || row.timestamp > chosen.timestamp ? row : chosen, null)
  if (last === null) return null
  const setting = Number(text(last, "setting"))
  if (!Number.isFinite(setting) || setting < 0) return null
  const unit = text(last, "unit")
  if (unit === "s") return setting * 1000
  if (unit === "min") return setting * 60_000
  return setting
}

export const MINUTE_COLUMNS = 60
const MINUTE_US = 60_000_000

const TIER_ORDER: Readonly<Record<EventTier, number>> = { critical: 0, notable: 1, routine: 2 }
const ERROR_TIERS: readonly EventTier[] = ["notable", "critical", "critical", "routine", "routine"]
const LIFECYCLE_TIERS: readonly EventTier[] = ["critical", "notable", "notable"]
const PGBOUNCER_TIERS: readonly EventTier[] = ["critical", "notable", "notable", "routine", "routine", "routine"]

export function groupEvents(streams: Readonly<Record<string, readonly DataRow[]>>, hour: number): readonly EventEntry[] {
  const thresholdMs = slowThresholdMs(streams.pg_settings ?? [])
  const entries = [
    ...groupErrors(streams.pg_log_errors ?? [], hour),
    ...groupSlowQueries(streams.pg_log_slow_queries ?? [], hour, thresholdMs),
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

function groupSlowQueries(rows: readonly DataRow[], hour: number, thresholdMs: number | null): readonly EventEntry[] {
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
          thresholdMs,
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
  // Acquired rows omit holder DETAIL and join waits with the same pid and target.
  const episodes = grouped(rows.filter((row) => number(row, "kind") === 0), (row) => text(row, "holding_pids") ?? "")
  const waiterOf = new Map<string, string>()
  for (const { key, members } of episodes) {
    for (const row of members) waiterOf.set(`${text(row, "pid") ?? ""}\u{1f}${text(row, "lock_target") ?? ""}`, key)
  }
  const attached = new Map<string, DataRow[]>()
  const leftovers: DataRow[] = []
  for (const row of rows.filter((row) => number(row, "kind") !== 0)) {
    const key = waiterOf.get(`${text(row, "pid") ?? ""}\u{1f}${text(row, "lock_target") ?? ""}`)
    if (key === undefined) {
      leftovers.push(row)
      continue
    }
    const joined = attached.get(key)
    if (joined === undefined) attached.set(key, [row])
    else joined.push(row)
  }
  const lockEntry = (key: string, waits: number, acquired: boolean, members: readonly DataRow[]) => build(
    `locks:${acquired ? "acquired" : key}`,
    "pg_log_lock_waits",
    members,
    hour,
    null,
    {
      tier: "notable",
      text: null,
      count: Math.max(waits, 1),
      stat: {
        kind: "pg.locks",
        holders: key === "" ? null : key,
        acquired,
        waiters: new Set(members.map((row) => text(row, "pid") ?? "")).size,
        maxMs: max(members, "duration_ms"),
        targets: unique(members.map((row) => text(row, "lock_target") ?? "")).filter((target) => target !== ""),
      },
    },
  )
  const entries = episodes.map(({ key, members }) => lockEntry(
    key,
    members.length,
    false,
    [...members, ...(attached.get(key) ?? [])].sort((left, right) => left.timestamp - right.timestamp),
  ))
  if (leftovers.length > 0) entries.push(lockEntry("", leftovers.length, true, leftovers))
  return entries
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
  // `grouped` never returns an empty group.
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
  // Avoid spread argument limits for large groups.
  let firstTs = Number.POSITIVE_INFINITY
  let lastTs = Number.NEGATIVE_INFINITY
  for (const row of members) {
    if (row.timestamp < firstTs) firstTs = row.timestamp
    if (row.timestamp > lastTs) lastTs = row.timestamp
  }
  return {
    key,
    section,
    tier: shape.tier,
    text: shape.text,
    count: shape.count ?? (weights === null ? members.length : weights.reduce((total, weight) => total + weight, 0)),
    firstTs,
    lastTs,
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

// Returns the shared value, or null when members differ.
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
  let found: number | null = null
  for (const row of members) {
    const value = number(row, field)
    if (value !== null && (found === null || value > found)) found = value
  }
  return found
}

function min(members: readonly DataRow[], field: string): number | null {
  let found: number | null = null
  for (const row of members) {
    const value = number(row, field)
    if (value !== null && (found === null || value < found)) found = value
  }
  return found
}

function unique(values: readonly string[]): readonly string[] {
  return [...new Set(values)]
}
