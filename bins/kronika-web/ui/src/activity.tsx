import { ChevronDown, ChevronRight, Maximize2, Minimize2 } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"

import { INDEX_CUTS, PLAN_CUTS, PROCESS_CUTS, STATEMENT_CUTS, TABLE_CUTS, activityPreview, cutScale, type ActivityCut, type ActivityScales } from "./activity-cuts"
import { loadHeatmap, loadRelatedStatementTextRow, type DataRow, type HourData, type SegmentBound } from "./api"
import { HOUR_MICROS, collapseHeatmapView, heatmapIntensity, heatmapViewMax, type HeatmapView, type HeatmapViewRow } from "./heatmap"
import { LabelHelp, type Translate } from "./help"
import { humanBytes, humanDuration, measure, rawText, value, type Locale } from "./model"
import { canonicalSearch } from "./search"
import type { RelatedNavigation } from "./statement-navigation"

const TOP_BLOCK = 8
const TOP_CHOICES = [10, 25, 50, 100] as const
const DEFAULT_TOP = 25

type ScaleMode = "global" | "row"

// The section-specific words of one ledger: its title and the names of the
// bands that speak about the section's entities.
interface LedgerKeys {
  readonly title: string
  readonly titleHelp: string
  readonly others: string
  readonly othersLabel: string
  readonly othersHelp: string
  readonly totalsHelp: string
}

interface HeatmapState {
  readonly loading: boolean
  readonly error: boolean
  readonly view: HeatmapView | null
}

// One small request per (hour, cut, top): the server ranks next to the
// segments and answers with kilobytes, so switching cuts stays instant even
// over a slow link. Nothing is requested while the ledger stays collapsed.
function useHeatmapView(
  section: string,
  columns: number,
  labels: readonly string[],
  hour: number,
  cut: ActivityCut,
  top: number,
  revision: number,
  enabled: boolean,
): HeatmapState {
  const [state, setState] = useState<HeatmapState>({ loading: true, error: false, view: null })
  const fields = cut.fields.join(",")
  useEffect(() => {
    if (!enabled) return
    const controller = new AbortController()
    setState({ loading: true, error: false, view: null })
    loadHeatmap(hour, section, fields.split(","), labels, columns, top, controller.signal)
      .then((view) => { if (!controller.signal.aborted) setState({ loading: false, error: false, view }) })
      .catch(() => { if (!controller.signal.aborted) setState({ loading: false, error: true, view: null }) })
    return () => controller.abort()
  }, [columns, enabled, fields, hour, labels, revision, section, top])
  return state
}

function loadActivityOpen(storageKey: string): boolean {
  try {
    return localStorage.getItem(storageKey) === "1"
  } catch {
    return false
  }
}

function storeActivityOpen(storageKey: string, open: boolean): void {
  try {
    localStorage.setItem(storageKey, open ? "1" : "0")
  } catch {}
}

interface RowLabel {
  readonly text: string
  readonly prefix: string | null
}

