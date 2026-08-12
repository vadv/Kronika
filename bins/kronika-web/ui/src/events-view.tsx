import { CircleAlert, Diamond, Search, TriangleAlert } from "lucide-react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { Cell, DataRow, Finding, HourData } from "./api"
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
import { asNumber, formatUtc, humanBytes, type Locale, rawText, shownMoment } from "./model"
import type { ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

type Filter = "all" | Finding["kind"]
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
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={onCursor} onFinding={onFinding} primaryLane="health" shownAt={shownAt} t={t} />
    <section className="events-console">
      <header className="events-tools">
        <div className="event-filters" role="group" aria-label={t("events.filters")}>
          {(["all", "event", "known_bad", "spike"] as const).map((choice) => <button aria-pressed={filter === choice} key={choice} onClick={() => setFilter(choice)} type="button">{choice === "all" ? t("events.all") : t(`locator.${choice}`)}</button>)}
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
                <button aria-label={`${findingCategory(finding, t)} · ${findingSource(finding, t)} · ${formatUtc(finding.timestamp)}`} aria-pressed={pressed} onClick={() => onFinding(finding)} type="button">
                  <KindIcon kind={finding.kind} />
                  <span><strong>{findingCategory(finding, t)}</strong><small>{findingSource(finding, t)}</small></span>
                  <time>{formatUtc(finding.timestamp)}</time>
                </button>
              </div>
            })}
          </div>
        </div>
        {active !== null && <FindingDetail data={data} finding={active} history={history} locale={locale} resolution={resolution} row={resolved} t={t} />}
      </div>
    </section>
  </>
}

function FindingDetail({ data, finding, history, locale, resolution, row, t }: {
  readonly data: HourData
  readonly finding: Finding
  readonly history: readonly ChartPoint[]
  readonly locale: Locale
  readonly resolution: FindingResolution
  readonly row: DataRow | null
  readonly t: Translate
}) {
  const metric = findingMetric(finding, t)
  const points = history.length === 0 ? findingHistory(finding, row === null ? [] : [row], data) : history
  const readings = findingReadings(finding, row, points, data)
  const entity = findingEntity(row)
  return <aside className="event-detail" data-testid="event-detail">
    <header><KindIcon kind={finding.kind} /><div><span>{findingCategory(finding, t)}</span><h2>{findingSource(finding, t)}</h2></div><time>{formatUtc(finding.timestamp)}</time></header>
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
    <details className="event-technical"><summary>{t("events.technical")}</summary><code>{locatorText(finding, t)}</code></details>
  </aside>
}

function KindIcon({ kind }: { readonly kind: Finding["kind"] }): ReactNode {
  if (kind === "event") return <CircleAlert aria-hidden="true" className="kind-event" size={15} />
  if (kind === "known_bad") return <Diamond aria-hidden="true" className="kind-known_bad" size={15} />
  return <TriangleAlert aria-hidden="true" className="kind-spike" size={15} />
}

export function locatorText(finding: Finding, t: Translate): string {
  return t("events.locator", {
    field: finding.fieldOrdinal,
    row: finding.rowOrdinal,
    segment: finding.segmentId,
    type: finding.typeId,
  })
}

export function categoryLabel(category: number, t: Translate): string {
  const key = ERROR_CATEGORIES[category]
  return key === undefined ? String(category) : t(key)
}

export function eventFieldLabel(field: string, t: Translate): string {
  const translated = t(`events.field.${field}`)
  return translated === `events.field.${field}` ? field : translated
}

function eventValue(finding: Finding, field: string, cell: Cell, locale: Locale, t: Translate): string {
  if (field === "category") {
    const category = asNumber(cell)
    return category === null ? "—" : categoryLabel(category, t)
  }
  const enumKey = enumValueKey(finding.logicalName, field, asNumber(cell))
  if (enumKey !== null) return t(enumKey)
  const number = asNumber(cell)
  if (number !== null && field.endsWith("_bytes")) return humanBytes(number, locale)
  if (number !== null && field.endsWith("_kb")) return `${exactNumber(number, locale)} KiB`
  if (number !== null && field.endsWith("_ms")) return `${exactNumber(number, locale)}${t("unit.ms")}`
  if (number !== null && field.endsWith("_mbs")) return `${exactNumber(number, locale)} MB/s`
  if (number !== null) return exactNumber(number, locale)
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

function formatMetric(value: number | null, unit: ReturnType<typeof findingMetric>["unit"], locale: Locale, t: Translate): string {
  if (value === null) return "—"
  if (unit === "percent") return `${exactNumber(value, locale)}%`
  if (unit === "milliseconds") return `${exactNumber(value, locale)}${t("unit.ms")}`
  if (unit === "milliseconds_per_call") return `${exactNumber(value, locale)}${t("unit.ms")}${t("unit.per_call")}`
  if (unit === "bytes_per_second") return `${humanBytes(value, locale)}${t("unit.per_second")}`
  return exactNumber(value, locale)
}

function exactNumber(value: number, locale: Locale): string {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 6 }).format(value)
}
