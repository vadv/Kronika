import { useEffect, useMemo, useState } from "react"

import type { LanePoint } from "./api"
import { ChartOnly } from "./chart-visibility"
import { LabelHelp, type Translate } from "./help"
import { humanBytes, humanPercent, measure, type Locale } from "./model"
import { readingAt, SeriesChart, type ChartPoint } from "./series-chart"

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

interface ChartChoice {
  readonly cell: Cell
  readonly column: (typeof COLUMNS)[number]
  readonly key: string
  readonly points: readonly ChartPoint[]
  readonly resource: Resource
  readonly second: readonly ChartPoint[]
}

export function UseTable({
  cursor,
  hour,
  lanePoints,
  locale,
  onCursor,
  t,
}: {
  readonly cursor: number
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const shown = useMemo(() => RESOURCES.filter((resource) => COLUMNS.some((column) => {
    const cell = resource[column]
    return cell !== null && lanePoints.some((point) => point.lane === cell.lane)
  })), [lanePoints])
  const choices = useMemo(() => buildUseChartChoices(lanePoints, shown), [lanePoints, shown])
  const [selectedKey, setSelectedKey] = useState(choices[0]?.key ?? "")
  useEffect(() => {
    if (choices.some((choice) => choice.key === selectedKey)) return
    setSelectedKey(choices[0]?.key ?? "")
  }, [choices, selectedKey])
  const selected = choices.find((choice) => choice.key === selectedKey) ?? choices[0]
  if (shown.length === 0) return null
  return <section aria-label={t("use.title")} className="use-table" data-testid="use-table">
    <header>
      <span>{t("use.resource")}</span>
      {COLUMNS.map((column) => <span key={column}>{t(`use.${column}`)}</span>)}
    </header>
    {shown.map((resource) => <div className="use-row" key={resource.key}>
      <span className="use-resource">{t(`use.resource.${resource.key}`)}</span>
      {COLUMNS.map((column) => {
        const cell = resource[column]
        const choice = choices.find((candidate) => candidate.resource.key === resource.key && candidate.column === column)
        if (cell === null || choice === undefined) return <span className="use-cell use-absent" key={column} title={t("use.not_measured")}>—</span>
        const primary = currentReading(choice.points, cursor, locale, cell.kind, t("unit.per_second"))
        const secondary = cell.second === undefined ? null : currentReading(choice.second, cursor, locale, cell.kind, t("unit.per_second"))
        const values = [
          `${t(`use.lane.${cell.lane}`)}: ${primary}`,
          ...(cell.second === undefined ? [] : [`${t(`use.lane.${cell.second}`)}: ${secondary}`]),
        ]
        return <span className="use-cell" key={column}>
          <button
            aria-label={values.join("; ")}
            aria-pressed={selected?.key === choice.key}
            className="use-cell-action"
            data-testid={`use-metric-${choice.key}`}
            onClick={() => setSelectedKey(choice.key)}
            type="button"
          >
            <span>{t(`use.lane.${cell.lane}`)}</span>
            <strong>{[primary, ...(secondary === null ? [] : [secondary])].join(" · ")}</strong>
          </button>
          <LabelHelp helpKey={useLaneHelp(cell.lane)} iconOnly labelKey={`use.lane.${cell.lane}`} t={t} />
        </span>
      })}
    </div>)}
    <ChartOnly>{selected !== undefined && <section className="use-history" data-testid="use-history">
      <SeriesChart
        cursor={cursor}
        empty={t("status.no_data")}
        format={(stored, place) => reading(stored, place, selected.cell.kind, t("unit.per_second"))}
        helpKey={useLaneHelp(selected.cell.lane)}
        hour={hour}
        labelKey={`use.lane.${selected.cell.lane}`}
        locale={locale}
        onCursor={onCursor}
        points={selected.points}
        scale={selected.cell.kind === "share" ? "percent" : "nonnegative"}
        second={selected.second.length === 0 ? undefined : selected.second}
        secondHelpKey={selected.cell.second === undefined ? undefined : useLaneHelp(selected.cell.second)}
        secondLabelKey={selected.cell.second === undefined ? undefined : `use.lane.${selected.cell.second}`}
        t={t}
        unit={cellUnit(selected.cell.kind, locale)}
      />
    </section>}</ChartOnly>
  </section>
}

const COLUMNS = ["utilisation", "saturation", "errors"] as const

const USE_LANE_HELP: Readonly<Record<string, string>> = {
  cpu_busy: "lane.cpu_busy.help",
  cpu_stall: "lane.cpu_stall.help",
  disk_busy: "system.field.device_busy.help",
  disk_queue: "system.field.average_queue.help",
  mem_oom: "system.metric.oom_kill.help",
  mem_swap: "use.lane.mem_swap.help",
  memory: "lane.memory.help",
  net_drop: "system.metric.network_drops.help",
  net_errors: "system.metric.network_errors.help",
  net_rx: "system.metric.network_rx.help",
  net_tx: "system.metric.network_tx.help",
}

function useLaneHelp(lane: string): string {
  return USE_LANE_HELP[lane] ?? "chart.metric.help"
}

export function availableUseChartKeys(lanePoints: readonly LanePoint[]): readonly string[] {
  return buildUseChartChoices(lanePoints, RESOURCES).map((choice) => choice.key)
}

function buildUseChartChoices(lanePoints: readonly LanePoint[], resources: readonly Resource[]): readonly ChartChoice[] {
  return resources.flatMap((resource) => COLUMNS.flatMap((column) => {
    const cell = resource[column]
    if (cell === null) return []
    const points = seriesOf(lanePoints, cell.lane)
    if (!points.some((point) => point.value !== null && Number.isFinite(point.value))) return []
    return [{
      cell,
      column,
      key: `${resource.key}-${column}`,
      points,
      resource,
      second: cell.second === undefined ? [] : seriesOf(lanePoints, cell.second),
    }]
  }))
}

function currentReading(points: readonly ChartPoint[], cursor: number, locale: Locale, kind: Cell["kind"], perSecond: string): string {
  const stored = readingAt(points, cursor)
  return stored === null ? "—" : reading(stored, locale, kind, perSecond)
}

function cellUnit(kind: Cell["kind"], locale: Locale): string {
  if (kind === "share") return "%"
  if (kind === "bytes") return locale === "ru" ? "байты/с" : "bytes/s"
  if (kind === "rate") return locale === "ru" ? "1/с" : "1/s"
  return locale === "ru" ? "количество" : "count"
}

function seriesOf(lanePoints: readonly LanePoint[], lane: string): readonly ChartPoint[] {
  return lanePoints
    .filter((point) => point.lane === lane)
    .map((point) => ({ segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }))
}

export function reading(value: number, locale: Locale, kind: Cell["kind"], perSecond: string): string {
  if (kind === "share") return humanPercent(value, locale)
  if (kind === "bytes") return humanBytes(value, locale, perSecond)
  if (kind === "count") return measure(value, locale)
  return measure(value, locale, perSecond)
}