// The shared hour activity ledger: ranked heatmap strips over one section,
// collapsed until the operator opens it. Wrappers supply the curated cuts,
// the row labels and the drill action of their section.
function ActivityLedger({ columns, cursor, cuts, defaultCut, drill, hour, keys, label, labels, locale, onCursor, onView, scales, section, storageKey, t }: {
  readonly columns: number
  readonly cursor: number
  readonly cuts: readonly ActivityCut[]
  readonly defaultCut: string
  readonly drill: (row: HeatmapViewRow) => void
  readonly hour: number
  readonly keys: LedgerKeys
  readonly label: (row: HeatmapViewRow) => RowLabel
  readonly labels: readonly string[]
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onView?: ((view: HeatmapView) => void) | undefined
  readonly scales: ActivityScales
  readonly section: string
  readonly storageKey: string
  readonly t: Translate
}) {
  const [open, setOpen] = useState(() => loadActivityOpen(storageKey))
  const [cutId, setCutId] = useState(defaultCut)
  const [scale, setScale] = useState<ScaleMode>("global")
  const [top, setTop] = useState<number>(DEFAULT_TOP)
  const [maximized, setMaximized] = useState(false)
  const [revision, setRevision] = useState(0)
  const cut = cuts.find((candidate) => candidate.id === cutId) ?? cuts[0] as ActivityCut
  const state = useHeatmapView(section, columns, labels, hour, cut, top, revision, open)
  const view = useMemo(() => {
    if (state.view === null) return null
    return maximized ? state.view : collapseHeatmapView(state.view, TOP_BLOCK)
  }, [maximized, state.view])
  const seen = onView
  useEffect(() => {
    if (state.view !== null) seen?.(state.view)
  }, [seen, state.view])

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
    storeActivityOpen(storageKey, next)
    if (!next) setMaximized(false)
  }

  if (!open) {
    return <section aria-label={t(keys.title)} className="activity-block" data-testid={`activity-${section}`}>
      <header className="flex items-center gap-1 px-2 py-[6px]">
        <button aria-expanded={false} className="flex cursor-pointer items-center gap-1 border-0 bg-transparent p-0 font-sans text-sm font-medium text-fg2" data-testid="activity-toggle" onClick={() => toggle(true)} type="button">
          <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />
          {t(keys.title)}
        </button>
        <LabelHelp helpKey={keys.titleHelp} iconOnly labelKey={keys.title} t={t} />
      </header>
    </section>
  }
  if (state.error) {
    return <section aria-label={t(keys.title)} className="activity-block" data-testid={`activity-${section}`}>
      <div className="flex items-center gap-3 px-3 py-2 font-sans text-sm text-fg3">
        <span>{t("activity.error")}</span>
        <button className="cursor-pointer border-0 bg-transparent p-0 font-sans text-sm text-accent3 underline decoration-dotted underline-offset-2" onClick={() => setRevision((current) => current + 1)} type="button">{t("activity.retry")}</button>
      </div>
    </section>
  }
  if (state.loading || view === null) {
    return <section aria-label={t(keys.title)} className="activity-block" data-testid={`activity-${section}`}>
      <header className="flex items-center gap-2 border-b border-line px-3 py-[6px]">
        <h2 className="m-0 font-sans text-sm font-medium text-fg2">{t(keys.title)}</h2>
        <span className="font-sans text-xs text-fg4">{t("activity.loading")}</span>
      </header>
      <div aria-hidden="true" className="px-3 py-2">
        {Array.from({ length: 4 }, (_, index) => <div className="mb-[6px] h-[18px] animate-pulse rounded-[3px] bg-s3 last:mb-0" key={index} />)}
      </div>
    </section>
  }

  const panel = <ActivityPanel
    columns={columns}
    cursor={cursor}
    cut={cut}
    cuts={cuts}
    drill={drill}
    hour={hour}
    keys={keys}
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
    scales={scales}
    section={section}
    t={t}
    top={top}
    view={view}
  />
  if (!maximized) return panel
  return <>
    <section aria-label={t(keys.title)} className="activity-block" data-testid={`activity-${section}-placeholder`} />
    {createPortal(<div aria-label={t(keys.title)} className="activity-overlay" data-testid="activity-overlay" role="dialog">{panel}</div>, document.body)}
  </>
}

const STATEMENT_KEYS: LedgerKeys = {
  title: "activity.title",
  titleHelp: "activity.title.help",
  others: "activity.others",
  othersLabel: "activity.others_label",
  othersHelp: "activity.others.help",
  totalsHelp: "activity.totals.help",
}

const PLAN_KEYS: LedgerKeys = {
  title: "activity.plans.title",
  titleHelp: "activity.plans.title.help",
  others: "activity.plans.others",
  othersLabel: "activity.plans.others_label",
  othersHelp: "activity.plans.others.help",
  totalsHelp: "activity.plans.totals.help",
}

const TABLE_KEYS: LedgerKeys = {
  title: "activity.tables.title",
  titleHelp: "activity.tables.title.help",
  others: "activity.tables.others",
  othersLabel: "activity.tables.others_label",
  othersHelp: "activity.tables.others.help",
  totalsHelp: "activity.tables.totals.help",
}

