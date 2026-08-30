import { useEffect, useMemo, useRef, useState } from "react"

import { Diamond, TriangleAlert } from "lucide-react"

import { acceptResponse, loadEventGroups, type Finding, type HourData } from "./api"
import { useDisplayTime } from "./display-time-context"
import { EventTierSection, SECTION_ICONS, entryChips, entryTitle, sectionLabel, tiersOf } from "./events-console"
import { categoryLabel } from "./events-format"
import { MINUTE_COLUMNS, type EventEntry } from "./events-groups"
import { findingKey, findingMetric, findingOrder, findingSource, type FindingMetric } from "./finding-presentation"
import type { Translate } from "./help"
import { globMatcher } from "./glob"
import { compact, shownMoment, type Locale } from "./model"
import { evaluateExpr, parseSearch } from "./search"
import { TableFilter } from "./table-filter"
import { Timeline } from "./timeline"

const MARK_ROWS = 120
// The digest tile that selects the threshold band instead of a log stream.
const MARKS_TILE = "marks"

const EVENT_SOURCES = [
  "pg_log_errors", "pg_log_checkpoints", "pg_log_autovacuum", "pg_log_slow_queries",
  "pg_log_lock_waits", "pg_log_lifecycle", "pgbouncer_events",
] as const

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
  onReady,
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
  readonly onReady: () => void
  readonly onShowAll: () => void
  readonly onSelectedLane: (lane: string) => void
  readonly revision: number
  readonly scope: readonly Finding[] | null
  readonly pattern: string
  readonly selected: Finding | null
  readonly selectedLane: string
  readonly t: Translate
}) {
  const events = useEventGroups(data, hour, revision, onReady)
  const entries = events.rows
  const [expandedKey, setExpandedKey] = useState<string | null>(null)
  useEffect(() => setExpandedKey(null), [hour])
  const selectedEntry = useMemo(() => selected === null || entries === null ? null : entryOf(entries, selected), [entries, selected])
  // A refetch must not reopen an entry that the user collapsed.
  const expandedFor = useRef<string | null>(null)
  useEffect(() => {
    if (selected === null || selectedEntry === null) return
    const key = findingKey(selected)
    if (expandedFor.current === key) return
    expandedFor.current = key
    setExpandedKey(selectedEntry.key)
  }, [selected, selectedEntry])
  const list = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (expandedKey === null || selectedEntry?.key !== expandedKey) return
    for (const node of list.current?.querySelectorAll("[data-entry-key]") ?? []) {
      if (node.getAttribute("data-entry-key") === expandedKey) {
        node.scrollIntoView({ block: "nearest" })
        return
      }
    }
  }, [expandedKey, selectedEntry])
  const parsedSearch = useMemo(() => parseSearch(pattern, "events"), [pattern])
  const [digest, setDigest] = useState<string | null>(null)
  useEffect(() => setDigest(null), [hour])
  const scoped = useMemo(() => {
    if (entries === null) return null
    if (scope === null) return entries
    return entries.filter((entry) => scope.some((finding) => entryContains(entry, finding)))
  }, [entries, scope])
  const chosen = useMemo(() => {
    if (scoped === null || digest === null) return scoped
    return scoped.filter((entry) => digest === "critical" ? entry.tier === "critical" : entry.section === digest)
  }, [digest, scoped])
  const visible = useMemo(() => chosen === null ? null : chosen.filter((entry) => {
    if (!parsedSearch.ok || parsedSearch.query.canonical === "") return true
    const title = entryTitle(entry, t, locale)
    const category = "category" in entry.stat ? entry.stat.category : null
    const fields: Readonly<Record<string, readonly string[]>> = {
      text: [title, sectionLabel(entry.section, t), ...entryChips(entry, t).map((chip) => chip.label)],
      kind: [entry.tier],
      source: [sectionLabel(entry.section, t), entry.section],
      category: category === null ? [] : [categoryLabel(category, t)],
    }
    const matches = (clause: { readonly key: string; readonly value: string }) =>
      fields[clause.key]?.some((candidate) => globMatcher(clause.value)?.(candidate) ?? true) === true
    if (!parsedSearch.query.structured || parsedSearch.query.expr === null) {
      return matches({ key: "text", value: parsedSearch.query.freeText ?? "" })
    }
    return evaluateExpr(parsedSearch.query.expr, (clause) => matches({ key: clause.key, value: clause.value }))
  }), [chosen, locale, parsedSearch, t])
  const marks = useMemo(() => (scope ?? data.findings)
    .filter((finding) => finding.kind !== "event" && !finding.logicalName.startsWith("pg_log_"))
    .slice()
    .sort((left, right) => findingOrder(right, left)), [data.findings, scope])
  const markGroups = useMemo(() => groupMarks(marks, hour, t), [hour, marks, t])
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  const totalCount = visible?.reduce((sum, entry) => sum + entry.count, 0) ?? 0
  const busy = loading || events.loading
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={onCursor} onFinding={onFinding} onOpenChart={onOpenChart} onSelectedLane={onSelectedLane} primaryLane="health" selectedLane={selectedLane} shownAt={shownAt} t={t} />
    {markGroups.length > 0 && (digest === null || digest === MARKS_TILE)
      ? <section className="mt-2" data-testid="event-marks">
        <header className="flex min-h-[38px] items-center justify-between border-b border-line2 px-1.5 py-1">
          <span className="flex items-center gap-2">
            <Diamond aria-hidden="true" className="text-bad" size={13} />
            <span className="text-xs font-medium text-fg2">{t("events.marks")}</span>
          </span>
          <span className="text-xs tabular-nums text-fg3">{t("events.console.count", { groups: markGroups.length, count: marks.length })}</span>
        </header>
        {markGroups.map((group) => <MarkGroupRow
          expanded={expandedKey === group.key}
          group={group}
          hour={hour}
          key={group.key}
          locale={locale}
          onCursor={onCursor}
          onFinding={onFinding}
          onToggle={() => setExpandedKey((current) => current === group.key ? null : group.key)}
          t={t}
        />)}
      </section>
      : null}
    <section className="mt-2" data-testid="events-console">
      <header className="flex min-h-[38px] items-center justify-between border-b border-line2 px-1.5 py-1 max-[760px]:flex-col max-[760px]:items-stretch max-[760px]:gap-[5px]">
        <span className="text-xs font-medium text-fg2">{t("events.console")}</span>
        <span className="text-xs tabular-nums text-fg3">{busy && visible !== null ? t("table.loading") : t("events.console.count", { groups: visible?.length ?? 0, count: totalCount })}</span>
        {scope !== null && <button className="min-h-[28px] cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s2 px-2.5 text-xs font-medium text-accent3 transition-colors hover:bg-s3" onClick={onShowAll} type="button">{t("events.show_all", { count: scope.length })}</button>}
      </header>
      {scoped !== null && scoped.length > 0 && <EventsDigest active={digest} entries={scoped} locale={locale} marks={markGroups} onChoose={(key) => setDigest((current) => current === key ? null : key)} t={t} />}
      <TableFilter kept={visible?.length ?? 0} onPattern={onPattern} pattern={pattern} surface="events" t={t} total={chosen?.length ?? 0} />
      <div className={`${digest === MARKS_TILE ? "" : "max-[520px]:min-h-0 min-h-[390px] "}${busy && visible !== null ? "animate-pulse opacity-55" : ""}`} data-loading={busy || undefined} ref={list}>
        {digest === MARKS_TILE && <p className="table-empty" role="status">{t("events.marks.only")}</p>}
        {digest !== MARKS_TILE && <>
        {visible === null && events.failed && <p className="table-empty" role="status">{t("events.console.error")}</p>}
        {visible === null && !events.failed && <p className="table-empty flex items-baseline" role="status"><span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none mr-[7px] h-[11px] w-[11px] align-[-1px]" />{t("table.loading")}</p>}
        {visible !== null && visible.length === 0 && <div className="table-empty flex items-center gap-2.5">{pattern === "" && scope === null ? t("events.console.empty") : <>{t("filter.none")}<button className="cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4" data-testid="events-clear-filter" onClick={() => { onPattern(""); onShowAll() }} type="button">{t("filter.clear")}</button></>}</div>}
        {visible !== null && tiersOf(visible).map(([tier, tierEntries]) => <EventTierSection
          entries={tierEntries}
          expandedKey={expandedKey}
          filtered={digest !== null || pattern !== "" || scope !== null}
          hour={hour}
          key={tier}
          locale={locale}
          onCursor={onCursor}
          onToggle={(key) => setExpandedKey((current) => current === key ? null : key)}
          t={t}
          tier={tier}
        />)}
        </>}
      </div>
    </section>
  </>
}

