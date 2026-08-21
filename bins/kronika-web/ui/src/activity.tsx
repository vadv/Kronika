import { ChevronDown, ChevronRight, Maximize2, Minimize2 } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"

import {
  STATEMENT_CUTS,
  STATEMENT_DEFAULT_CUT,
  statementPreview,
  statementTextsByQueryId,
  type StatementCut,
} from "./activity-statements"
import { loadHeatmap, loadRelatedStatementTextRow, type HourData, type SegmentBound } from "./api"
import { HOUR_MICROS, collapseHeatmapView, heatmapIntensity, heatmapViewMax, type HeatmapView, type HeatmapViewRow } from "./heatmap"
import { LabelHelp, type Translate } from "./help"
import { humanBytes, humanDuration, measure, rawText, value, type Locale } from "./model"
import { canonicalSearch } from "./search"
import type { RelatedNavigation } from "./statement-navigation"

const COLUMNS = 60
const TOP_BLOCK = 8
const TOP_CHOICES = [10, 25, 50, 100] as const
const DEFAULT_TOP = 25
const OPEN_KEY = "kronika.activity-open"
const ACTIVITY_LABELS = ["datname", "usename"] as const

type ScaleMode = "global" | "row"

interface HeatmapState {
  readonly loading: boolean
  readonly error: boolean
  readonly view: HeatmapView | null
}

// One small request per (hour, cut, top): the server ranks next to the
// segments and answers with kilobytes, so switching cuts stays instant even
// over a slow link. Nothing is requested while the ledger stays collapsed.
function useHeatmapView(hour: number, cut: StatementCut, top: number, revision: number, enabled: boolean): HeatmapState {
  const [state, setState] = useState<HeatmapState>({ loading: true, error: false, view: null })
  useEffect(() => {
    if (!enabled) return
    const controller = new AbortController()
    setState({ loading: true, error: false, view: null })
    loadHeatmap(hour, "pg_stat_statements", cut.field, ACTIVITY_LABELS, COLUMNS, top, controller.signal)
      .then((view) => { if (!controller.signal.aborted) setState({ loading: false, error: false, view }) })
      .catch(() => { if (!controller.signal.aborted) setState({ loading: false, error: true, view: null }) })
    return () => controller.abort()
  }, [cut.field, enabled, hour, revision, top])
  return state
}

function loadActivityOpen(): boolean {
  try {
    return localStorage.getItem(OPEN_KEY) === "1"
  } catch {
    return false
  }
}

function storeActivityOpen(open: boolean): void {
  try {
    localStorage.setItem(OPEN_KEY, open ? "1" : "0")
  } catch {}
}