const INDEX_KEYS: LedgerKeys = {
  title: "activity.indexes.title",
  titleHelp: "activity.indexes.title.help",
  others: "activity.indexes.others",
  othersLabel: "activity.indexes.others_label",
  othersHelp: "activity.indexes.others.help",
  totalsHelp: "activity.indexes.totals.help",
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
  const [view, setView] = useState<HeatmapView | null>(null)
  const tableTexts = useMemo(() => statementTextsByQueryId(data.sections.pg_stat_statements ?? []), [data.sections.pg_stat_statements])
  // Text lookups land on the newest interval that actually carries data: the
  // cursor may sit on an empty stretch of the hour.
  const textAt = useMemo(() => {
    const totals = view?.totals.cells ?? []
    for (let index = totals.length - 1; index >= 0; index -= 1) {
      if (totals[index] !== null) return view?.intervals[index]?.end ?? cursor
    }
    return cursor
  }, [cursor, view])
  const fetchedTexts = useStatementTexts(view, tableTexts, segments, textAt, hour)

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

  const label = (row: HeatmapViewRow): RowLabel => {
    const queryId = rowQueryId(row)
    const text = queryId === null ? undefined : tableTexts.get(queryId) ?? fetchedTexts.get(queryId)
    return {
      text: text === undefined ? `Query ID ${queryId ?? "—"}` : activityPreview(text),
      // The identity is (queryid, userid, dbid, toplevel): the same query
      // under two roles or nested under track=all ranks as separate rows, so
      // the prefix must carry enough of the identity to tell them apart.
      prefix: identityPrefix(row.labels, row.identity[3] === "false" ? t("activity.nested") : null),
    }
  }

  return <ActivityLedger columns={60} cursor={cursor} cuts={STATEMENT_CUTS} defaultCut="exec_time" drill={drill} hour={hour} keys={STATEMENT_KEYS} label={label} labels={STATEMENT_LABELS} locale={locale} onCursor={onCursor} onView={setView} scales={{ blockSize, clockTicks: null }} section="pg_stat_statements" storageKey="kronika.activity-open" t={t} />
}

export function PlansActivity({ blockSize, cursor, data, hour, locale, onCursor, onRelated, t }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly data: HourData
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onRelated: (target: RelatedNavigation) => void
  readonly t: Translate
}) {
  const planTexts = useMemo(() => planTextsByPlanId(data.sections.pg_store_plans ?? []), [data.sections.pg_store_plans])
  const drill = (row: HeatmapViewRow) => {
    const planId = row.identity[3]
    if (planId == null || planId === "0") return
    const expression = canonicalSearch([{ key: "plan_id", value: planId }], "pg_store_plans")
    if (expression !== null) onRelated({ expression, planId, section: "plans" })
  }
  const label = (row: HeatmapViewRow): RowLabel => {
    const planId = row.identity[3]
    const text = planId == null ? undefined : planTexts.get(planId)
    return {
      text: text === undefined ? `Plan ID ${planId ?? "—"}` : activityPreview(text),
      prefix: identityPrefix(row.labels, null),
    }
  }
  return <ActivityLedger columns={60} cursor={cursor} cuts={PLAN_CUTS} defaultCut="exec_time" drill={drill} hour={hour} keys={PLAN_KEYS} label={label} labels={STATEMENT_LABELS} locale={locale} onCursor={onCursor} scales={{ blockSize, clockTicks: null }} section="pg_store_plans" storageKey="kronika.activity-open.plans" t={t} />
}

// Tables and indexes snapshot every five minutes, so their hour is twelve
// honest wide cells rather than sixty.
export function RelationsActivity({ blockSize, cursor, hour, locale, onCursor, onPattern, section, t }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onPattern: (pattern: string) => void
  readonly section: "pg_stat_user_tables" | "pg_stat_user_indexes"
  readonly t: Translate
}) {
  const indexes = section === "pg_stat_user_indexes"
  const drill = (row: HeatmapViewRow) => {
    const name = row.labels[indexes ? 3 : 2]
    if (name != null) onPattern(name)
  }
  const label = (row: HeatmapViewRow): RowLabel => {
    const [datname, schemaname, relname, indexrelname] = row.labels
    const object = [schemaname, indexes ? indexrelname ?? relname : relname]
      .filter((part): part is string => part != null)
      .join(".")
    return {
      text: object === "" ? "—" : object,
      prefix: datname ?? null,
    }
  }
  return <ActivityLedger columns={12} cursor={cursor} cuts={indexes ? INDEX_CUTS : TABLE_CUTS} defaultCut={indexes ? "idx_scan" : "writes"} drill={drill} hour={hour} keys={indexes ? INDEX_KEYS : TABLE_KEYS} label={label} labels={indexes ? INDEX_LABELS : TABLE_LABELS} locale={locale} onCursor={onCursor} scales={{ blockSize, clockTicks: null }} section={section} storageKey={`kronika.activity-open.${indexes ? "indexes" : "tables"}`} t={t} />
}