interface DigestTile {
  readonly key: string
  readonly label: string
  readonly count: number
  readonly minutes: readonly number[]
  readonly bad: boolean
  readonly section: string | null
}

function EventsDigest({ active, entries, locale, marks, onChoose, t }: {
  readonly active: string | null
  readonly entries: readonly EventEntry[]
  readonly locale: Locale
  readonly marks: readonly MarkGroup[]
  readonly onChoose: (key: string) => void
  readonly t: Translate
}) {
  const tiles = useMemo(() => {
    const bySection = new Map<string, { count: number; minutes: number[] }>()
    let criticalCount = 0
    const criticalMinutes = Array.from({ length: MINUTE_COLUMNS }, () => 0)
    for (const entry of entries) {
      const tile = bySection.get(entry.section) ?? { count: 0, minutes: Array.from({ length: MINUTE_COLUMNS }, () => 0) }
      tile.count += entry.count
      entry.minutes.forEach((count, minute) => { tile.minutes[minute] = (tile.minutes[minute] ?? 0) + count })
      bySection.set(entry.section, tile)
      if (entry.tier === "critical") {
        criticalCount += entry.count
        entry.minutes.forEach((count, minute) => { criticalMinutes[minute] = (criticalMinutes[minute] ?? 0) + count })
      }
    }
    const ordered = ["pg_log_errors", "pg_log_slow_queries", "pg_log_lock_waits", "pg_log_checkpoints", "pg_log_autovacuum", "pg_log_lifecycle", "pgbouncer_events"]
    return [
      ...(criticalCount === 0 ? [] : [{ key: "critical", label: t("events.tier.critical"), count: criticalCount, minutes: criticalMinutes, bad: true, section: null } satisfies DigestTile]),
      ...ordered.flatMap((section) => {
        const tile = bySection.get(section)
        return tile === undefined ? [] : [{ key: section, label: sectionLabel(section, t), count: tile.count, minutes: tile.minutes, bad: false, section } satisfies DigestTile]
      }),
    ]
  }, [entries, t])
  const markTile = useMemo(() => {
    if (marks.length === 0) return null
    const minutes = Array.from({ length: MINUTE_COLUMNS }, () => 0)
    let count = 0
    for (const group of marks) {
      count += group.findings.length
      group.minutes.forEach((crossings, minute) => { minutes[minute] = (minutes[minute] ?? 0) + crossings })
    }
    return { key: MARKS_TILE, label: t("events.marks"), count, minutes, bad: true, section: null } satisfies DigestTile
  }, [marks, t])
  const shown = markTile === null ? tiles : [markTile, ...tiles]
  if (shown.length < 2) return null
  return <div aria-label={t("events.digest")} className="flex flex-wrap gap-1.5 border-b border-line2 px-1.5 py-2" data-testid="events-digest" role="group">
    {shown.map((tile) => {
      const Icon = tile.key === MARKS_TILE ? Diamond : tile.section === null ? TriangleAlert : SECTION_ICONS[tile.section] ?? TriangleAlert
      const pressed = active === tile.key
      return <button
        aria-pressed={pressed}
        className={`flex cursor-pointer items-center gap-2 rounded-[var(--radius-sm)] border px-2 py-1.5 text-left transition-colors ${pressed ? "border-accent3 bg-s3" : "border-line2 bg-s1 hover:bg-s2"}`}
        key={tile.key}
        onClick={() => onChoose(tile.key)}
        type="button"
      >
        <Icon aria-hidden="true" className={tile.bad ? "text-bad" : "text-fg3"} size={13} />
        <span className="text-xs text-fg3">{tile.label}</span>
        <strong className="font-mono text-[13px] font-semibold tabular-nums text-fg">{compact(tile.count, locale)}</strong>
        <DigestStrip bad={tile.bad} minutes={tile.minutes} />
      </button>
    })}
  </div>
}

