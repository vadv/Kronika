import { useEffect, useMemo, useState } from "react"

import { loadRelatedStatementTextRows, type DataRow, type SegmentBound } from "./api"
import { rawText, value } from "./model"
import { statementsForPlan } from "./statement-navigation"

export interface RecordedQueryText {
  readonly database: string | null
  readonly occurrences: number
  readonly queryId: string
  readonly role: string | null
  readonly text: string
  readonly timestamp: number
  readonly topLevel: boolean | null
}

export type PlanQueryTextState =
  | { readonly status: "loading"; readonly texts: readonly RecordedQueryText[] }
  | { readonly status: "ready"; readonly texts: readonly RecordedQueryText[] }
  | { readonly status: "unavailable"; readonly texts: readonly RecordedQueryText[] }
  | { readonly status: "no_bridge"; readonly texts: readonly RecordedQueryText[] }
  | { readonly status: "error"; readonly texts: readonly RecordedQueryText[] }

type LoadedPlanQueryTextState = PlanQueryTextState & { readonly key: string }

export function recordedQueryTexts(rows: readonly DataRow[]): readonly RecordedQueryText[] {
  const distinct = new Map<string, RecordedQueryText>()
  for (const row of rows) {
    const text = rawText(value(row, "query"))
    const queryId = rawText(value(row, "queryid"))
    if (text === null || text.length === 0 || queryId === null) continue
    const earlier = distinct.get(text)
    if (earlier !== undefined) {
      distinct.set(text, { ...earlier, occurrences: earlier.occurrences + 1 })
      continue
    }
    const topLevel = value(row, "toplevel")
    distinct.set(text, {
      database: rawText(value(row, "datname")),
      occurrences: 1,
      queryId,
      role: rawText(value(row, "usename")),
      text,
      timestamp: row.timestamp,
      topLevel: typeof topLevel === "boolean" ? topLevel : null,
    })
  }
  return [...distinct.values()]
}

export function usePlanQueryTexts(
  row: DataRow,
  cursor: number,
  segments: readonly SegmentBound[],
  revision: number,
): PlanQueryTextState & { readonly retry: () => void } {
  const target = useMemo(() => statementsForPlan(row), [row])
  const targetKey = target === null
    ? `no-bridge:${row.typeId}:${row.segmentId}:${row.ordinal}:${row.timestamp}`
    : JSON.stringify([row.typeId, row.segmentId, row.ordinal, cursor, target.expression, revision, segments])
  const [retryRevision, setRetryRevision] = useState(0)
  const requestKey = `${targetKey}:${retryRevision}`
  const [loaded, setLoaded] = useState<LoadedPlanQueryTextState>(() => target === null
    ? { key: requestKey, status: "no_bridge", texts: [] }
    : { key: requestKey, status: "loading", texts: [] })

  useEffect(() => {
    if (target === null) {
      setLoaded({ key: requestKey, status: "no_bridge", texts: [] })
      return
    }
    const controller = new AbortController()
    void loadRelatedStatementTextRows(segments, cursor, target.expression, controller.signal)
      .then((rows) => {
        if (controller.signal.aborted) return
        const texts = recordedQueryTexts(rows)
        setLoaded({
          key: requestKey,
          status: texts.length === 0 ? "unavailable" : "ready",
          texts,
        })
      })
      .catch(() => {
        if (!controller.signal.aborted) setLoaded({ key: requestKey, status: "error", texts: [] })
      })
    return () => controller.abort()
  }, [cursor, requestKey, segments, target])

  const current = loaded.key === requestKey
    ? loaded
    : target === null
      ? { key: requestKey, status: "no_bridge" as const, texts: [] }
      : { key: requestKey, status: "loading" as const, texts: [] }
  return useMemo(() => ({
    status: current.status,
    texts: current.texts,
    retry: () => setRetryRevision((value) => value + 1),
  }), [current.status, current.texts])
}
