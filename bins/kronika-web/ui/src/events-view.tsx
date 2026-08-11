import { CircleAlert, Diamond, Search, TriangleAlert } from "lucide-react"
import { useMemo, useState, type ReactNode } from "react"

import type { DataRow, Finding, HourData } from "./api"
import type { Translate } from "./help"
import { formatUtc, rawText } from "./model"
import { Timeline } from "./timeline"

type Filter = "all" | Finding["kind"]

export function EventsView({
  cursor,
  data,
  hour,
  onCursor,
  onFinding,
  resolve,
  selected,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly hour: number
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly resolve: (finding: Finding) => DataRow | null
  readonly selected: Finding | null
  readonly t: Translate
}) {
  const [filter, setFilter] = useState<Filter>("all")
  const [search, setSearch] = useState("")
  const visible = useMemo(() => data.findings.filter((finding) => {
    if (filter !== "all" && finding.kind !== filter) return false
    if (search.trim() === "") return true
    const row = resolve(finding)
    const haystack = [finding.kind, logicalName(data, finding), ...Object.values(row?.values ?? {}).map((cell) => rawText(cell) ?? "")].join("\n").toLocaleLowerCase()
    return haystack.includes(search.toLocaleLowerCase())
  }), [data, filter, resolve, search])
  const active = selected ?? visible[0] ?? null
  const row = active === null ? null : resolve(active)
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={data.load} memory={data.memory} onCursor={onCursor} onFinding={onFinding} pressure={data.pressure} t={t} />
    <section className="events-console">
      <header className="events-tools">
        <div className="event-filters" role="group">
          {(["all", "event", "known_bad", "spike"] as const).map((choice) => <button aria-pressed={filter === choice} key={choice} onClick={() => setFilter(choice)} type="button">{choice === "all" ? t("events.all") : t(`locator.${choice}`)}</button>)}
        </div>
        <label><Search aria-hidden="true" size={13} /><span>{t("events.search")}</span><input onChange={(event) => setSearch(event.target.value)} type="search" value={search} /></label>
      </header>
      <div className="events-layout">
        <ol className="event-list">
          {visible.length === 0 && <li className="table-empty">{t("events.empty")}</li>}
          {visible.map((finding) => <li key={findingKey(finding)}>
            <button aria-pressed={active !== null && findingKey(active) === findingKey(finding)} onClick={() => onFinding(finding)} type="button">
              <KindIcon kind={finding.kind} />
              <span><strong>{t(`locator.${finding.kind}`)}</strong><small>{logicalName(data, finding)}</small></span>
              <time>{formatUtc(finding.timestamp)}</time>
            </button>
          </li>)}
        </ol>
        <aside className="event-detail">
          {active === null
            ? <p className="table-empty">{t("events.empty")}</p>
            : <>
              <header><KindIcon kind={active.kind} /><div><span>{t(`locator.${active.kind}`)}</span><h2>{logicalName(data, active)}</h2></div><time>{formatUtc(active.timestamp)}</time></header>
              {active.category !== null && <p className="event-category">{t("events.category", { category: active.category })}</p>}
              {row === null
                ? <p className="table-empty">{t("events.row_unavailable")}</p>
                : <dl>{Object.entries(row.values).map(([field, cell]) => <div key={field}><dt>{field}</dt><dd>{rawText(cell) ?? "—"}</dd></div>)}</dl>}
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

function logicalName(data: HourData, finding: Finding): string {
  const row = Object.entries(data.sections).find(([, rows]) => rows.some((candidate) => candidate.typeId === finding.typeId))
  return row?.[0] ?? "health"
}

function findingKey(finding: Finding): string {
  return `${finding.segmentId}:${finding.typeId}:${finding.rowOrdinal}:${finding.fieldOrdinal}:${finding.timestamp}:${finding.kind}`
}