export function StatementsActivity({ blockSize, cursor, data, hour, locale, onCursor, onRelated, segments, t }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly data: HourData
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onRelated: (target: RelatedNavigation) => void
  readonly segments: readonly SegmentBound[]
  readonly t: Translate
}) {
  const [open, setOpen] = useState(loadActivityOpen)
  const [cutId, setCutId] = useState(STATEMENT_DEFAULT_CUT)
  const [scale, setScale] = useState<ScaleMode>("global")
  const [top, setTop] = useState<number>(DEFAULT_TOP)
  const [maximized, setMaximized] = useState(false)
  const [revision, setRevision] = useState(0)
  const cut = STATEMENT_CUTS.find((candidate) => candidate.id === cutId) ?? STATEMENT_CUTS[0] as StatementCut
  const state = useHeatmapView(hour, cut, top, revision, open)
  const view = useMemo(() => {
    if (state.view === null) return null
    return maximized ? state.view : collapseHeatmapView(state.view, TOP_BLOCK)
  }, [maximized, state.view])
  const tableTexts = useMemo(() => statementTextsByQueryId(data.sections.pg_stat_statements ?? []), [data.sections.pg_stat_statements])
  // Text lookups land on the newest interval that actually carries data: the
  // cursor may sit on an empty stretch of the hour.
  const textAt = useMemo(() => {
    const totals = state.view?.totals.cells ?? []
    for (let index = totals.length - 1; index >= 0; index -= 1) {
      if (totals[index] !== null) return state.view?.intervals[index]?.end ?? cursor
    }
    return cursor
  }, [cursor, state.view])
  const fetchedTexts = useStatementTexts(state.view, tableTexts, segments, textAt, hour)

  useEffect(() => {
    if (!maximized) return
    const keydown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      setMaximized(false)
    }
    window.addEventListener("keydown", keydown, true)
    return () => window.removeEventListener("keydown", keydown, true)
  }, [maximized])

  const toggle = (next: boolean) => {
    setOpen(next)
    storeActivityOpen(next)
    if (!next) setMaximized(false)
  }

  if (!open) {
    return <section aria-label={t("activity.title")} className="activity-block" data-testid="statements-activity">
      <header className="flex items-center gap-1 px-2 py-[6px]">
        <button aria-expanded={false} className="flex cursor-pointer items-center gap-1 border-0 bg-transparent p-0 font-sans text-sm font-medium text-fg2" data-testid="activity-toggle" onClick={() => toggle(true)} type="button">
          <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />
          {t("activity.title")}
        </button>
        <LabelHelp helpKey="activity.title.help" iconOnly labelKey="activity.title" t={t} />
      </header>
    </section>
  }
  if (state.error) {
    return <section aria-label={t("activity.title")} className="activity-block" data-testid="statements-activity">
      <div className="flex items-center gap-3 px-3 py-2 font-sans text-sm text-fg3">
        <span>{t("activity.error")}</span>
        <button className="cursor-pointer border-0 bg-transparent p-0 font-sans text-sm text-accent3 underline decoration-dotted underline-offset-2" onClick={() => setRevision((current) => current + 1)} type="button">{t("activity.retry")}</button>
      </div>
    </section>
  }
  if (state.loading || view === null) {
    return <section aria-label={t("activity.title")} className="activity-block" data-testid="statements-activity">
      <header className="flex items-center gap-2 border-b border-line px-3 py-[6px]">
        <h2 className="m-0 font-sans text-sm font-medium text-fg2">{t("activity.title")}</h2>
        <span className="font-sans text-xs text-fg4">{t("activity.loading")}</span>
      </header>
      <div aria-hidden="true" className="px-3 py-2">
        {Array.from({ length: 4 }, (_, index) => <div className="mb-[6px] h-[18px] animate-pulse rounded-[3px] bg-s3 last:mb-0" key={index} />)}
      </div>
    </section>
  }

  const drill = (row: HeatmapViewRow) => {
    const queryId = rowQueryId(row)
    if (queryId === null) return
    const [database, role] = row.labels
    const clauses = [
      ...(database == null ? [] : [{ key: "database", value: database }]),
      ...(role == null ? [] : [{ key: "role", value: role }]),
      { key: "query_id", value: queryId },
    ]
    const expression = canonicalSearch(clauses, "pg_stat_statements")
    if (expression !== null) onRelated({ expression, queryId, section: "statements" })
  }

  const label = (row: HeatmapViewRow) => {
    const queryId = rowQueryId(row)
    const text = queryId === null ? undefined : tableTexts.get(queryId) ?? fetchedTexts.get(queryId)
    // The identity is (queryid, userid, dbid, toplevel): the same query under
    // two roles or nested under track=all ranks as separate rows, so the
    // prefix must carry enough of the identity to tell them apart.
    const parts = [
      ...(row.labels[0] == null ? [] : [row.labels[0]]),
      ...(row.labels[1] == null || row.labels[1] === row.labels[0] ? [] : [row.labels[1]]),
      ...(row.identity[3] === "false" ? [t("activity.nested")] : []),
    ]
    return {
      text: text === undefined ? `Query ID ${queryId ?? "—"}` : statementPreview(text),
      prefix: parts.length === 0 ? null : parts.join(" · "),
    }
  }

  const panel = <ActivityPanel
    blockScale={cut.blockScaled === true ? blockSize : 1}
    cursor={cursor}
    cut={cut}
    drill={drill}
    hour={hour}
    label={label}
    locale={locale}
    maximized={maximized}
    onCollapse={() => toggle(false)}
    onCursor={onCursor}
    onCut={setCutId}
    onMaximized={setMaximized}
    onScale={setScale}
    onTop={setTop}
    scale={scale}
    t={t}
    top={top}
    view={view}
  />
  if (!maximized) return panel
  return <>
    <section aria-label={t("activity.title")} className="activity-block" data-testid="statements-activity-placeholder" />
    {createPortal(<div aria-label={t("activity.title")} className="activity-overlay" data-testid="activity-overlay" role="dialog">{panel}</div>, document.body)}
  </>
}

