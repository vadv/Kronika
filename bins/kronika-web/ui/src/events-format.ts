import type { findingMetric } from "./finding-presentation"
import type { Translate } from "./help"
import { compact, humanBytes, humanDuration, humanPercent, type Locale } from "./model"

const ERROR_CATEGORIES = [
  "events.category.lock", "events.category.constraint", "events.category.serialization",
  "events.category.timeout", "events.category.resource", "events.category.data_corruption",
  "events.category.system", "events.category.connection", "events.category.auth",
  "events.category.syntax", "events.category.other",
] as const

export function categoryLabel(category: number, t: Translate): string {
  const key = ERROR_CATEGORIES[category]
  return key === undefined ? String(category) : t(key)
}

export function formatMetric(value: number | null, unit: ReturnType<typeof findingMetric>["unit"], locale: Locale, t: Translate): string {
  if (value === null) return "—"
  if (unit === "percent") return humanPercent(value, locale)
  if (unit === "milliseconds") return humanDuration(value, locale)
  if (unit === "milliseconds_per_call") return humanDuration(value, locale, "milliseconds", t("unit.per_call"))
  if (unit === "bytes_per_second") return `${humanBytes(value, locale)}${t("unit.per_second")}`
  return compact(value, locale)
}
