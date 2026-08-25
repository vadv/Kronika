import type { HourData } from "./api"
import type { TableOrder } from "./entity-table"
import { postgresqlActivityRequest } from "./postgres-activity-query"
import { parsePostgresqlActivityPage, postgresqlActivityHourData } from "./postgres-activity-result"
import { apiFetch } from "./session"

export async function loadPostgresqlActivity(
  at: number,
  search: string,
  signal: AbortSignal,
  order?: TableOrder | undefined,
  cursor?: string | undefined,
): Promise<HourData> {
  const request = postgresqlActivityRequest(at, search, order, cursor)
  const stored = await requestActivityJson(request.path, signal)
  const page = parsePostgresqlActivityPage(stored, at)
  return postgresqlActivityHourData(page, request.sort, request.direction, request.pageSize)
}

async function requestActivityJson(path: string, signal: AbortSignal): Promise<unknown> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    let response: Response
    try {
      response = await apiFetch(path, { headers: { Accept: "application/json" }, signal })
    } catch (error) {
      signal.throwIfAborted()
      if (attempt === 0 && error instanceof TypeError) continue
      throw error
    }
    if (!response.ok) throw new Error(`HTTP ${response.status} for ${path}`)
    try {
      return await response.json()
    } catch (error) {
      signal.throwIfAborted()
      if (attempt === 0 && error instanceof TypeError) continue
      throw error
    }
  }
  throw new Error(`HTTP read failed for ${path}`)
}