function rowQueryId(row: HeatmapViewRow): string | null {
  const stored = row.identity[0]
  return stored == null || stored === "0" ? null : stored
}

// Query text for the ranked entities: the loaded table page answers most of
// them; the remainder is fetched one bounded text row per queryid, only after
// ranking decided the row is worth a label.
function useStatementTexts(
  view: HeatmapView | null,
  tableTexts: ReadonlyMap<string, string>,
  segments: readonly SegmentBound[],
  at: number,
  hour: number,
): ReadonlyMap<string, string> {
  const [texts, setTexts] = useState<ReadonlyMap<string, string>>(new Map())
  const requested = useRef(new Set<string>())
  const known = useRef(hour)
  if (known.current !== hour) {
    known.current = hour
    requested.current = new Set()
    if (texts.size > 0) setTexts(new Map())
  }
  const missing = (view?.rows ?? [])
    .map(rowQueryId)
    .filter((queryId): queryId is string => queryId !== null && !tableTexts.has(queryId) && !requested.current.has(queryId))
  const wanted = JSON.stringify(missing)
  useEffect(() => {
    const queryIds = JSON.parse(wanted) as readonly string[]
    if (queryIds.length === 0) return
    for (const queryId of queryIds) requested.current.add(queryId)
    const controller = new AbortController()
    void Promise.all(queryIds.map(async (queryId) => {
      const row = await loadRelatedStatementTextRow(segments, at, queryId, controller.signal).catch(() => null)
      const text = row === null ? null : rawText(value(row, "query"))
      return text === null ? null : ([queryId, text] as const)
    })).then((entries) => {
      const found = entries.filter((entry): entry is readonly [string, string] => entry !== null)
      if (controller.signal.aborted || found.length === 0) return
      setTexts((current) => new Map([...current, ...found]))
    })
    return () => controller.abort()
  }, [at, segments, wanted])
  return texts
}

