// The curated cuts of every activity ledger. Each answers one forensic
// question; the menus stay short on purpose. A cut may sum several physical
// counters — the server adds the present fields per row, so the delta of the
// sum stays a counter. Block counters are stored in blocks and scale to bytes
// with the recorded block size; without one they stay block counts.
export interface ActivityCut {
  readonly id: string
  readonly fields: readonly string[]
  readonly kind: "milliseconds" | "count" | "bytes"
  readonly blockScaled?: boolean
}

export const STATEMENT_CUTS: readonly ActivityCut[] = [
  { id: "exec_time", fields: ["total_exec_time"], kind: "milliseconds" },
  { id: "calls", fields: ["calls"], kind: "count" },
  { id: "rows", fields: ["rows"], kind: "count" },
  { id: "shared_read", fields: ["shared_blks_read"], kind: "bytes", blockScaled: true },
  { id: "shared_dirtied", fields: ["shared_blks_dirtied"], kind: "bytes", blockScaled: true },
  { id: "temp_written", fields: ["temp_blks_written"], kind: "bytes", blockScaled: true },
  { id: "wal_bytes", fields: ["wal_bytes"], kind: "bytes" },
]

// pg_store_plans has no WAL or planning counters shared across its forks;
// execution time is total_time in every fork.
export const PLAN_CUTS: readonly ActivityCut[] = [
  { id: "exec_time", fields: ["total_time"], kind: "milliseconds" },
  { id: "calls", fields: ["calls"], kind: "count" },
  { id: "rows", fields: ["rows"], kind: "count" },
  { id: "shared_read", fields: ["shared_blks_read"], kind: "bytes", blockScaled: true },
  { id: "temp_written", fields: ["temp_blks_written"], kind: "bytes", blockScaled: true },
]

export const TABLE_CUTS: readonly ActivityCut[] = [
  { id: "writes", fields: ["n_tup_ins", "n_tup_upd", "n_tup_del"], kind: "count" },
  { id: "seq_read", fields: ["seq_tup_read"], kind: "count" },
  { id: "heap_read", fields: ["heap_blks_read"], kind: "bytes", blockScaled: true },
  { id: "dead_tuples", fields: ["n_dead_tup"], kind: "count" },
  { id: "autovacuum_time", fields: ["total_autovacuum_time"], kind: "milliseconds" },
]

export const INDEX_CUTS: readonly ActivityCut[] = [
  { id: "idx_scan", fields: ["idx_scan"], kind: "count" },
  { id: "idx_tup_read", fields: ["idx_tup_read"], kind: "count" },
  { id: "idx_blks_read", fields: ["idx_blks_read"], kind: "bytes", blockScaled: true },
]

export function activityPreview(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 240)
}
