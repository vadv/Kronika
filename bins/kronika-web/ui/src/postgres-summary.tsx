import type { Cell, DataRow } from "./api"
import type { Translate } from "./help"
import { asNumber, humanBytes, humanDuration, humanPercent, measure, value, type Locale } from "./model"
import { parseSearch, rowMatchesSearch } from "./search"

export type PostgresSummarySection =
  | "pg_stat_database"
  | "pg_stat_statements"
  | "pg_stat_user_indexes"
  | "pg_stat_user_tables"
  | "pg_store_plans"

type FactKind = "bytes" | "count" | "duration" | "duration_rate" | "percent" | "rate"

interface Fact {
  readonly field: string
  readonly kind: FactKind
  readonly label: string
}

const DENSE_FACTS: readonly Fact[] = [
  { field: "call_rate", kind: "rate", label: "pg.field.calls_per_second.label" },
  { field: "exec_time_rate", kind: "duration_rate", label: "pg.field.execution_ms_per_second.label" },
  { field: "mean_exec", kind: "duration", label: "pg.field.mean_exec_ms_per_call.label" },
  { field: "row_rate", kind: "rate", label: "pg.field.rows_per_second.label" },
]

const FACTS: Readonly<Record<PostgresSummarySection, readonly Fact[]>> = {
  pg_stat_database: [
    { field: "backends", kind: "count", label: "pg.field.numbackends.label" },
    { field: "transactions", kind: "rate", label: "pg.vitals.row.tps" },
    { field: "buffer_hit_pct", kind: "percent", label: "pg.vitals.row.cache_hit" },
    { field: "xid_age", kind: "count", label: "pg.vitals.row.xid_age" },
  ],
  pg_stat_statements: DENSE_FACTS,
  pg_store_plans: DENSE_FACTS,
  pg_stat_user_tables: [
    { field: "tuple_throughput", kind: "rate", label: "pg.field.tuple_throughput.label" },
    { field: "dml_total", kind: "rate", label: "pg.field.dml_total.label" },
    { field: "displayed_storage_bytes", kind: "bytes", label: "pg.field.displayed_storage_bytes.label" },
    { field: "buffer_hit_pct", kind: "percent", label: "pg.field.buffer_hit_pct.label" },
  ],
  pg_stat_user_indexes: [
    { field: "idx_scan", kind: "rate", label: "pg.field.idx_scan.label" },
    { field: "no_scan_count", kind: "count", label: "pg.field.no_scan_count.label" },
    { field: "main_fork_bytes", kind: "bytes", label: "pg.field.main_fork_bytes.label" },
    { field: "buffer_hit_pct", kind: "percent", label: "pg.field.buffer_hit_pct.label" },
  ],
}

export function PostgresSummary({ locale, section, summary, t }: {
  readonly locale: Locale
  readonly section: PostgresSummarySection
  readonly summary: Readonly<Record<string, Cell>> | undefined
  readonly t: Translate
}) {
  return <section aria-label={t("pg.summary.title")} className="process-summary-inline flex min-w-0 flex-1 items-center overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden min-[521px]:max-[760px]:flex-wrap min-[521px]:max-[760px]:overflow-visible">
    {FACTS[section].map((fact) => <div className="flex h-[25px] flex-none items-center space-x-1.5 px-2" data-summary-fact={fact.field} key={fact.field}>
      <span className="whitespace-nowrap text-xs font-medium text-fg3">{t(fact.label)}</span>
      <strong className="flex-none font-mono text-xs tabular-nums text-fg">{format(summary?.[fact.field] ?? null, fact.kind, locale, t)}</strong>
    </div>)}
  </section>
}

export function databaseSummary(rows: readonly DataRow[], pattern: string): Readonly<Record<string, Cell>> {
  const parsed = parseSearch(pattern, "pg_stat_database")
  const selected = rows.filter((row) => asNumber(value(row, "datid")) !== 0
    && (!parsed.ok || rowMatchesSearch(row, parsed.query, "pg_stat_database")))
  const hits = total(selected, ["blks_hit"])
  const reads = total(selected, ["blks_read"])
  return {
    backends: total(selected, ["numbackends"]),
    transactions: total(selected, ["xact_commit", "xact_rollback"]),
    buffer_hit_pct: hits === null || reads === null || hits + reads <= 0 ? null : 100 * hits / (hits + reads),
    xid_age: maximum(selected, "frozen_xid_age"),
  }
}

function total(rows: readonly DataRow[], fields: readonly string[]): number | null {
  if (rows.length === 0) return null
  let result = 0
  for (const row of rows) for (const field of fields) {
    const stored = asNumber(value(row, field))
    if (stored === null) return null
    result += stored
  }
  return Number.isFinite(result) ? result : null
}

function maximum(rows: readonly DataRow[], field: string): number | null {
  let result: number | null = null
  for (const row of rows) {
    const stored = asNumber(value(row, field))
    if (stored === null) return null
    result = result === null ? stored : Math.max(result, stored)
  }
  return result
}

function format(cell: Cell, kind: FactKind, locale: Locale, t: Translate): string {
  if (kind === "bytes") return humanBytes(cell, locale)
  if (kind === "duration") return humanDuration(cell, locale)
  if (kind === "duration_rate") return humanDuration(cell, locale, "milliseconds", t("unit.per_second"))
  if (kind === "percent") return humanPercent(cell, locale)
  return measure(cell, locale, kind === "rate" ? t("unit.per_second") : "")
}
