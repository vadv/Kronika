import type { LanePoint } from "./api"
import type { Translate } from "./help"
import { humanBytes, measure, type Locale } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"

const RESOURCES: readonly Resource[] = [
  {
    key: "cpu",
    utilisation: { lane: "cpu_busy", kind: "share" },
    saturation: { lane: "cpu_stall", kind: "share" },
    errors: null,
  },
  {
    key: "memory",
    utilisation: { lane: "memory", kind: "share" },
    saturation: { lane: "mem_swap", kind: "rate" },
    errors: { lane: "mem_oom", kind: "rate" },
  },
  {
    key: "disk",
    utilisation: { lane: "disk_busy", kind: "share" },
    saturation: { lane: "disk_queue", kind: "count" },
    errors: null,
  },
  {
    key: "network",
    utilisation: { lane: "net_rx", kind: "bytes", second: "net_tx" },
    saturation: { lane: "net_drop", kind: "rate" },
    errors: { lane: "net_errors", kind: "rate" },
  },
]

interface Cell {
  readonly lane: string
  readonly kind: "share" | "rate" | "count" | "bytes"
  readonly second?: string
}

interface Resource {
  readonly key: string
  readonly utilisation: Cell | null
  readonly saturation: Cell | null
  readonly errors: Cell | null
}

export function UseTable({
  cursor,
  hour,
  lanePoints,
  locale,
  t,
}: {
  readonly cursor: number
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly t: Translate
}) {
  const shown = RESOURCES.filter((resource) => COLUMNS.some((column) => {
    const cell = resource[column]
    return cell !== null && lanePoints.some((point) => point.lane === cell.lane)
  }))
  if (shown.length === 0) return null
  return <section aria-label={t("use.title")} className="use-table" data-testid="use-table">
    <header>
      <span>{t("use.resource")}</span>
      {COLUMNS.map((column) => <span key={column}>{t(`use.${column}`)}</span>)}
    </header>
    {shown.map((resource) => <div className="use-row" key={resource.key}>
      <span className="use-resource">{t(`use.resource.${resource.key}`)}</span>
      {COLUMNS.map((column) => <UseCell
        cell={resource[column]}
        cursor={cursor}
        hour={hour}
        key={column}
        lanePoints={lanePoints}
        locale={locale}
        t={t}
      />)}
    </div>)}
  </section>
}

const COLUMNS = ["utilisation", "saturation", "errors"] as const

function UseCell({
  cell,
  cursor,
  hour,
  lanePoints,
  locale,
  t,
}: {
  readonly cell: Cell | null
  readonly cursor: number
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly t: Translate
}) {
  if (cell === null) return <span className="use-cell use-absent" title={t("use.not_measured")}>—</span>
  const points = seriesOf(lanePoints, cell.lane)
  const second = cell.second === undefined ? [] : seriesOf(lanePoints, cell.second)
  const format = (value: number, place: Locale) => reading(value, place, cell.kind, t("unit.per_second"))
  return <span className="use-cell">
    <SeriesChart
      cursor={cursor}
      empty={t("status.no_data")}
      format={format}
      hour={hour}
      label={t(`use.lane.${cell.lane}`)}
      locale={locale}
      points={points}
      scale={cell.kind === "share" ? "percent" : cell.kind === "count" ? "count" : "auto"}
      second={second.length === 0 ? undefined : second}
    />
  </span>
}

function seriesOf(lanePoints: readonly LanePoint[], lane: string): readonly ChartPoint[] {
  return lanePoints
    .filter((point) => point.lane === lane)
    .map((point) => ({ segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }))
}

export function reading(value: number, locale: Locale, kind: Cell["kind"], perSecond: string): string {
  if (kind === "share") return measure(value, locale, "%")
  if (kind === "bytes") return humanBytes(value, locale, perSecond)
  if (kind === "count") return measure(value, locale)
  return measure(value, locale, perSecond)
}
