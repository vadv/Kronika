import { useEffect, useMemo, useRef, useState } from "react"

import { Diamond, TriangleAlert } from "lucide-react"

import { acceptResponse, loadSeries, type DataRow, type Finding, type HourData } from "./api"
import { useDisplayTime } from "./display-time-context"
import { EventTierSection, entryChips, entryTitle, sectionLabel, tiersOf } from "./events-console"
import { categoryLabel } from "./events-format"
import { errorCategory, groupEvents, type EventEntry } from "./events-groups"
import { findingCategory, findingKey, findingOrder, findingSource } from "./finding-presentation"
import type { Translate } from "./help"
import { globMatcher } from "./glob"
import { shownMoment, type Locale } from "./model"
import { parseSearch } from "./search"
import { TableFilter } from "./table-filter"
import { Timeline } from "./timeline"

export type FindingResolution = "idle" | "loading" | "ready" | "unavailable"

// Threshold marks rendered before the list states how many were left out.
const MARK_ROWS = 120

export { categoryLabel, eventFieldLabel, eventValue, formatMetric } from "./events-format"

// The streams the console reads, with the columns each entry renders.
const EVENT_STREAMS: readonly { readonly section: string; readonly fields: readonly string[] }[] = [
  { section: "pg_log_errors", fields: ["severity", "category", "sqlstate", "pattern", "count", "sample", "detail", "hint", "context", "statement", "database", "username"] },
  { section: "pg_log_checkpoints", fields: ["phase", "reason", "seconds_apart", "buffers_written", "write_ms", "sync_ms", "total_ms", "distance_kb", "wal_added", "wal_removed", "wal_recycled", "sync_files"] },
  { section: "pg_log_autovacuum", fields: ["kind", "relation", "tuples_removed", "tuples_remaining", "tuples_dead_not_removable", "elapsed_ms"] },
  { section: "pg_log_slow_queries", fields: ["pattern", "sample", "count", "max_duration_ms", "total_duration_ms"] },
  { section: "pg_log_lock_waits", fields: ["kind", "pid", "lock_mode", "lock_target", "duration_ms", "holding_pids", "wait_queue", "statement"] },
  { section: "pg_log_lifecycle", fields: ["kind", "pid", "signal", "shutdown_mode", "message", "query_detail"] },
  { section: "pgbouncer_events", fields: ["level", "database", "username", "host", "text"] },
]

