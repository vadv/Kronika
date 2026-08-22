import type { DataRow } from "./api"
import { asNumber, rawText, value } from "./model"

// The vacuum ledger: one recorded hour of pg_stat_progress_vacuum, grouped
// into episodes for display. Grouping is presentation over recorded values,
// the same shape as the events console: rows group by recorded identity
// fields, and nothing is inferred about time nobody recorded.

// The union of every layout's columns. A field a layout does not define
// arrives as null under this projection, which reads as N/A, never as 0.
export const VACUUM_HOUR_FIELDS = [
  "pid", "datid", "datname", "relid", "is_autovacuum", "phase",
  "heap_blks_total", "heap_blks_scanned", "heap_blks_vacuumed", "index_vacuum_count",
  "max_dead_tuples", "num_dead_tuples",
  "max_dead_tuple_bytes", "dead_tuple_bytes", "num_dead_item_ids",
  "indexes_total", "indexes_processed", "delay_time",
] as const

// Two samples of one key continue an episode when they are no further apart
// than this many recorded sampling intervals: collection drifts, so exactly
// one interval would split ordinary runs. Without a recorded interval the
// time condition is not applied — nothing is invented in its place.
export const EPISODE_ADJACENCY_FACTOR = 2.5

// How many consecutive samples with an unchanged designated counter make the
// "No movement" reading.
export const NO_MOVEMENT_SAMPLES = 3

export type VacuumRisk = "ordinary" | "heavy" | "dangerous"

// Fixed by phase name, never computed from observed load. `truncating heap`
// is dangerous unconditionally: the phase is set before the conditional
// AccessExclusiveLock attempt.
const PHASE_RISK: Readonly<Record<string, VacuumRisk>> = {
  "cleaning up indexes": "heavy",
  "initializing": "ordinary",
  "performing final cleanup": "ordinary",
  "scanning heap": "ordinary",
  "truncating heap": "dangerous",
  "vacuuming heap": "heavy",
  "vacuuming indexes": "heavy",
}

export function phaseRisk(phase: string | null): VacuumRisk {
  return phase === null ? "ordinary" : PHASE_RISK[phase] ?? "ordinary"
}

export interface VacuumEpisode {
  /// Recorded samples, ascending by timestamp.
  readonly rows: readonly DataRow[]
  readonly last: DataRow
  /// The trailing run of samples sharing the last row's phase and index
  /// cycle: a cycle increment starts a new span even when the phase string
  /// is unchanged.
  readonly phaseRows: readonly DataRow[]
  readonly noMovement: { readonly samples: number; readonly spanUs: number } | null
}

// Counters that only grow within one vacuum run. A row where any of them is
// lower than in the previous row is a different run and starts a new episode.
const MONOTONE_FIELDS = ["index_vacuum_count", "heap_blks_scanned", "heap_blks_vacuumed"] as const

function episodeKey(row: DataRow): string {
  return [row.typeId, rawText(value(row, "pid")) ?? "", rawText(value(row, "datid")) ?? "", rawText(value(row, "relid")) ?? ""].join(":")
}

export function buildVacuumEpisodes(rows: readonly DataRow[], intervalSeconds: number | null): readonly VacuumEpisode[] {
  const streams = new Map<string, DataRow[]>()
  for (const row of [...rows].sort((left, right) => left.timestamp - right.timestamp)) {
    const key = episodeKey(row)
    const stored = streams.get(key)
    if (stored === undefined) streams.set(key, [row])
    else stored.push(row)
  }
  const limit = intervalSeconds === null || intervalSeconds <= 0
    ? null
    : intervalSeconds * 1_000_000 * EPISODE_ADJACENCY_FACTOR
  const episodes: VacuumEpisode[] = []
  for (const stream of streams.values()) {
    let current: DataRow[] = []
    for (const row of stream) {
      const previous = current[current.length - 1]
      const continues = previous !== undefined
        && (limit === null || row.timestamp - previous.timestamp <= limit)
        && MONOTONE_FIELDS.every((field) => {
          const before = asNumber(value(previous, field))
          const after = asNumber(value(row, field))
          return before === null || after === null || after >= before
        })
      if (!continues && current.length > 0) {
        episodes.push(finishEpisode(current))
        current = []
      }
      current.push(row)
    }
    if (current.length > 0) episodes.push(finishEpisode(current))
  }
  return episodes
}

