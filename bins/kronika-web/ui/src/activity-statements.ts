import type { DataRow } from "./api"
import { rawText, value } from "./model"

// The curated statement cuts. Each answers one forensic question; the menu
// stays short on purpose. Block counters are stored in blocks and scale to
// bytes with the recorded block size; without one they stay block counts.
// Field names are physical: layouts that lack one simply yield no stream.
export interface StatementCut {
  readonly id: string
  readonly field: string
  readonly kind: "milliseconds" | "count" | "bytes"
  readonly blockScaled?: boolean
}

export const STATEMENT_CUTS: readonly StatementCut[] = [
  { id: "exec_time", field: "total_exec_time", kind: "milliseconds" },
  { id: "calls", field: "calls", kind: "count" },
  { id: "rows", field: "rows", kind: "count" },
  { id: "shared_read", field: "shared_blks_read", kind: "bytes", blockScaled: true },
  { id: "shared_dirtied", field: "shared_blks_dirtied", kind: "bytes", blockScaled: true },
  { id: "temp_written", field: "temp_blks_written", kind: "bytes", blockScaled: true },
  { id: "wal_bytes", field: "wal_bytes", kind: "bytes" },
]

export const STATEMENT_DEFAULT_CUT = "exec_time"

// Query text is the one heavy label: it never travels with the ranking
// response. The loaded table page answers most of the ranked entities; the
// rest are fetched one bounded text row at a time, by queryid, after ranking.
export function statementTextsByQueryId(tableRows: readonly DataRow[]): ReadonlyMap<string, string> {
  const texts = new Map<string, string>()
  for (const row of tableRows) {
    const queryId = rawText(value(row, "queryid"))
    const text = rawText(value(row, "query"))
    if (queryId === null || text === null || text === "" || texts.has(queryId)) continue
    texts.set(queryId, text)
  }
  return texts
}

export function statementPreview(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 240)
}