function DigestStrip({ bad, minutes }: { readonly bad: boolean; readonly minutes: readonly number[] }) {
  const peak = Math.max(...minutes, 1)
  return <svg aria-hidden="true" className="block h-[14px] w-[44px] flex-none" preserveAspectRatio="none" viewBox={`0 0 ${minutes.length} 14`}>
    {minutes.map((count, minute) => count === 0 ? null : <rect
      className={bad ? "fill-bad" : "fill-accent3"}
      height={Math.max(1.5, (count / peak) * 13)}
      key={minute}
      width="0.8"
      x={minute + 0.1}
      y={14 - Math.max(1.5, (count / peak) * 13)}
    />)}
  </svg>
}

// A threshold crossing repeats for as long as the value stays across, so one
// hour of a busy host is dozens of identical rows. They group the way log
// events do: one entry per crossed metric, with the hour's shape under it.
interface MarkGroup {
  readonly key: string
  readonly kind: Finding["kind"]
  readonly metric: FindingMetric
  readonly source: string
  readonly findings: readonly Finding[]
  readonly minutes: readonly number[]
}

export function groupMarks(marks: readonly Finding[], hour: number, t: Translate): readonly MarkGroup[] {
  const groups = new Map<string, Finding[]>()
  for (const finding of marks) {
    const key = `${finding.kind}\u{1f}${finding.logicalName}\u{1f}${finding.typeId}\u{1f}${finding.fieldOrdinal}`
    const members = groups.get(key)
    if (members === undefined) groups.set(key, [finding])
    else members.push(finding)
  }
  return [...groups.entries()].flatMap(([key, findings]) => {
    const [first] = findings
    if (first === undefined) return []
    const minutes = Array.from({ length: MINUTE_COLUMNS }, () => 0)
    for (const finding of findings) {
      const minute = Math.floor((finding.timestamp - hour) / 60_000_000)
      if (minute >= 0 && minute < MINUTE_COLUMNS) minutes[minute] = (minutes[minute] ?? 0) + 1
    }
    return [{
      key: `mark:${key}`,
      kind: first.kind,
      metric: findingMetric(first, t),
      source: findingSource(first, t),
      findings: findings.slice().sort((left, right) => findingOrder(right, left)),
      minutes,
    }]
  }).sort((left, right) => right.findings.length - left.findings.length || left.key.localeCompare(right.key))
}