export function EventsView({
  cursor,
  data,
  hour,
  loading = false,
  locale,
  navigationTimestamps,
  onCursor,
  onFinding,
  onOpenChart,
  onPattern,
  onShowAll,
  onSelectedLane,
  revision,
  scope,
  pattern,
  selected,
  selectedLane,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly hour: number
  readonly loading?: boolean | undefined
  readonly locale: Locale
  readonly navigationTimestamps: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly onOpenChart: () => void
  readonly onPattern: (pattern: string) => void
  readonly onShowAll: () => void
  readonly onSelectedLane: (lane: string) => void
  readonly revision: number
  readonly scope: readonly Finding[] | null
  readonly pattern: string
  readonly selected: Finding | null
  readonly selectedLane: string
  readonly t: Translate
}) {
  const streams = useEventStreams(data, hour, revision)
  const entries = useMemo(() => streams.rows === null ? null : groupEvents(streams.rows, hour), [hour, streams.rows])
  const [expandedKey, setExpandedKey] = useState<string | null>(null)
  useEffect(() => setExpandedKey(null), [hour])
  const selectedEntry = useMemo(() => selected === null || entries === null ? null : entryOf(entries, selected), [entries, selected])
  useEffect(() => {
    if (selectedEntry !== null) setExpandedKey(selectedEntry.key)
  }, [selectedEntry])
  const list = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (expandedKey === null || selectedEntry?.key !== expandedKey) return
    list.current?.querySelector(`[data-entry-key=${JSON.stringify(expandedKey)}]`)?.scrollIntoView({ block: "nearest" })
  }, [expandedKey, selectedEntry])
  const parsedSearch = useMemo(() => parseSearch(pattern, "events"), [pattern])
  const scoped = useMemo(() => {
    if (entries === null) return null
    if (scope === null) return entries
    return entries.filter((entry) => scope.some((finding) => entryContains(entry, finding)))
  }, [entries, scope])
  const visible = useMemo(() => scoped === null ? null : scoped.filter((entry) => {
    if (!parsedSearch.ok || parsedSearch.query.canonical === "") return true
    const title = entryTitle(entry, t, locale)
    const fields: Readonly<Record<string, readonly string[]>> = {
      text: [title, entry.text ?? "", sectionLabel(entry.section, t), ...entryChips(entry, t)],
      kind: [entry.tier],
      source: [sectionLabel(entry.section, t), entry.section],
      category: errorCategory(entry.stat) === null ? [] : [categoryLabel(errorCategory(entry.stat) ?? 0, t)],
    }
    const clauses = parsedSearch.query.structured
      ? parsedSearch.query.clauses
      : [{ key: "text", value: parsedSearch.query.freeText ?? "" }]
    return clauses.every((clause) => fields[clause.key]?.some((candidate) => globMatcher(clause.value)?.(candidate) ?? true) === true)
  }), [locale, parsedSearch, scoped, t])
  const marks = useMemo(() => (scope ?? data.findings)
    .filter((finding) => finding.kind !== "event" && !finding.logicalName.startsWith("pg_log_"))
    .slice()
    .sort((left, right) => findingOrder(right, left)), [data.findings, scope])
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  const totalCount = visible?.reduce((sum, entry) => sum + entry.count, 0) ?? 0
  const busy = loading || streams.loading
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={onCursor} onFinding={onFinding} onOpenChart={onOpenChart} onSelectedLane={onSelectedLane} primaryLane="health" selectedLane={selectedLane} shownAt={shownAt} t={t} />
    <section className="mt-2" data-testid="events-console">
      <header className="flex min-h-[38px] items-center justify-between border-b border-line2 px-1.5 py-1 max-[760px]:flex-col max-[760px]:items-stretch max-[760px]:gap-[5px]">
        <span className="text-xs font-medium text-fg2">{t("events.console")}</span>
        <span className="text-xs tabular-nums text-fg3">{busy && visible !== null ? t("table.loading") : t("events.console.count", { groups: visible?.length ?? 0, count: totalCount })}</span>
        {scope !== null && <button className="min-h-[28px] cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s2 px-2.5 text-xs font-medium text-accent3 transition-colors hover:bg-s3" onClick={onShowAll} type="button">{t("events.show_all", { count: scope.length })}</button>}
      </header>
      <TableFilter kept={visible?.length ?? 0} onPattern={onPattern} pattern={pattern} surface="events" t={t} total={scoped?.length ?? 0} />
      <div className={`min-h-[390px] ${busy && visible !== null ? "animate-pulse opacity-55" : ""}`} data-loading={busy || undefined} ref={list}>
        {visible === null && <p className="table-empty flex items-baseline" role="status"><span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none mr-[7px] h-[11px] w-[11px] align-[-1px]" />{t("table.loading")}</p>}
        {visible !== null && visible.length === 0 && <div className="table-empty flex items-center gap-2.5">{pattern === "" && scope === null ? t("events.console.empty") : <>{t("filter.none")}<button className="cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4" data-testid="events-clear-filter" onClick={() => { onPattern(""); onShowAll() }} type="button">{t("filter.clear")}</button></>}</div>}
        {visible !== null && tiersOf(visible).map(([tier, tierEntries]) => <EventTierSection
          entries={tierEntries}
          expandedKey={expandedKey}
          hour={hour}
          key={tier}
          locale={locale}
          onCursor={onCursor}
          onToggle={(key) => setExpandedKey((current) => current === key ? null : key)}
          t={t}
          tier={tier}
        />)}
        {marks.length > 0 && <section data-testid="event-marks">
          <header className="flex items-center gap-2 border-b border-line2 bg-s2 px-[9px] py-[5px]">
            <Diamond aria-hidden="true" className="text-bad" size={13} />
            <span className="text-xs font-medium text-fg2">{t("events.marks")}</span>
            <span className="text-xs tabular-nums text-fg3">{marks.length}</span>
          </header>
          {marks.slice(0, MARK_ROWS).map((finding) => <MarkRow finding={finding} key={findingKey(finding)} onFinding={onFinding} t={t} />)}
          {marks.length > MARK_ROWS && <p className="px-[9px] py-1.5 text-xs text-fg3">{t("events.marks.more", { count: marks.length - MARK_ROWS })}</p>}
        </section>}
      </div>
    </section>
  </>
}

