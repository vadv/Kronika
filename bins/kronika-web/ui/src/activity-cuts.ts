import type { TopActivityMetric } from "./heatmap"

export interface ActivityCut {
  readonly id: TopActivityMetric
  readonly fields: readonly string[]
  readonly class: "cumulative" | "gauge"
  readonly kind: "milliseconds" | "seconds" | "microseconds" | "nanoseconds" | "count" | "bytes"
  readonly scaleBy?: "block_size" | "clock_ticks" | "kib"
}

export const STATEMENT_CUTS: readonly ActivityCut[] = [
  { id: "exec_time", fields: ["total_exec_time"], class: "cumulative", kind: "milliseconds" },
  { id: "calls", fields: ["calls"], class: "cumulative", kind: "count" },
  { id: "rows", fields: ["rows"], class: "cumulative", kind: "count" },
  { id: "shared_read", fields: ["shared_blks_read"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
  { id: "shared_dirtied", fields: ["shared_blks_dirtied"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
  { id: "temp_written", fields: ["temp_blks_written"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
  { id: "wal_bytes", fields: ["wal_bytes"], class: "cumulative", kind: "bytes" },
]

// pg_store_plans has no WAL or planning counters shared across its forks;
// execution time is total_time in every fork.
export const PLAN_CUTS: readonly ActivityCut[] = [
  { id: "exec_time", fields: ["total_time"], class: "cumulative", kind: "milliseconds" },
  { id: "calls", fields: ["calls"], class: "cumulative", kind: "count" },
  { id: "rows", fields: ["rows"], class: "cumulative", kind: "count" },
  { id: "shared_read", fields: ["shared_blks_read"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
  { id: "temp_written", fields: ["temp_blks_written"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
]

export const TABLE_CUTS: readonly ActivityCut[] = [
  { id: "writes", fields: ["n_tup_ins", "n_tup_upd", "n_tup_del"], class: "cumulative", kind: "count" },
  { id: "seq_read", fields: ["seq_tup_read"], class: "cumulative", kind: "count" },
  { id: "heap_read", fields: ["heap_blks_read"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
  { id: "dead_tuples", fields: ["n_dead_tup"], class: "gauge", kind: "count" },
  { id: "autovacuum_time", fields: ["total_autovacuum_time"], class: "cumulative", kind: "milliseconds" },
]

export const INDEX_CUTS: readonly ActivityCut[] = [
  { id: "idx_scan", fields: ["idx_scan"], class: "cumulative", kind: "count" },
  { id: "idx_tup_read", fields: ["idx_tup_read"], class: "cumulative", kind: "count" },
  { id: "idx_blks_read", fields: ["idx_blks_read"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
]

// utime and stime are jiffies; the recorded clock rate turns their sum into
// CPU seconds, so one second per second reads as one busy core.
export const PROCESS_CUTS: readonly ActivityCut[] = [
  { id: "cpu", fields: ["utime", "stime"], class: "cumulative", kind: "seconds", scaleBy: "clock_ticks" },
  { id: "rss", fields: ["rmem_kb"], class: "gauge", kind: "bytes", scaleBy: "kib" },
  { id: "io_read", fields: ["read_bytes"], class: "cumulative", kind: "bytes" },
  { id: "io_write", fields: ["write_bytes"], class: "cumulative", kind: "bytes" },
  { id: "majflt", fields: ["majflt"], class: "cumulative", kind: "count" },
  { id: "run_delay", fields: ["rundelay_ns"], class: "cumulative", kind: "nanoseconds" },
]

export const DATABASE_CUTS: readonly ActivityCut[] = [
  { id: "commits", fields: ["xact_commit"], class: "cumulative", kind: "count" },
  { id: "rollbacks", fields: ["xact_rollback"], class: "cumulative", kind: "count" },
  { id: "db_read", fields: ["blks_read"], class: "cumulative", kind: "bytes", scaleBy: "block_size" },
  { id: "temp_bytes", fields: ["temp_bytes"], class: "cumulative", kind: "bytes" },
  { id: "deadlocks", fields: ["deadlocks"], class: "cumulative", kind: "count" },
]

export const CGROUP_CPU_CUTS: readonly ActivityCut[] = [
  { id: "cg_cpu", fields: ["usage_usec"], class: "cumulative", kind: "microseconds" },
  { id: "cg_throttled", fields: ["throttled_usec"], class: "cumulative", kind: "microseconds" },
]

export const CGROUP_IO_CUTS: readonly ActivityCut[] = [
  { id: "cg_read", fields: ["rbytes"], class: "cumulative", kind: "bytes" },
  { id: "cg_write", fields: ["wbytes"], class: "cumulative", kind: "bytes" },
  { id: "cg_rios", fields: ["rios"], class: "cumulative", kind: "count" },
  { id: "cg_wios", fields: ["wios"], class: "cumulative", kind: "count" },
]

export function activityPreview(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 240)
}
