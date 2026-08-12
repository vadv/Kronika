import type { Cell } from "./api"

export type ValueTone = "good" | "warning" | "critical" | "inactive"

/** Small display rules for values with an operator-useful interpretation.
 * Workload volume stays neutral: a high rate is not bad by itself. */
export function semanticValueTone(field: string, cell: Cell, rate = false): ValueTone | null {
  const number = numericCell(cell)
  if (number === null) return null
  if (rate && number === 0) return "inactive"

  switch (field) {
    case "mean_exec_ms_per_call":
    case "mean_exec_time_ms":
    case "query_duration_ms":
      return number >= 5_000 ? "critical" : null
    case "hit_pct":
      if (number < 90) return "critical"
      return number < 99 ? "warning" : "good"
    case "cv":
      if (number < 1) return "good"
      return number < 3 ? "warning" : "critical"
    case "plan_time_pct":
      if (number < 50) return "good"
      return number < 80 ? "warning" : "critical"
    default:
      return null
  }
}

function numericCell(cell: Cell): number | null {
  if (typeof cell === "number") return Number.isFinite(cell) ? cell : null
  if (typeof cell !== "string" || cell.trim() === "") return null
  const number = Number(cell)
  return Number.isFinite(number) ? number : null
}
