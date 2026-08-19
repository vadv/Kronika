import type { Translate } from "./help"
import type { SearchSurface } from "./search"

export type SearchRequestPhase = "idle" | "pending" | "error"

export interface SearchRequestState {
  readonly phase: SearchRequestPhase
  readonly retained: boolean
  readonly surface: SearchSurface | null
}

export const IDLE_SEARCH_REQUEST: SearchRequestState = {
  phase: "idle",
  retained: false,
  surface: null,
}

export function beginSearchRequest(surface: SearchSurface, retained: boolean): SearchRequestState {
  return { phase: "pending", retained, surface }
}

export function searchRequestForSurface(
  current: SearchRequestState,
  surface: SearchSurface | null,
): SearchRequestState {
  return surface !== null && current.surface === surface ? current : IDLE_SEARCH_REQUEST
}

export function SearchRequestMessage({ request, t }: {
  readonly request: SearchRequestState
  readonly t: Translate
}) {
  if (request.phase === "idle") return null
  const pending = request.phase === "pending"
  const key = `filter.${pending ? "searching" : "search_failed"}${request.retained ? "_retained" : ""}`
  return <span aria-live={pending ? "polite" : undefined} className={`flex min-w-0 flex-1 items-center gap-2${pending ? "" : " text-bad"}`} role={pending ? "status" : "alert"}>
    {pending && <progress style={{ width: 44 }} />}
    <span className="min-w-0 overflow-hidden text-ellipsis">{t(key)}</span>
  </span>
}
