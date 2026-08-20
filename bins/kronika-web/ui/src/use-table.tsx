import { ChevronDown, ChevronRight } from "lucide-react"
import { useMemo, type ReactNode } from "react"

import type { LanePoint } from "./api"
import { LabelHelp, type Translate } from "./help"
import { humanBytes, humanPercent, measure, type Locale } from "./model"
import { readingAt, type ChartPoint } from "./series-chart"
import { SparkCell } from "./spark-cell"
import { sparkScaleMax } from "./spark"

export type UseResourceKey = "cpu" | "memory" | "disk" | "network"
export type LedgerKey = UseResourceKey | "cgroups"

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

// The ledger IS the Host page: a row is one resource, its cells carry the
// hour's shape with the reading at the cursor, and expanding a row discloses
// the group's chart, metric chips and entity tables in place. Expansion is
// disclosure, not navigation — several rows open side by side.
export function UseTable({
  cgroups,
  cursor,
  expanded,
  hour,
  lanePoints,
  locale,
  onToggle,
  renderExpansion,
  withContent,
  t,
}: {
  readonly cgroups: boolean
  readonly cursor: number
  readonly expanded: ReadonlySet<LedgerKey>
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly onToggle: (key: LedgerKey) => void
  readonly renderExpansion: (key: LedgerKey) => ReactNode
  readonly withContent: ReadonlySet<UseResourceKey>
  readonly t: Translate
}) {
  // A row exists when the hour carries anything for its resource: lane
  // readings, group metrics or entity tables. Cells without a lane stay "—".
  const byLane = useMemo(() => lanePointsByLane(lanePoints), [lanePoints])
  const shown = useMemo(() => USE_RESOURCES.filter((resource) =>
    withContent.has(resource.key) || USE_COLUMNS.some((column) => {
      const cell = resource[column]
      return cell !== null && seriesHasReading(byLane.get(cell.lane) ?? [])
    })), [byLane, withContent])
  const end = hour + 3_600_000_000
  if (shown.length === 0 && !cgroups) return null
  return <section aria-label={t("use.title")} className="use-table" data-testid="use-table">
    <header className="grid grid-cols-[minmax(96px,130px)_repeat(3,minmax(0,1fr))] border-b border-line2 text-xs font-medium text-fg3 [&>span]:px-2 [&>span]:py-[5px] max-[760px]:grid-cols-[80px_repeat(3,minmax(0,1fr))] max-[760px]:[&>span]:px-[5px]">
      <span>{t("use.resource")}</span>
      {USE_COLUMNS.map((column) => <span key={column}>{t(`use.${column}`)}</span>)}
    </header>
    {shown.map((resource) => {
      const open = expanded.has(resource.key)
      return <div data-testid={`use-group-${resource.key}`} key={resource.key}>
        <div className="use-row" data-expanded={open || undefined} data-testid={`use-row-${resource.key}`} onClick={(event) => { if ((event.target as HTMLElement).closest("button, a, [role=tooltip]") !== null) return; onToggle(resource.key) }}>
          <button aria-expanded={open} className="use-resource flex cursor-pointer items-center gap-1 self-stretch border-0 bg-transparent px-2 py-[7px] text-left font-sans text-sm font-medium text-fg2 max-[760px]:px-[5px]" data-testid={`use-toggle-${resource.key}`} onClick={() => onToggle(resource.key)} type="button">
            {open ? <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} /> : <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />}
            {t(`use.resource.${resource.key}`)}
          </button>
          {USE_COLUMNS.map((column) => {
            const cell = resource[column]
            const points = cell === null ? [] : byLane.get(cell.lane) ?? []
            if (cell === null || !seriesHasReading(points)) {
              return <span className="use-cell relative flex min-w-0 items-center justify-center text-fg4" key={column} title={t("use.not_measured")}>—</span>
            }
            const second = cell.second === undefined ? undefined : byLane.get(cell.second) ?? []
            const primary = seriesReading(points, cursor, locale, cell.kind, t("unit.per_second"))
            const secondary = second === undefined ? null : seriesReading(second, cursor, locale, cell.kind, t("unit.per_second"))
            return <span className="use-cell relative min-w-0 py-1.5 pl-2 pr-[24px] max-[760px]:px-[5px]" key={column}>
              <span className="flex items-baseline justify-between gap-[7px] [&>span]:min-w-0 [&>span]:overflow-hidden [&>span]:text-ellipsis [&>span]:whitespace-nowrap [&>span]:font-sans [&>span]:text-xs [&>span]:text-fg3 [&_strong]:flex-none [&_strong]:whitespace-nowrap [&_strong]:font-mono [&_strong]:text-xs [&_strong]:font-normal [&_strong]:tabular-nums [&_strong]:text-fg2">
                <span>{t(`use.lane.${cell.lane}`)}</span>
                <strong>{[primary, ...(secondary === null ? [] : [secondary])].join(" · ")}</strong>
              </span>
              <SparkCell cursor={cursor} end={end} hour={hour} max={sparkScaleMax(cell.kind, [points, ...(second === undefined ? [] : [second])])} points={points} second={second} />
              <LabelHelp helpKey={useLaneHelp(cell.lane)} iconOnly labelKey={`use.lane.${cell.lane}`} t={t} />
            </span>
          })}
        </div>
        {open && <div className="use-expansion border-b border-line2 bg-s1" data-testid={`use-expansion-${resource.key}`}>{renderExpansion(resource.key)}</div>}
      </div>
    })}
    {cgroups && <div data-testid="use-group-cgroups">
      <div className="use-row grid-cols-[minmax(96px,130px)_minmax(0,1fr)]" data-expanded={expanded.has("cgroups") || undefined} data-testid="use-row-cgroups" onClick={(event) => { if ((event.target as HTMLElement).closest("button, a, [role=tooltip]") !== null) return; onToggle("cgroups") }}>
        <button aria-expanded={expanded.has("cgroups")} className="use-resource flex cursor-pointer items-center gap-1 self-stretch border-0 bg-transparent px-2 py-[7px] text-left font-sans text-sm font-medium text-fg2 max-[760px]:px-[5px]" data-testid="use-toggle-cgroups" onClick={() => onToggle("cgroups")} type="button">
          {expanded.has("cgroups") ? <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} /> : <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />}
          {t("section.cgroups")}
        </button>
        <span className="use-cell flex min-w-0 items-center px-2 font-sans text-xs text-fg4">{t("use.cgroups_hint")}</span>
      </div>
      {expanded.has("cgroups") && <div className="use-expansion border-b border-line2 bg-s1" data-testid="use-expansion-cgroups">{renderExpansion("cgroups")}</div>}
    </div>}
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

export function lanePointsByLane(lanePoints: readonly LanePoint[]): ReadonlyMap<string, readonly ChartPoint[]> {
  const map = new Map<string, ChartPoint[]>()
  for (const point of lanePoints) {
    const stored = map.get(point.lane)
    const entry = { segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }
    if (stored === undefined) map.set(point.lane, [entry])
    else stored.push(entry)
  }
  return map
}

export function seriesHasReading(points: readonly ChartPoint[]): boolean {
  return points.some((point) => point.value !== null && Number.isFinite(point.value))
}

export function seriesReading(
  points: readonly ChartPoint[],
  cursor: number,
  locale: Locale,
  kind: UseCell["kind"],
  perSecond: string,
): string {
  const stored = readingAt(points, cursor)
  return stored === null ? "—" : reading(stored, locale, kind, perSecond)
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
