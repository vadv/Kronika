import { CircleAlert, Diamond, Search, TriangleAlert } from "lucide-react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { Cell, DataRow, Finding, HourData } from "./api"
import { ChartOnly } from "./chart-visibility"
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
import { asNumber, compact, humanBytes, humanPercent, identifier, type Locale, rawText, shownMoment } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

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
  locale,
  onCursor,
  onFinding,
  onShowAll,
  resolution,
  resolved,
  scope,
  selected,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly history: readonly ChartPoint[]
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly onShowAll: () => void
  readonly resolution: FindingResolution
  readonly resolved: DataRow | null
  readonly scope: readonly Finding[] | null
  readonly selected: Finding | null
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const [filter, setFilter] = useState<Filter>("all")
  const [search, setSearch] = useState("")
  useEffect(() => {
    if (scope === null) return
    setFilter("all")
    setSearch("")
  }, [scope])
  const visible = useMemo(() => (scope ?? data.findings)
    .filter((finding) => {
      if (filter !== "all" && finding.kind !== filter) return false
      if (search.trim() === "") return true
      const selectedRow = selected !== null && findingKey(finding) === findingKey(selected) ? resolved : null
      const haystack = [
        findingCategory(finding, t), findingSource(finding, t), finding.logicalName,
        ...Object.values(selectedRow?.values ?? {}).map((cell) => rawText(cell) ?? ""),
      ].join("\n").toLocaleLowerCase(locale)
      return haystack.includes(search.trim().toLocaleLowerCase(locale))
    })
    .slice()
    .sort((left, right) => findingOrder(right, left)), [data.findings, filter, locale, resolved, scope, search, selected, t])
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
    <ChartOnly><Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={onCursor} onFinding={onFinding} primaryLane="health" shownAt={shownAt} t={t} /></ChartOnly>
    <section className="events-console">
      <header className="events-tools">
        <div className="event-filters" role="group" aria-label={t("events.filters")}>
          {(["all", "event", "known_bad"] as const).map((choice) => <button aria-pressed={filter === choice} key={choice} onClick={() => setFilter(choice)} type="button">{choice === "all" ? t("events.all") : t(`locator.${choice}`)}</button>)}
        </div>
        <span className="events-count">{t("events.count", { "shown": visible.length, total: original })}{omitted > 0 ? ` · ${t("events.omitted", { count: omitted })}` : ""}</span>
        {scope !== null && <button className="events-show-all" onClick={onShowAll} type="button">{t("events.show_all", { count: scope.length })}</button>}
        <label><Search aria-hidden="true" size={13} /><span>{t("events.search")}</span><input aria-label={t("events.search")} onChange={(event) => setSearch(event.target.value)} type="search" value={search} /></label>
      </header>
      <div className={`events-layout${active === null ? " events-list-only" : ""}`}>
        <div className="event-list" ref={list} role="list">
          {visible.length === 0 && <div className="table-empty">{t("events.empty")}</div>}
          <div className="event-list-body" style={{ height: virtual.getTotalSize() }}>
            {virtual.getVirtualItems().map((item) => {
              const finding = visible[item.index]
              if (finding === undefined) return null
              const pressed = active !== null && findingKey(active) === findingKey(finding)
              return <div className="event-item" key={findingKey(finding)} role="listitem" style={{ height: item.size, transform: `translateY(${item.start}px)` }}>
                <button aria-label={`${findingCategory(finding, t)} · ${findingSource(finding, t)} · ${time.timestamp(finding.timestamp)}`} aria-pressed={pressed} onClick={() => onFinding(finding)} type="button">
                  <KindIcon kind={finding.kind} />
                  <span><strong>{findingCategory(finding, t)}</strong><small>{findingSource(finding, t)}</small></span>
                  <time>{time.timestamp(finding.timestamp)}</time>
                </button>
              </div>
            })}
          </div>
        </div>
        {active !== null && <FindingDetail cursor={cursor} data={data} finding={active} history={history} hour={hour} locale={locale} onCursor={onCursor} resolution={resolution} row={resolved} t={t} />}
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
  return <aside className="event-detail" data-testid="event-detail">
    <header><KindIcon kind={finding.kind} /><div><span>{findingCategory(finding, t)}</span><h2>{findingSource(finding, t)}</h2></div><time>{time.timestamp(finding.timestamp)}</time></header>
    {resolution === "loading" && <p className="event-resolution">{t("events.loading_row")}</p>}
    {resolution === "unavailable" && <p className="event-resolution">{t("events.row_unavailable")}</p>}
    {resolution === "ready" && row !== null && <>
      {entity !== null && <p className="event-entity">{entity}</p>}
      {finding.category !== null && <p className="event-category">{t("events.category", { "category": categoryLabel(finding.category, t) })}</p>}
      {metric.field !== null && <section className="event-change" aria-label={t("events.change")}>
        <span>{metric.label}</span>
        <strong>{readings.previous === null
          ? formatMetric(readings.current, metric.unit, locale, t)
          : `${formatMetric(readings.previous, metric.unit, locale, t)} → ${formatMetric(readings.current, metric.unit, locale, t)}`}</strong>
        {metric.boundary !== null && <small>{t("events.boundary", { "boundary": metric.boundary })}</small>}
      </section>}
      <dl>{findingDetailFields(row, finding).map(([field, cell]) => <div key={field}><dt>{eventFieldLabel(field, t)}</dt><dd>{eventValue(finding, field, cell, locale, t)}</dd></div>)}</dl>
    </>}
    <ChartOnly>{metric.field !== null && points.some(({ value }) => typeof value === "number" && Number.isFinite(value)) && <SeriesChart
      cursor={cursor}
      format={(number, place) => formatMetric(number, metric.unit, place, t)}
      hour={hour}
      label={metric.label}
      locale={locale}
      onCursor={onCursor}
      points={points}
      scale={metric.unit === "percent" || finding.logicalName === "health" ? "percent" : "nonnegative"}
      unit={finding.logicalName === "health" ? "%" : metricUnit(metric.unit, locale)}
    />}</ChartOnly>
  </aside>
}

function KindIcon({ kind }: { readonly kind: Finding["kind"] }): ReactNode {
  if (kind === "event") return <CircleAlert aria-hidden="true" className="kind-event" size={15} />
  if (kind === "known_bad") return <Diamond aria-hidden="true" className="kind-known_bad" size={15} />
  return <TriangleAlert aria-hidden="true" className="kind-spike" size={15} />
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
  if (number !== null && field.endsWith("_ms")) return `${compact(number, locale)}${t("unit.ms")}`
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
  if (unit === "milliseconds") return `${compact(value, locale)}${t("unit.ms")}`
  if (unit === "milliseconds_per_call") return `${compact(value, locale)}${t("unit.ms")}${t("unit.per_call")}`
  if (unit === "bytes_per_second") return `${humanBytes(value, locale)}${t("unit.per_second")}`
  return compact(value, locale)
}

function metricUnit(unit: ReturnType<typeof findingMetric>["unit"], locale: Locale): string {
  if (unit === "percent") return "%"
  if (unit === "milliseconds") return "ms"
  if (unit === "milliseconds_per_call") return "ms/call"
  if (unit === "bytes_per_second") return "bytes/s"
  if (unit === "count") return locale === "ru" ? "количество" : "count"
  return locale === "ru" ? "значение" : "value"
}

function identityField(field: string): boolean {
  const name = field.toLowerCase()
  return name === "pid" || name === "oid" || name === "starttime" || name.endsWith("id") || name.endsWith("_id")
}