function ActivityPanel({ blockScale, cursor, cut, drill, hour, label, locale, maximized, onCollapse, onCursor, onCut, onMaximized, onScale, onTop, scale, t, top, view }: {
  readonly blockScale: number | null
  readonly cursor: number
  readonly cut: StatementCut
  readonly drill: (row: HeatmapViewRow) => void
  readonly hour: number
  readonly label: (row: HeatmapViewRow) => { readonly text: string; readonly prefix: string | null }
  readonly locale: Locale
  readonly maximized: boolean
  readonly onCollapse: () => void
  readonly onCursor: (timestamp: number) => void
  readonly onCut: (id: string) => void
  readonly onMaximized: (maximized: boolean) => void
  readonly onScale: (scale: ScaleMode) => void
  readonly onTop: (top: number) => void
  readonly scale: ScaleMode
  readonly t: Translate
  readonly top: number
  readonly view: HeatmapView
}) {
  // Without a recorded block size the block counters stay honest block counts.
  const kind = cut.blockScaled === true && blockScale === null ? "count" : cut.kind
  const valueScale = blockScale ?? 1
  const cursorColumn = cursor >= hour && cursor < hour + HOUR_MICROS
    ? Math.min(COLUMNS - 1, Math.floor(((cursor - hour) * COLUMNS) / HOUR_MICROS))
    : null
  const globalMax = heatmapViewMax(view)
  const totalsMax = view.totals.cells.reduce<number>((current, cell) => cell !== null && cell > current ? cell : current, 0)
  const rowMax = (cells: readonly (number | null)[]) => scale === "row"
    ? cells.reduce<number>((current, cell) => cell !== null && cell > current ? cell : current, 0)
    : globalMax
  const atCursor = (cells: readonly (number | null)[]) => {
    const cell = cursorColumn === null ? null : cells[cursorColumn] ?? null
    return cell === null ? "—" : formatValue(cell * valueScale, kind, locale, t("unit.per_second"))
  }
  const total = (stored: number | null) => stored === null ? "—" : formatValue(stored * valueScale, kind, locale)

  return <section aria-label={t("activity.title")} className={`activity-block${maximized ? " activity-max" : ""}`} data-testid="statements-activity">
    <header className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-3 py-[6px]">
      <button aria-expanded className="flex cursor-pointer items-center gap-1 border-0 bg-transparent p-0 font-sans text-sm font-medium text-fg2" data-testid="activity-toggle" onClick={onCollapse} type="button">
        <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} />
        {t("activity.title")}
      </button>
      <LabelHelp helpKey="activity.title.help" iconOnly labelKey="activity.title" t={t} />
      <div aria-label={t("activity.cut_label")} className="lens-tabs" role="group">
        {STATEMENT_CUTS.map((candidate) => <button aria-pressed={cut.id === candidate.id} data-testid={`activity-cut-${candidate.id}`} key={candidate.id} onClick={() => onCut(candidate.id)} type="button">{t(`activity.cut.${candidate.id}`)}</button>)}
      </div>
      <LabelHelp helpKey={`activity.cut.${cut.id}.help`} iconOnly labelKey={`activity.cut.${cut.id}`} t={t} />
      <span className="font-sans text-xs text-fg4" data-testid="activity-top-count">{t("activity.top", { top: String(view.rows.length), total: String(view.entityCount) })}</span>
      <div className="ml-auto flex items-center gap-2">
        {maximized && <div aria-label={t("activity.top_label")} className="lens-tabs" role="group">
          {TOP_CHOICES.map((choice) => <button aria-pressed={top === choice} data-testid={`activity-top-${choice}`} key={choice} onClick={() => onTop(choice)} type="button">{choice}</button>)}
        </div>}
        <div aria-label={t("activity.scale_label")} className="lens-tabs" role="group">
          <button aria-pressed={scale === "global"} data-testid="activity-scale-global" onClick={() => onScale("global")} type="button">{t("activity.scale.global")}</button>
          <button aria-pressed={scale === "row"} data-testid="activity-scale-row" onClick={() => onScale("row")} type="button">{t("activity.scale.row")}</button>
        </div>
        <LabelHelp helpKey="activity.scale.help" iconOnly labelKey="activity.scale_label" t={t} />
        <button aria-label={t(maximized ? "activity.restore" : "activity.maximize")} aria-pressed={maximized} className="inspector-maximize" data-testid="activity-maximize" onClick={() => onMaximized(!maximized)} type="button">{maximized ? <Minimize2 aria-hidden="true" size={13} /> : <Maximize2 aria-hidden="true" size={13} />}</button>
      </div>
    </header>
    {view.entityCount === 0 && <div className="px-3 py-2 font-sans text-sm text-fg4">{t("activity.empty")}</div>}
    {view.entityCount > 0 && <div className={maximized ? "min-h-0 flex-1 overflow-y-auto" : ""}>
      <ActivityRow cells={view.totals.cells} cursor={cursor} help={<LabelHelp helpKey="activity.totals.help" iconOnly labelKey="activity.totals" t={t} />} hour={hour} max={totalsMax} muted onCursor={onCursor} reading={atCursor(view.totals.cells)} testId="activity-row-totals" text={t("activity.totals")} total={total(view.totals.total)} />
      {view.rows.map((row) => {
        const { prefix, text } = label(row)
        return <ActivityRow cells={row.cells} cursor={cursor} hour={hour} key={`${row.typeId}:${row.identity.join(":")}`} max={rowMax(row.cells)} onClick={() => drill(row)} onCursor={onCursor} prefix={prefix} reading={atCursor(row.cells)} testId="activity-row" text={text} total={total(row.total)} />
      })}
      {view.othersCount > 0 && <ActivityRow cells={view.others.cells} cursor={cursor} help={<LabelHelp helpKey="activity.others.help" iconOnly labelKey="activity.others_label" t={t} />} hour={hour} max={rowMax(view.others.cells)} muted onCursor={onCursor} reading={atCursor(view.others.cells)} testId="activity-row-others" text={t("activity.others", { count: String(view.othersCount) })} total={total(view.others.total)} />}
    </div>}
  </section>
}