function finishEpisode(rows: readonly DataRow[]): VacuumEpisode {
  const last = rows[rows.length - 1] as DataRow
  const phase = rawText(value(last, "phase"))
  const cycle = asNumber(value(last, "index_vacuum_count"))
  let start = rows.length - 1
  while (start > 0) {
    const candidate = rows[start - 1] as DataRow
    if (rawText(value(candidate, "phase")) !== phase) break
    if (asNumber(value(candidate, "index_vacuum_count")) !== cycle) break
    start -= 1
  }
  const phaseRows = rows.slice(start)
  return { rows, last, phaseRows, noMovement: noMovement(phaseRows, last) }
}

// The counter whose movement says the phase is progressing. Layout 1_012_001
// does not record index progress, so its index phases never claim stillness.
function movementField(phase: string | null, typeId: string): string | null {
  if (phase === "scanning heap") return "heap_blks_scanned"
  if (phase === "vacuuming heap") return "heap_blks_vacuumed"
  if (phase === "vacuuming indexes" || phase === "cleaning up indexes") {
    return typeId === "1012001" ? null : "indexes_processed"
  }
  if (phase === "truncating heap") return "phase"
  return null
}

function noMovement(phaseRows: readonly DataRow[], last: DataRow): VacuumEpisode["noMovement"] {
  const field = movementField(rawText(value(last, "phase")), last.typeId)
  if (field === null) return null
  let start = phaseRows.length - 1
  if (field !== "phase") {
    const reading = asNumber(value(last, field))
    if (reading === null) return null
    while (start > 0 && asNumber(value(phaseRows[start - 1] as DataRow, field)) === reading) start -= 1
  } else {
    start = 0
  }
  const still = phaseRows.slice(start)
  if (still.length < NO_MOVEMENT_SAMPLES) return null
  const first = still[0] as DataRow
  return { samples: still.length, spanUs: last.timestamp - first.timestamp }
}

// The moment of the collection pass the cursor stands on: the newest recorded
// progress timestamp at or before the cursor. An episode whose last sample
// carries this timestamp was seen at the cursor; every other episode was last
// seen at its own recorded moment.
export function vacuumAtTimestamp(rows: readonly DataRow[], cursor: number): number | null {
  let at: number | null = null
  for (const row of rows) {
    if (row.timestamp <= cursor && (at === null || row.timestamp > at)) at = row.timestamp
  }
  return at
}

const RISK_ORDER: Readonly<Record<VacuumRisk, number>> = { dangerous: 0, heavy: 1, ordinary: 2 }

// At-sample episodes first, riskiest first, longest phase first; everything
// last seen earlier follows, newest first.
export function sortVacuumEpisodes(episodes: readonly VacuumEpisode[], atTs: number | null): readonly VacuumEpisode[] {
  return [...episodes].sort((left, right) => {
    const leftAt = atTs !== null && left.last.timestamp === atTs
    const rightAt = atTs !== null && right.last.timestamp === atTs
    if (leftAt !== rightAt) return leftAt ? -1 : 1
    if (leftAt) {
      const risk = RISK_ORDER[phaseRisk(rawText(value(left.last, "phase")))] - RISK_ORDER[phaseRisk(rawText(value(right.last, "phase")))]
      if (risk !== 0) return risk
      const span = phaseSpanUs(right) - phaseSpanUs(left)
      if (span !== 0) return span
      const cycles = (asNumber(value(right.last, "index_vacuum_count")) ?? 0) - (asNumber(value(left.last, "index_vacuum_count")) ?? 0)
      if (cycles !== 0) return cycles
    }
    return right.last.timestamp - left.last.timestamp
  })
}

export function phaseSpanUs(episode: VacuumEpisode): number {
  const first = episode.phaseRows[0]
  return first === undefined ? 0 : episode.last.timestamp - first.timestamp
}

// Whether any loaded row's layout defines the column: PG17 index progress
// exists from 1_012_002 on, the PG18 cost delay only on 1_012_003. In an
// hour without such layouts the columns are omitted rather than all-N/A.
export function vacuumLayoutHas(rows: readonly DataRow[], field: "indexes_total" | "delay_time"): boolean {
  return rows.some((row) => field === "delay_time" ? row.typeId === "1012003" : row.typeId !== "1012001")
}

// The PG18 cost-delay delta between the episode's last two samples, when the
// layout records it. Cumulative milliseconds; adjacent samples only.
export function delayDelta(episode: VacuumEpisode): number | null {
  if (episode.last.typeId !== "1012003") return null
  const previous = episode.rows[episode.rows.length - 2]
  if (previous === undefined) return null
  const before = asNumber(value(previous, "delay_time"))
  const after = asNumber(value(episode.last, "delay_time"))
  return before === null || after === null || after < before ? null : after - before
}
