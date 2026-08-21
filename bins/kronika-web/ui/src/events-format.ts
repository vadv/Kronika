import type { Cell } from "./api"
import type { findingMetric } from "./finding-presentation"
import type { Translate } from "./help"
import { asNumber, compact, humanBytes, humanDuration, humanPercent, identifier, rawText, type Locale } from "./model"

const ERROR_CATEGORIES = [
  "events.category.lock", "events.category.constraint", "events.category.serialization",
  "events.category.timeout", "events.category.resource", "events.category.data_corruption",
  "events.category.system", "events.category.connection", "events.category.auth",
  "events.category.syntax", "events.category.other",
] as const
const EVENT_PREFIX = "events."

export function categoryLabel(category: number, t: Translate): string {
  const key = ERROR_CATEGORIES[category]
  return key === undefined ? String(category) : t(key)
}

export function eventValue(finding: { readonly logicalName: string }, field: string, cell: Cell, locale: Locale, t: Translate): string {
  if (field === "category") {
    const category = asNumber(cell)
    return category === null ? "—" : categoryLabel(category, t)
  }
  const enumKey = enumValueKey(finding.logicalName, field, asNumber(cell))
  if (enumKey !== null) return t(enumKey)
  if (identityField(field)) return identifier(cell)
  const number = asNumber(cell)
  if (number !== null && field.endsWith("_bytes")) return humanBytes(number, locale)
  if (number !== null && field.endsWith("_kb")) return `${compact(number, locale)} KiB`
  if (number !== null && field.endsWith("_ms")) return humanDuration(number, locale)
  if (number !== null && field.endsWith("_mbs")) return `${compact(number, locale)} MB/s`
  if (number !== null) return compact(number, locale)
  return rawText(cell) ?? "—"
}

function enumValueKey(logicalName: string, field: string, number: number | null): string | null {
  if (number === null) return null
  const name = (values: readonly string[], prefix: string) => values[number] === undefined ? null : `${prefix}.${values[number]}`
  if (logicalName === "pg_log_errors" && field === "severity") return name(["error", "fatal", "panic", "warning", "log"], `${EVENT_PREFIX}severity`)
  if (logicalName === "pg_log_checkpoints" && field === "phase") return name(["started", "completed", "too_frequent"], `${EVENT_PREFIX}checkpoint`)
  if (logicalName === "pg_log_autovacuum" && field === "kind") return name(["vacuum", "analyze"], `${EVENT_PREFIX}autovacuum`)
  if (logicalName === "pg_log_lock_waits" && field === "kind") return name(["waiting", "acquired"], `${EVENT_PREFIX}lock_wait`)
  if (logicalName === "pg_log_lifecycle" && field === "kind") return name(["crash", "shutdown", "ready"], `${EVENT_PREFIX}lifecycle`)
  if (logicalName === "pgbouncer_events" && field === "level") return name(["fatal", "error", "warning", "log", "debug", "noise"], `${EVENT_PREFIX}pgbouncer`)
  return null
}

export function formatMetric(value: number | null, unit: ReturnType<typeof findingMetric>["unit"], locale: Locale, t: Translate): string {
  if (value === null) return "—"
  if (unit === "percent") return humanPercent(value, locale)
  if (unit === "milliseconds") return humanDuration(value, locale)
  if (unit === "milliseconds_per_call") return humanDuration(value, locale, "milliseconds", t("unit.per_call"))
  if (unit === "bytes_per_second") return `${humanBytes(value, locale)}${t("unit.per_second")}`
  return compact(value, locale)
}

function identityField(field: string): boolean {
  const name = field.toLowerCase()
  return name === "pid" || name === "oid" || name.endsWith("id") || name.endsWith("_id")
}
