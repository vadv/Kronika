import { CircleAlert, Diamond, TriangleAlert } from "lucide-react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { Cell, DataRow, Finding, HourData } from "./api"
import { DetailList, DetailRow } from "./detail-list"
import { useDisplayTime } from "./display-time-context"
import {
  findingCategory,
  findingDetailFields,
  findingEntity,
  findingHistory,
  findingKey,
  findingMetric,
  findingOrder,
  findingReadings,
  findingSource,
} from "./finding-presentation"
import type { Translate } from "./help"
import { globMatcher } from "./glob"
import { InspectorPortal } from "./inspector"
import { asNumber, compact, humanBytes, humanDuration, humanPercent, identifier, type Locale, rawText, shownMoment } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"
import { parseSearch } from "./search"
import { TableFilter } from "./table-filter"

type Filter = "all" | "event" | "known_bad"
export type FindingResolution = "idle" | "loading" | "ready" | "unavailable"

const ERROR_CATEGORIES = [
  "events.category.lock", "events.category.constraint", "events.category.serialization",
  "events.category.timeout", "events.category.resource", "events.category.data_corruption",
  "events.category.system", "events.category.connection", "events.category.auth",
  "events.category.syntax", "events.category.other",
] as const
const EVENT_PREFIX = "events."

export function EventsView({
  cursor,
  data,
  history,
  hour,
  loading = false,
  locale,
  navigationTimestamps,
  onCursor,
  onClose,
  onFinding,
  onOpenChart,
  onPattern,
  onShowAll,
  onSelectedLane,
  resolution,
  resolved,
  scope,
  pattern,
  selected,
  selectedLane,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly history: readonly ChartPoint[]
  readonly hour: number
  readonly loading?: boolean | undefined
  readonly locale: Locale
  readonly navigationTimestamps: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly onClose: () => void
  readonly onFinding: (finding: Finding) => void
  readonly onOpenChart: () => void
  readonly onPattern: (pattern: string) => void
  readonly onShowAll: () => void
  readonly onSelectedLane: (lane: string) => void
  readonly resolution: FindingResolution
  readonly resolved: DataRow | null
  readonly scope: readonly Finding[] | null
  readonly pattern: string
  readonly selected: Finding | null
  readonly selectedLane: string
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const [filter, setFilter] = useState<Filter>("all")
  useEffect(() => {
    if (scope === null) return
    setFilter("all")
  }, [scope])
  const parsedSearch = useMemo(() => parseSearch(pattern, "events"), [pattern])
  const visible = useMemo(() => (scope ?? data.findings)
    .filter((finding) => {
      if (filter !== "all" && finding.kind !== filter) return false
      if (!parsedSearch.ok || parsedSearch.query.canonical === "") return true
      const selectedRow = selected !== null && findingKey(finding) === findingKey(selected) ? resolved : null
      const text = [findingCategory(finding, t), findingSource(finding, t), finding.logicalName,
        ...Object.values(selectedRow?.values ?? {}).map((cell) => rawText(cell) ?? "")]
      const fields: Readonly<Record<string, readonly string[]>> = {
        text,
        kind: [finding.kind],
        source: [findingSource(finding, t), finding.logicalName],
        category: [findingCategory(finding, t)],
      }
      const clauses = parsedSearch.query.structured
        ? parsedSearch.query.clauses
        : [{ key: "text", value: parsedSearch.query.freeText ?? "" }]
      return clauses.every((clause) => fields[clause.key]?.some((candidate) => globMatcher(clause.value)?.(candidate) ?? true) === true)
    })
    .slice()
    .sort((left, right) => findingOrder(right, left)), [data.findings, filter, parsedSearch, resolved, scope, selected, t])
  const active = selected !== null && visible.some((finding) => findingKey(finding) === findingKey(selected)) ? selected : null
  const list = useRef<HTMLDivElement>(null)
  const virtual = useVirtualizer({ count: visible.length, estimateSize: () => 50, getScrollElement: () => list.current, overscan: 12 })
  useEffect(() => {
    if (active === null) return
    const index = visible.findIndex((finding) => findingKey(finding) === findingKey(active))
    if (index >= 0) virtual.scrollToIndex(index, { align: "auto" })
  }, [active, virtual, visible])
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  const original = scope === null
    ? data.findingGroups.reduce((total, group) => total + group.totalHits, 0) || data.findings.length
    : scope.length
  const omitted = scope === null ? Math.max(0, original - data.findings.length) : 0
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={onCursor} onFinding={onFinding} onOpenChart={onOpenChart} onSelectedLane={onSelectedLane} primaryLane="health" selectedLane={selectedLane} shownAt={shownAt} t={t} />
    <section className="mt-2" data-testid="events-console">
      <header className="flex min-h-[38px] items-center justify-between border-b border-line2 px-1.5 py-1 max-[760px]:flex-col max-[760px]:items-stretch max-[760px]:gap-[5px]">
        <div className="flex border border-line3" role="group" aria-label={t("events.filters")}>
          {(["all", "event", "known_bad"] as const).map((choice) => <button aria-pressed={filter === choice} className="min-h-[27px] cursor-pointer border-0 bg-transparent px-2.5 text-xs uppercase text-fg3 aria-pressed:bg-s4 aria-pressed:text-accent3" key={choice} onClick={() => setFilter(choice)} type="button">{choice === "all" ? t("events.all") : t(`locator.${choice}`)}</button>)}
        </div>
        <span className="text-xs tabular-nums text-fg3">{t("events.count", { "shown": visible.length, total: original })}{omitted > 0 ? ` · ${t("events.omitted", { count: omitted })}` : ""}</span>
        {scope !== null && <button className="min-h-[29px] cursor-pointer border border-line4 bg-s2 px-[9px] text-xs uppercase text-accent3" onClick={onShowAll} type="button">{t("events.show_all", { count: scope.length })}</button>}
      </header>
      <TableFilter kept={visible.length} onPattern={onPattern} pattern={pattern} surface="events" t={t} total={original} />
      <div className="grid min-h-[430px] grid-cols-[minmax(0,1fr)]">
        <div className="h-[min(570px,calc(100vh-360px))] min-h-[390px] overflow-auto border-line2 max-[760px]:h-[260px] max-[760px]:min-h-[180px]" ref={list} role="list">
          {visible.length === 0 && (loading
        ? <p className="table-empty flex items-baseline" role="status"><span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none mr-[7px] h-[11px] w-[11px] align-[-1px]" />{t("table.loading")}</p>
        : <div className="table-empty">{t("events.empty")}</div>)}
          <div className="relative" style={{ height: virtual.getTotalSize() }}>
            {virtual.getVirtualItems().map((item) => {
              const finding = visible[item.index]
              if (finding === undefined) return null
              const pressed = active !== null && findingKey(active) === findingKey(finding)
              return <div className="absolute left-0 top-0 w-full border-b border-line" data-testid="event-item" key={findingKey(finding)} role="listitem" style={{ height: item.size, transform: `translateY(${item.start}px)` }}>
                <button aria-label={`${findingCategory(finding, t)} · ${findingSource(finding, t)} · ${time.timestamp(finding.timestamp)}`} aria-pressed={pressed} className="grid h-full min-h-[44px] w-full cursor-pointer grid-cols-[18px_minmax(0,1fr)_auto] items-center gap-2 border-0 bg-s1 px-[9px] py-1.5 text-left text-fg2 hover:bg-s3 aria-pressed:bg-s4 aria-pressed:shadow-[inset_2px_0_var(--color-accent)]" onClick={() => onFinding(finding)} type="button">
                  <KindIcon kind={finding.kind} />
                  <span><strong className="block text-xs font-medium uppercase">{findingCategory(finding, t)}</strong><small className="mt-[3px] block text-xs text-fg3">{findingSource(finding, t)}</small></span>
                  <time className="whitespace-nowrap text-xs text-fg3">{time.timestamp(finding.timestamp)}</time>
                </button>
              </div>
            })}
          </div>
        </div>
        {active !== null && <InspectorPortal identity={`event:${findingKey(active)}`} onClose={onClose} title={findingSource(active, t)}><FindingDetail cursor={cursor} data={data} finding={active} history={history} hour={hour} locale={locale} onCursor={onCursor} resolution={resolution} row={resolved} t={t} /></InspectorPortal>}
      </div>
    </section>
  </>
}

function FindingDetail({ cursor, data, finding, history, hour, locale, onCursor, resolution, row, t }: {
  readonly cursor: number
  readonly data: HourData
  readonly finding: Finding
  readonly history: readonly ChartPoint[]
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly resolution: FindingResolution
  readonly row: DataRow | null
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const metric = findingMetric(finding, t)
  const points = history.length === 0 ? findingHistory(finding, row === null ? [] : [row], data) : history
  const readings = findingReadings(finding, row, points, data)
  const entity = findingEntity(row)
  return <aside className="p-[11px]" data-testid="event-detail">
    <header className="grid grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-[9px] border-b border-line3 pb-[9px]"><KindIcon kind={finding.kind} /><div><span className="text-xs uppercase text-fg3">{findingCategory(finding, t)}</span><h2 className="mt-[3px] text-md">{findingSource(finding, t)}</h2></div><time className="text-xs text-fg2">{time.timestamp(finding.timestamp)}</time></header>
    {resolution === "loading" && <p className="mt-2.5 text-sm leading-[1.45] text-fg2 [overflow-wrap:anywhere]">{t("events.loading_row")}</p>}
    {resolution === "unavailable" && <p className="mt-2.5 text-sm leading-[1.45] text-fg2 [overflow-wrap:anywhere]">{t("events.row_unavailable")}</p>}
    {resolution === "ready" && row !== null && <>
      {entity !== null && <p className="mt-2.5 text-sm leading-[1.45] text-fg2 [overflow-wrap:anywhere]">{entity}</p>}
      {finding.category !== null && <p className="mt-[9px] inline-block border border-line3 bg-s2 px-[7px] py-[5px] text-xs text-fg2">{t("events.category", { "category": categoryLabel(finding.category, t) })}</p>}
      {metric.field !== null && <section className="mt-[9px] grid gap-1 border border-line3 bg-s2 px-[9px] py-[7px]" aria-label={t("events.change")}>
        <span className="text-xs text-fg3">{metric.label}</span>
        <strong className="text-sm tabular-nums text-fg">{readings.previous === null
          ? formatMetric(readings.current, metric.unit, locale, t)
          : `${formatMetric(readings.previous, metric.unit, locale, t)} → ${formatMetric(readings.current, metric.unit, locale, t)}`}</strong>
        {metric.boundary !== null && <small className="text-xs text-fg3">{t("events.boundary", { "boundary": metric.boundary })}</small>}
      </section>}
      <DetailList>{findingDetailFields(row, finding).map(([field, cell]) => <DetailRow key={field} term={eventFieldLabel(field, t)}>{eventValue(finding, field, cell, locale, t)}</DetailRow>)}</DetailList>
    </>}
    {metric.field !== null && points.some(({ value }) => typeof value === "number" && Number.isFinite(value)) && <SeriesChart
      cursor={cursor}
      durationAxis={metric.unit === "milliseconds"}
      format={(number, place) => formatMetric(number, metric.unit, place, t)}
      helpKey={metric.helpKey}
      hour={hour}
      labelKey={metric.labelKey}
      locale={locale}
      onCursor={onCursor}
      points={points}
      scale={metric.unit === "percent" || finding.logicalName === "health" ? "percent" : "nonnegative"}
      t={t}
      unit={finding.logicalName === "health" ? "%" : metricUnit(metric.unit, locale)}
    />}
  </aside>
}

function KindIcon({ kind }: { readonly kind: Finding["kind"] }): ReactNode {
  if (kind === "event") return <CircleAlert aria-hidden="true" className="text-event" size={15} />
  if (kind === "known_bad") return <Diamond aria-hidden="true" className="text-bad" size={15} />
  return <TriangleAlert aria-hidden="true" className="text-warn" size={15} />
}

export function categoryLabel(category: number, t: Translate): string {
  const key = ERROR_CATEGORIES[category]
  return key === undefined ? String(category) : t(key)
}

export function eventFieldLabel(field: string, t: Translate): string {
  const translated = t(`events.field.${field}`)
  return translated === `events.field.${field}` ? field : translated
}

export function eventValue(finding: Finding, field: string, cell: Cell, locale: Locale, t: Translate): string {
  if (field === "category") {
    const category = asNumber(cell)
    return category === null ? "—" : categoryLabel(category, t)
  }
  const enumKey = enumValueKey(finding.logicalName, field, asNumber(cell))
  if (enumKey !== null) return t(enumKey)
  if (identityField(field)) return identifier(cell)
  const number = asNumber(cell)
  if (number !== null && field.endsWith("_bytes")) return humanBytes(number, locale)
  if (number !== null && field.endsWith("_kb")) return `${compact(number, locale)} KiB`
  if (number !== null && field.endsWith("_ms")) return humanDuration(number, locale)
  if (number !== null && field.endsWith("_mbs")) return `${compact(number, locale)} MB/s`
  if (number !== null) return compact(number, locale)
  return rawText(cell) ?? "—"
}

function enumValueKey(logicalName: string, field: string, number: number | null): string | null {
  if (number === null) return null
  const name = (values: readonly string[], prefix: string) => values[number] === undefined ? null : `${prefix}.${values[number]}`
  if (logicalName === "pg_log_errors" && field === "severity") return name(["error", "fatal", "panic", "warning", "log"], `${EVENT_PREFIX}severity`)
  if (logicalName === "pg_log_checkpoints" && field === "phase") return name(["started", "completed", "too_frequent"], `${EVENT_PREFIX}checkpoint`)
  if (logicalName === "pg_log_autovacuum" && field === "kind") return name(["vacuum", "analyze"], `${EVENT_PREFIX}autovacuum`)
  if (logicalName === "pg_log_lock_waits" && field === "kind") return name(["waiting", "acquired"], `${EVENT_PREFIX}lock_wait`)
  if (logicalName === "pg_log_lifecycle" && field === "kind") return name(["crash", "shutdown", "ready"], `${EVENT_PREFIX}lifecycle`)
  if (logicalName === "pgbouncer_events" && field === "level") return name(["fatal", "error", "warning", "log", "debug", "noise"], `${EVENT_PREFIX}pgbouncer`)
  return null
}

export function formatMetric(value: number | null, unit: ReturnType<typeof findingMetric>["unit"], locale: Locale, t: Translate): string {
  if (value === null) return "—"
  if (unit === "percent") return humanPercent(value, locale)
  if (unit === "milliseconds") return humanDuration(value, locale)
  if (unit === "milliseconds_per_call") return humanDuration(value, locale, "milliseconds", t("unit.per_call"))
  if (unit === "bytes_per_second") return `${humanBytes(value, locale)}${t("unit.per_second")}`
  return compact(value, locale)
}

function metricUnit(unit: ReturnType<typeof findingMetric>["unit"], locale: Locale): string {
  if (unit === "percent") return "%"
  if (unit === "milliseconds") return ""
  if (unit === "milliseconds_per_call") return ""
  if (unit === "bytes_per_second") return "bytes/s"
  if (unit === "count") return locale === "ru" ? "количество" : "count"
  return locale === "ru" ? "значение" : "value"
}

function identityField(field: string): boolean {
  const name = field.toLowerCase()
  return name === "pid" || name === "oid" || name.endsWith("id") || name.endsWith("_id")
}
