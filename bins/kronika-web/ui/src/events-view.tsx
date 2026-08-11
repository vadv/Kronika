import { CircleAlert, Diamond, Search, TriangleAlert } from "lucide-react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { DataRow, Finding, HourData } from "./api"
import type { Translate } from "./help"
import { asNumber, formatUtc, rawText } from "./model"
import { Timeline } from "./timeline"

type Filter = "all" | Finding["kind"]

const ERROR_CATEGORIES = [
  "events.category.lock", "events.category.constraint", "events.category.serialization",
  "events.category.timeout", "events.category.resource", "events.category.data_corruption",
  "events.category.system", "events.category.connection", "events.category.auth",
  "events.category.syntax", "events.category.other",
] as const

export function EventsView({
  cursor,
  data,
  hour,
  onCursor,
  onFinding,
  onShowAll,
  resolve,
  scope,
  selected,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly hour: number
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly onShowAll: () => void
  readonly resolve: (finding: Finding) => DataRow | null
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
  const visible = useMemo(() => (scope ?? data.findings).filter((finding) => {
    if (filter !== "all" && finding.kind !== filter) return false
    if (search.trim() === "") return true
    const row = resolve(finding)
    const haystack = [finding.kind, finding.logicalName, ...Object.values(row?.values ?? {}).map((cell) => rawText(cell) ?? "")].join("\n").toLocaleLowerCase()
    return haystack.includes(search.toLocaleLowerCase())
  }), [data.findings, filter, resolve, scope, search])
  const active = selected !== null && visible.some((finding) => findingKey(finding) === findingKey(selected)) ? selected : visible[0] ?? null
  const row = active === null ? null : resolve(active)
  const list = useRef<HTMLDivElement>(null)
  const virtual = useVirtualizer({ count: visible.length, estimateSize: () => scope === null ? 44 : 72, getScrollElement: () => list.current, overscan: 12 })
  useEffect(() => {
    if (active === null) return
    const index = visible.findIndex((finding) => findingKey(finding) === findingKey(active))
    if (index >= 0) virtual.scrollToIndex(index, { align: "auto" })
  }, [active, virtual, visible])
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={[]} memory={[]} onCursor={onCursor} onFinding={onFinding} pressure={[]} t={t} />
    <section className="events-console">
      <header className="events-tools">
        <div className="event-filters" role="group">
          {(["all", "event", "known_bad", "spike"] as const).map((choice) => <button aria-pressed={filter === choice} key={choice} onClick={() => setFilter(choice)} type="button">{choice === "all" ? t("events.all") : t(`locator.${choice}`)}</button>)}
        </div>
        {scope !== null && <button className="events-show-all" onClick={onShowAll} type="button">{t("events.show_all", { count: scope.length })}</button>}
        <label><Search aria-hidden="true" size={13} /><span>{t("events.search")}</span><input onChange={(event) => setSearch(event.target.value)} type="search" value={search} /></label>
      </header>
      <div className="events-layout">
        <div className="event-list" ref={list} role="list">
          {visible.length === 0 && <div className="table-empty">{t("events.empty")}</div>}
          <div className="event-list-body" style={{ height: virtual.getTotalSize() }}>
            {virtual.getVirtualItems().map((item) => {
              const finding = visible[item.index]
              if (finding === undefined) return null
              return <div className="event-item" key={findingKey(finding)} role="listitem" style={{ height: item.size, transform: `translateY(${item.start}px)` }}>
                <button aria-pressed={active !== null && findingKey(active) === findingKey(finding)} onClick={() => onFinding(finding)} type="button">
                  <KindIcon kind={finding.kind} />
                  <span><strong>{t(`locator.${finding.kind}`)}</strong><small>{finding.logicalName}{scope !== null && <code>{locatorText(finding, t)}</code>}</small></span>
                  <time>{formatUtc(finding.timestamp)}</time>
                </button>
              </div>
            })}
          </div>
        </div>
        <aside className="event-detail">
          {active === null
            ? <p className="table-empty">{t("events.empty")}</p>
            : <>
              <header><KindIcon kind={active.kind} /><div><span>{t(`locator.${active.kind}`)}</span><h2>{active.logicalName}</h2></div><time>{formatUtc(active.timestamp)}</time></header>
              {active.category !== null && <p className="event-category">{t("events.category", { category: categoryLabel(active.category, t) })}</p>}
              {row === null
                ? <p className="table-empty">{t("events.row_unavailable")}</p>
                : <dl>{Object.entries(row.values).map(([field, cell]) => <div key={field}><dt>{field}</dt><dd>{eventValue(field, cell)}</dd></div>)}</dl>}
            </>}
        </aside>
      </div>
    </section>
  </>
}

function KindIcon({ kind }: { readonly kind: Finding["kind"] }): ReactNode {
  if (kind === "event") return <CircleAlert aria-hidden="true" className="kind-event" size={15} />
  if (kind === "known_bad") return <Diamond aria-hidden="true" className="kind-known_bad" size={15} />
  return <TriangleAlert aria-hidden="true" className="kind-spike" size={15} />
}

function findingKey(finding: Finding): string {
  return `${finding.segmentId}:${finding.typeId}:${finding.rowOrdinal}:${finding.fieldOrdinal}:${finding.timestamp}:${finding.kind}`
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

function eventValue(field: string, cell: DataRow["values"][string]): string {
  if (["starttime", "backend_start", "xact_start", "query_start", "state_change", "waitstart", "stats_since"].includes(field)) {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : formatUtc(timestamp)
  }
  return rawText(cell) ?? "—"
}