function ActivityRow({ cells, cursor, help, hour, max, muted = false, onClick, onCursor, prefix = null, reading, testId, text, total }: {
  readonly cells: readonly (number | null)[]
  readonly cursor: number
  readonly help?: React.ReactNode
  readonly hour: number
  readonly max: number
  readonly muted?: boolean
  readonly onClick?: (() => void) | undefined
  readonly onCursor: (timestamp: number) => void
  readonly prefix?: string | null
  readonly reading: string
  readonly testId: string
  readonly text: string
  readonly total: string
}) {
  return <div className={`activity-row${onClick === undefined ? "" : " activity-row-link"}`} data-testid={testId} onClick={onClick}>
    <span className={`flex min-w-0 items-baseline gap-[6px] overflow-hidden px-2 ${muted ? "font-sans text-xs text-fg4" : "font-mono text-xs text-fg2"}`} title={text}>
      {prefix !== null && <span className="flex-none font-sans text-fg4">{prefix}</span>}
      <span className="overflow-hidden text-ellipsis whitespace-nowrap">{text}</span>
      {help}
    </span>
    <ActivityStrip cells={cells} cursor={cursor} hour={hour} max={max} onCursor={onCursor} />
    <strong className="px-2 text-right font-mono text-xs font-normal tabular-nums text-fg2">{total}</strong>
    <strong className="px-2 text-right font-mono text-xs font-normal tabular-nums text-fg3">{reading}</strong>
  </div>
}

function ActivityStrip({ cells, cursor, hour, max, onCursor }: {
  readonly cells: readonly (number | null)[]
  readonly cursor: number
  readonly hour: number
  readonly max: number
  readonly onCursor: (timestamp: number) => void
}) {
  const columns = Math.max(cells.length, 1)
  const cursorX = cursor >= hour && cursor < hour + HOUR_MICROS ? ((cursor - hour) / HOUR_MICROS) * columns : null
  const pick = (event: React.MouseEvent<SVGSVGElement>) => {
    event.stopPropagation()
    const bounds = event.currentTarget.getBoundingClientRect()
    if (bounds.width <= 0) return
    const column = Math.max(0, Math.min(columns - 1, Math.floor(((event.clientX - bounds.left) / bounds.width) * columns)))
    onCursor(hour + Math.floor(((column + 1) * HOUR_MICROS) / columns) - 1)
  }
  return <svg className="activity-strip" onClick={pick} preserveAspectRatio="none" viewBox={`0 0 ${columns} 8`}>
    {cells.map((cell, index) => cell === null
      ? null
      : <rect className={`heat-${heatmapIntensity(cell, max)}`} height={8} key={index} width={0.9} x={index + 0.05} y={0} />)}
    {cursorX !== null && <>
      <path className="activity-cursor-halo" d={`M${cursorX.toFixed(3)} 0 V8`} vectorEffect="non-scaling-stroke" />
      <path className="activity-cursor" d={`M${cursorX.toFixed(3)} 0 V8`} vectorEffect="non-scaling-stroke" />
    </>}
  </svg>
}

function formatValue(stored: number, kind: StatementCut["kind"], locale: Locale, suffix = ""): string {
  if (kind === "bytes") return humanBytes(stored, locale, suffix)
  if (kind === "milliseconds") return humanDuration(stored, locale, "milliseconds", suffix)
  return measure(stored, locale, suffix)
}