function MarkRow({ finding, onFinding, t }: {
  readonly finding: Finding
  readonly onFinding: (finding: Finding) => void
  readonly t: Translate
}) {
  const time = useDisplayTime()
  return <div className="border-b border-line" data-testid="event-mark">
    <button
      aria-label={`${findingCategory(finding, t)} · ${findingSource(finding, t)} · ${time.timestamp(finding.timestamp)}`}
      className="grid w-full cursor-pointer grid-cols-[18px_minmax(0,1fr)_auto] items-center gap-2 border-0 bg-s1 px-[9px] py-1.5 text-left text-fg2 hover:bg-s3"
      onClick={() => onFinding(finding)}
      type="button"
    >
      {finding.kind === "spike"
        ? <TriangleAlert aria-hidden="true" className="text-warn" size={15} />
        : <Diamond aria-hidden="true" className="text-bad" size={15} />}
      <span><strong className="block text-xs font-medium">{findingCategory(finding, t)}</strong><small className="mt-[3px] block text-xs text-fg3">{findingSource(finding, t)}</small></span>
      <time className="whitespace-nowrap font-mono text-xs tabular-nums text-fg3">{time.timestamp(finding.timestamp)}</time>
    </button>
  </div>
}

function entryOf(entries: readonly EventEntry[], finding: Finding): EventEntry | null {
  return entries.find((entry) => entryContains(entry, finding)) ?? null
}

function entryContains(entry: EventEntry, finding: Finding): boolean {
  return entry.rows.some((row) => row.segmentId === finding.segmentId
    && row.typeId === finding.typeId
    && row.ordinal === finding.rowOrdinal)
}

interface StreamState {
  readonly key: string
  readonly rows: Readonly<Record<string, readonly DataRow[]>> | null
  readonly loading: boolean
}

function useEventStreams(data: HourData, hour: number, revision: number): StreamState {
  const wanted = EVENT_STREAMS
    .filter((stream) => data.availableSections.includes(stream.section))
    .map((stream) => stream.section)
    .join(",")
  const key = `${hour}:${revision}:${wanted}`
  const [state, setState] = useState<StreamState>({ key: "", rows: null, loading: false })
  useEffect(() => {
    if (wanted === "") {
      setState({ key, rows: {}, loading: false })
      return
    }
    setState((current) => ({ ...current, key, loading: true }))
    const controller = new AbortController()
    const streams = EVENT_STREAMS.filter((stream) => wanted.split(",").includes(stream.section))
    acceptResponse(
      Promise.all(streams.map((stream) => loadSeries(hour, stream.section, {}, stream.fields, controller.signal).catch(() => []))),
      controller.signal,
      (loaded) => setState({
        key,
        rows: Object.fromEntries(streams.map((stream, index) => [stream.section, loaded[index] ?? []])),
        loading: false,
      }),
      () => setState((current) => ({ ...current, loading: false })),
    )
    return () => controller.abort()
  }, [hour, key, revision, wanted])
  return state
}