export function MarkGroupRow({ expanded, group, hour, locale, onCursor, onFinding, onToggle, t }: {
  readonly expanded: boolean
  readonly group: MarkGroup
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly onToggle: () => void
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const [shown, setShown] = useState(MARK_ROWS)
  const first = group.findings[group.findings.length - 1]
  const last = group.findings[0]
  const moments = first === undefined || last === undefined
    ? ""
    : first.timestamp === last.timestamp
      ? time.timestamp(first.timestamp)
      : `${time.timestamp(first.timestamp)}–${time.timestamp(last.timestamp)}`
  const Icon = group.kind === "spike" ? TriangleAlert : Diamond
  return <div className="border-b border-line" data-testid="event-mark">
    <button
      aria-expanded={expanded}
      className="grid w-full cursor-pointer grid-cols-[26px_minmax(0,1fr)_auto_120px_auto] items-center gap-2.5 border-0 bg-transparent px-[9px] py-[7px] text-left text-fg2 transition-colors hover:bg-s3 aria-expanded:bg-s2 max-[760px]:grid-cols-[26px_minmax(0,1fr)_auto]"
      onClick={onToggle}
      type="button"
    >
      <span aria-hidden="true" className={`flex h-[24px] w-[24px] items-center justify-center rounded-[var(--radius-sm)] ${group.kind === "spike" ? "bg-warn/15 text-warn" : "bg-bad/15 text-bad"}`}>
        <Icon size={13} />
      </span>
      <span className="min-w-0">
        <strong className="block truncate text-xs font-medium text-fg" data-testid="event-mark-label">{group.metric.label}</strong>
        <small className="mt-[3px] block truncate text-xs text-fg3">
          {group.source}
          {group.metric.boundary !== null && <><span aria-hidden="true"> · </span><span data-testid="event-mark-boundary">{group.metric.boundary}</span></>}
        </small>
      </span>
      <span className="whitespace-nowrap text-right font-mono text-[13px] font-semibold tabular-nums text-fg">×{compact(group.findings.length, locale)}</span>
      <span className="max-[760px]:hidden"><MarkStrip group={group} hour={hour} onCursor={onCursor} t={t} /></span>
      <time className="whitespace-nowrap text-right font-mono text-xs tabular-nums text-fg3 max-[760px]:hidden">{moments}</time>
    </button>
    {expanded && <div className="border-t border-line2 bg-s2 px-[9px] py-[7px]">
      <p className="mb-[7px] mt-0 text-xs leading-[1.45] text-fg3" data-testid="event-mark-help">{t(group.metric.helpKey)}</p>
      <div className="grid gap-px overflow-hidden rounded-[var(--radius-sm)] border border-line2 bg-s1">
        {group.findings.slice(0, shown).map((finding) => <button
          className="grid w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)] items-baseline gap-2.5 border-0 bg-transparent px-2 py-[3px] text-left transition-colors hover:bg-s3"
          data-testid="event-mark-crossing"
          key={findingKey(finding)}
          onClick={() => onFinding(finding)}
          type="button"
        >
          <time className="whitespace-nowrap font-mono text-xs tabular-nums text-fg3">{time.timestamp(finding.timestamp)}</time>
          <span className="truncate text-xs text-fg2">{findingSource(finding, t)}</span>
        </button>)}
      </div>
      {group.findings.length > shown && <button className="mt-1.5 cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4" onClick={() => setShown((current) => current + MARK_ROWS)} type="button">{t("events.marks.more", { count: group.findings.length - shown })}</button>}
    </div>}
  </div>
}

function MarkStrip({ group, hour, onCursor, t }: {
  readonly group: MarkGroup
  readonly hour: number
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const peak = Math.max(...group.minutes, 1)
  const columns = group.minutes.length
  return <button
    aria-label={t("events.strip")}
    className="block h-[24px] w-[120px] flex-none cursor-pointer rounded-[var(--radius-xs)] border-0 bg-transparent p-0 transition-colors hover:bg-s2"
    onClick={(event) => {
      event.stopPropagation()
      const bounds = event.currentTarget.getBoundingClientRect()
      const minute = Math.min(columns - 1, Math.max(0, Math.floor(((event.clientX - bounds.left) / bounds.width) * columns)))
      onCursor(hour + minute * 60_000_000)
    }}
    tabIndex={-1}
    type="button"
  >
    <svg aria-hidden="true" className="block h-full w-full" preserveAspectRatio="none" viewBox={`0 0 ${columns * 2} 24`}>
      <line className="stroke-line2" strokeWidth="1" x1="0" x2={columns * 2} y1="23.5" y2="23.5" />
      {group.minutes.map((count, minute) => count === 0 ? null : <rect
        className={group.kind === "spike" ? "fill-warn" : "fill-bad"}
        height={Math.max(2, Math.round((count / peak) * 21))}
        key={minute}
        rx="0.5"
        width="1.6"
        x={minute * 2 + 0.2}
        y={23 - Math.max(2, Math.round((count / peak) * 21))}
      />)}
    </svg>
  </button>
}

function entryOf(entries: readonly EventEntry[], finding: Finding): EventEntry | null {
  return entries.find((entry) => entryContains(entry, finding)) ?? null
}

function entryContains(entry: EventEntry, finding: Finding): boolean {
  return entry.detailLocator.segment_id === finding.segmentId
    && entry.detailLocator.type_id === finding.typeId
    && entry.detailLocator.row_ordinal === finding.rowOrdinal
}

interface StreamState {
  readonly key: string
  readonly rows: readonly EventEntry[] | null
  readonly loading: boolean
  readonly failed: boolean
}

// Live-hour revisions are frequent; full log sections refresh at most once per minute.
const STREAM_REFRESH_MIN_MS = 60_000

function useEventGroups(data: HourData, hour: number, revision: number, onReady: () => void): StreamState {
  const wanted = EVENT_SOURCES
    .filter((source) => data.availableSections.includes(source))
    .join(",")
  const key = `${hour}:${wanted}`
  const [state, setState] = useState<StreamState>({ key: "", rows: null, loading: false, failed: false })
  const lastRead = useRef({ key: "", at: 0 })
  useEffect(() => {
    if (wanted === "") {
      setState({ key, rows: [], loading: false, failed: false })
      onReady()
      return
    }
    const now = Date.now()
    if (lastRead.current.key === key && now - lastRead.current.at < STREAM_REFRESH_MIN_MS) return
    lastRead.current = { key, at: now }
    setState((current) => current.key === key
      ? { ...current, loading: true }
      : { key, rows: null, loading: true, failed: false })
    const controller = new AbortController()
    const load = loadEventGroups(hour, wanted.split(","), controller.signal)
    acceptResponse(
      load,
      controller.signal,
      (rows) => {
        setState({
          key,
          rows,
          loading: false,
          failed: false,
        })
        onReady()
      },
      () => {
        lastRead.current = { key: "", at: 0 }
        setState((current) => ({ ...current, key, loading: false, failed: true }))
      },
    )
    load.catch((error: unknown) => {
      if (!controller.signal.aborted) console.error("events load failed", error)
    })
    return () => controller.abort()
  }, [hour, key, onReady, revision, wanted])
  return state
}