export function ProcessesActivity({ cursor, hour, locale, onCursor, onPattern, t, ticksPerSecond }: {
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onPattern: (pattern: string) => void
  readonly t: Translate
  readonly ticksPerSecond: number | null
}) {
  const drill = (row: HeatmapViewRow) => {
    const comm = row.labels[0]
    if (comm != null && comm !== "") onPattern(comm)
  }
  const label = (row: HeatmapViewRow): RowLabel => {
    const [comm, cmdline] = row.labels
    const text = cmdline != null && cmdline !== "" ? activityPreview(cmdline) : comm ?? "—"
    return { text, prefix: row.identity[0] == null ? null : `PID ${row.identity[0]}` }
  }
  return <ActivityLedger columns={60} cursor={cursor} cuts={PROCESS_CUTS} defaultCut="cpu" drill={drill} hour={hour} keys={PROCESS_KEYS} label={label} labels={PROCESS_LABELS} locale={locale} onCursor={onCursor} scales={{ blockSize: null, clockTicks: ticksPerSecond }} section="os_process" storageKey="kronika.activity-open.processes" t={t} />
}

const PROCESS_KEYS: LedgerKeys = {
  title: "activity.processes.title",
  titleHelp: "activity.processes.title.help",
  others: "activity.processes.others",
  othersLabel: "activity.processes.others_label",
  othersHelp: "activity.processes.others.help",
  totalsHelp: "activity.processes.totals.help",
}

const PROCESS_LABELS = ["comm", "cmdline"] as const
const STATEMENT_LABELS = ["datname", "usename"] as const
const TABLE_LABELS = ["datname", "schemaname", "relname"] as const
const INDEX_LABELS = ["datname", "schemaname", "relname", "indexrelname"] as const

function identityPrefix(labels: readonly (string | null)[], marker: string | null): string | null {
  const parts = [
    ...(labels[0] == null ? [] : [labels[0]]),
    ...(labels[1] == null || labels[1] === labels[0] ? [] : [labels[1]]),
    ...(marker === null ? [] : [marker]),
  ]
  return parts.length === 0 ? null : parts.join(" · ")
}

function rowQueryId(row: HeatmapViewRow): string | null {
  const stored = row.identity[0]
  return stored == null || stored === "0" ? null : stored
}

// Query text is the one heavy label: it never travels with the ranking
// response. The loaded table page answers most of the ranked entities; the
// remainder is fetched one bounded text row per queryid, after ranking.
function statementTextsByQueryId(tableRows: readonly DataRow[]): ReadonlyMap<string, string> {
  const texts = new Map<string, string>()
  for (const row of tableRows) {
    const queryId = rawText(value(row, "queryid"))
    const text = rawText(value(row, "query"))
    if (queryId === null || text === null || text === "" || texts.has(queryId)) continue
    texts.set(queryId, text)
  }
  return texts
}

// Plan text reaches the ledger only through the loaded table page; there is
// no bounded single-plan text fetch, so an unlabeled row keeps its plan id.
function planTextsByPlanId(tableRows: readonly DataRow[]): ReadonlyMap<string, string> {
  const texts = new Map<string, string>()
  for (const row of tableRows) {
    const planId = rawText(value(row, "planid"))
    const text = rawText(value(row, "plan"))
    if (planId === null || text === null || text === "" || texts.has(planId)) continue
    texts.set(planId, text)
  }
  return texts
}

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

