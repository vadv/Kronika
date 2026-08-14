import type { HourData } from "./api"

export function hasPostgresTelemetry(data: Pick<HourData, "activities" | "availableSections" | "postgresqlConfigured">): boolean {
  return data.postgresqlConfigured === true || data.activities.length !== 0
    || data.availableSections.some((name) => name.startsWith("pg_") && !name.startsWith("pg_log_"))
}
