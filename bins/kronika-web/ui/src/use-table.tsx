import { useMemo } from "react"

import type { LanePoint } from "./api"
import { LabelHelp, type Translate } from "./help"
import { humanBytes, humanPercent, measure, type Locale } from "./model"
import { readingAt, type ChartPoint } from "./series-chart"

export type UseResourceKey = "cpu" | "memory" | "disk" | "network"

export interface UseCell {
  readonly lane: string
  readonly kind: "share" | "rate" | "count" | "bytes"
  readonly second?: string
}

export interface UseResource {
  readonly key: UseResourceKey
  readonly utilisation: UseCell | null
  readonly saturation: UseCell | null
  readonly errors: UseCell | null
}

export const USE_RESOURCES: readonly UseResource[] = [
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

export const USE_COLUMNS = ["utilisation", "saturation", "errors"] as const

// The grid is the host overview: a row is one resource, a click chooses the
// resource whose submetrics the detail area below charts and lists. Cells stay
// static readings; the per-cell chart moved to the submetric chips.
export function UseTable({
  canOpen,
  cursor,
  lanePoints,
  locale,
  onSelect,
  selected,
  t,
}: {
  readonly canOpen?: (resource: UseResourceKey) => boolean
  readonly cursor: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly onSelect: (resource: UseResourceKey) => void
  readonly selected: UseResourceKey | null
  readonly t: Translate
}) {
  const shown = useMemo(() => shownUseResources(lanePoints), [lanePoints])
  if (shown.length === 0) return null
  return <section aria-label={t("use.title")} className="use-table" data-testid="use-table" role="table">
    <header className="grid grid-cols-[minmax(96px,130px)_repeat(3,minmax(0,1fr))] border-b border-line2 text-xs uppercase text-fg3 [&>span]:px-2 [&>span]:py-[5px] max-[760px]:grid-cols-[80px_repeat(3,minmax(0,1fr))] max-[760px]:[&>span]:px-[5px]" role="row">
      <span role="columnheader">{t("use.resource")}</span>
      {USE_COLUMNS.map((column) => <span key={column} role="columnheader">{t(`use.${column}`)}</span>)}
    </header>
    {shown.map((resource) => {
      // A row whose group has no metrics to chart stays honest: no pointer, no
      // dead click.
      const openable = canOpen?.(resource.key) ?? true
      return <div
        aria-disabled={!openable}
        aria-selected={selected === resource.key}
        className="use-row"
        data-testid={`use-row-${resource.key}`}
        key={resource.key}
        onClick={openable ? () => onSelect(resource.key) : undefined}
        onKeyDown={openable ? (event) => {
          if (event.key !== "Enter" && event.key !== " ") return
          event.preventDefault()
          onSelect(resource.key)
        } : undefined}
        role="row"
        tabIndex={openable ? 0 : undefined}
      >
      <span className="use-resource self-center px-2 py-[7px] text-sm uppercase text-fg2 max-[760px]:px-[5px]" role="cell">{t(`use.resource.${resource.key}`)}</span>
      {USE_COLUMNS.map((column) => {
        const cell = resource[column]
        if (cell === null || !laneHasReading(lanePoints, cell.lane)) {
          return <span className="use-cell relative flex min-w-0 items-center justify-center text-fg4" key={column} role="cell" title={t("use.not_measured")}>—</span>
        }
        const primary = currentLaneReading(lanePoints, cell.lane, cursor, locale, cell.kind, t("unit.per_second"))
        const secondary = cell.second === undefined ? null : currentLaneReading(lanePoints, cell.second, cursor, locale, cell.kind, t("unit.per_second"))
        return <span className="use-cell relative min-w-0" key={column} role="cell">
          <span className="use-cell-body flex min-h-[38px] items-baseline justify-between gap-[7px] py-1.5 pl-2 pr-[26px] text-fg3 max-[760px]:min-w-0 max-[760px]:flex-col max-[760px]:items-start max-[760px]:gap-0.5 max-[760px]:px-[5px] max-[760px]:[&_strong]:max-w-full max-[760px]:[&_strong]:overflow-hidden max-[760px]:[&_strong]:text-ellipsis [&>span]:min-w-0 [&>span]:overflow-hidden [&>span]:text-ellipsis [&>span]:whitespace-nowrap [&>span]:text-xs [&_strong]:flex-none [&_strong]:whitespace-nowrap [&_strong]:text-sm [&_strong]:font-medium [&_strong]:tabular-nums [&_strong]:text-fg2">
            <span>{t(`use.lane.${cell.lane}`)}</span>
            <strong>{[primary, ...(secondary === null ? [] : [secondary])].join(" · ")}</strong>
          </span>
          <LabelHelp helpKey={useLaneHelp(cell.lane)} iconOnly labelKey={`use.lane.${cell.lane}`} t={t} />
        </span>
      })}
    </div>
    })}
  </section>
}

export function shownUseResources(lanePoints: readonly LanePoint[]): readonly UseResource[] {
  return USE_RESOURCES.filter((resource) => USE_COLUMNS.some((column) => {
    const cell = resource[column]
    return cell !== null && laneHasReading(lanePoints, cell.lane)
  }))
}

export function laneHasReading(lanePoints: readonly LanePoint[], lane: string): boolean {
  return lanePoints.some((point) => point.lane === lane && point.value !== null && Number.isFinite(point.value))
}

export function laneSeriesPoints(lanePoints: readonly LanePoint[], lane: string): readonly ChartPoint[] {
  return lanePoints
    .filter((point) => point.lane === lane)
    .map((point) => ({ segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }))
}

export function currentLaneReading(
  lanePoints: readonly LanePoint[],
  lane: string,
  cursor: number,
  locale: Locale,
  kind: UseCell["kind"],
  perSecond: string,
): string {
  const stored = readingAt(laneSeriesPoints(lanePoints, lane), cursor)
  return stored === null ? "—" : reading(stored, locale, kind, perSecond)
}

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

export function useLaneHelp(lane: string): string {
  return USE_LANE_HELP[lane] ?? "chart.metric.help"
}

export function reading(value: number, locale: Locale, kind: UseCell["kind"], perSecond: string): string {
  if (kind === "share") return humanPercent(value, locale)
  if (kind === "bytes") return humanBytes(value, locale, perSecond)
  if (kind === "count") return measure(value, locale)
  return measure(value, locale, perSecond)
}