function ActivityPanel({ columns, cursor, cut, cuts, drill, hour, keys, label, locale, maximized, onCollapse, onCursor, onCut, onMaximized, onScale, onTop, scale, scales, section, t, top, view }: {
  readonly columns: number
  readonly cursor: number
  readonly cut: ActivityCut
  readonly cuts: readonly ActivityCut[]
  readonly drill: (row: HeatmapViewRow) => void
  readonly hour: number
  readonly keys: LedgerKeys
  readonly label: (row: HeatmapViewRow) => RowLabel
  readonly locale: Locale
  readonly maximized: boolean
  readonly onCollapse: () => void
  readonly onCursor: (timestamp: number) => void
  readonly onCut: (id: string) => void
  readonly onMaximized: (maximized: boolean) => void
  readonly onScale: (scale: ScaleMode) => void
  readonly onTop: (top: number) => void
  readonly scale: ScaleMode
  readonly scales: ActivityScales
  readonly section: string
  readonly t: Translate
  readonly top: number
  readonly view: HeatmapView
}) {
  const { kind, scale: valueScale } = cutScale(cut, scales)
  // A gauge cell is a plain reading, not a rate.
  const suffix = view.cumulative ? t("unit.per_second") : ""
  const cursorColumn = cursor >= hour && cursor < hour + HOUR_MICROS
    ? Math.min(columns - 1, Math.floor(((cursor - hour) * columns) / HOUR_MICROS))
    : null
  const globalMax = heatmapViewMax(view)
  const totalsMax = view.totals.cells.reduce<number>((current, cell) => cell !== null && cell > current ? cell : current, 0)
  const rowMax = (cells: readonly (number | null)[]) => scale === "row"
    ? cells.reduce<number>((current, cell) => cell !== null && cell > current ? cell : current, 0)
    : globalMax
  const atCursor = (cells: readonly (number | null)[]) => {
    const cell = cursorColumn === null ? null : cells[cursorColumn] ?? null
    return cell === null ? "—" : formatValue(cell * valueScale, kind, locale, suffix)
  }
  const total = (stored: number | null) => stored === null ? "—" : formatValue(stored * valueScale, kind, locale)

  return <section aria-label={t(keys.title)} className={`activity-block${maximized ? " activity-max" : ""}`} data-testid={`activity-${section}`}>
    <header className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-3 py-[6px]">
      <button aria-expanded className="flex cursor-pointer items-center gap-1 border-0 bg-transparent p-0 font-sans text-sm font-medium text-fg2" data-testid="activity-toggle" onClick={onCollapse} type="button">
        <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} />
        {t(keys.title)}
      </button>
      <LabelHelp helpKey={keys.titleHelp} iconOnly labelKey={keys.title} t={t} />
      <div aria-label={t("activity.cut_label")} className="lens-tabs" role="group">
        {cuts.map((candidate) => <button aria-pressed={cut.id === candidate.id} data-testid={`activity-cut-${candidate.id}`} key={candidate.id} onClick={() => onCut(candidate.id)} type="button">{t(`activity.cut.${candidate.id}`)}</button>)}
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
      <ActivityRow cells={view.totals.cells} cursor={cursor} help={<LabelHelp helpKey={keys.totalsHelp} iconOnly labelKey="activity.totals" t={t} />} hour={hour} max={totalsMax} muted onCursor={onCursor} reading={atCursor(view.totals.cells)} testId="activity-row-totals" text={t("activity.totals")} total={total(view.totals.total)} />
      {view.rows.map((row) => {
        const { prefix, text } = label(row)
        return <ActivityRow cells={row.cells} cursor={cursor} hour={hour} key={`${row.typeId}:${row.identity.join(":")}`} max={rowMax(row.cells)} onClick={() => drill(row)} onCursor={onCursor} prefix={prefix} reading={atCursor(row.cells)} testId="activity-row" text={text} total={total(row.total)} />
      })}
      {view.othersCount > 0 && <ActivityRow cells={view.others.cells} cursor={cursor} help={<LabelHelp helpKey={keys.othersHelp} iconOnly labelKey={keys.othersLabel} t={t} />} hour={hour} max={rowMax(view.others.cells)} muted onCursor={onCursor} reading={atCursor(view.others.cells)} testId="activity-row-others" text={t(keys.others, { count: String(view.othersCount) })} total={total(view.others.total)} />}
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

function formatValue(stored: number, kind: ActivityCut["kind"], locale: Locale, suffix = ""): string {
  if (kind === "bytes") return humanBytes(stored, locale, suffix)
  if (kind === "milliseconds") return humanDuration(stored, locale, "milliseconds", suffix)
  if (kind === "seconds") return humanDuration(stored, locale, "seconds", suffix)
  if (kind === "nanoseconds") return humanDuration(stored, locale, "nanoseconds", suffix)
  return measure(stored, locale, suffix)
}
