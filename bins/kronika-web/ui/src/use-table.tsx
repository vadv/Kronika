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
  readonly metric: string
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
    utilisation: { lane: "cpu_busy", kind: "share", metric: "cpu_busy" },
    saturation: { lane: "cpu_stall", kind: "share", metric: "cpu_stall" },
    errors: null,
  },
  {
    key: "memory",
    utilisation: { lane: "memory", kind: "share", metric: "memory" },
    saturation: { lane: "mem_swap", kind: "rate", metric: "mem_swap" },
    errors: { lane: "mem_oom", kind: "rate", metric: "oom_kill" },
  },
  {
    key: "disk",
    utilisation: { lane: "disk_busy", kind: "share", metric: "disk_busy" },
    saturation: { lane: "disk_queue", kind: "count", metric: "disk_queue" },
    errors: null,
  },
  {
    key: "network",
    utilisation: { lane: "net_rx", kind: "bytes", metric: "network_rx", second: "net_tx" },
    saturation: { lane: "net_drop", kind: "rate", metric: "net_drop" },
    errors: { lane: "net_errors", kind: "rate", metric: "network_errors" },
  },
]

export const USE_COLUMNS = ["utilisation", "saturation", "errors"] as const

// The ledger IS the Host page: a row is one resource, its cells carry the
// hour's shape with the reading at the cursor, and expanding a row discloses
// the group's chart, metric chips and entity tables in place. Expansion is
// disclosure, not navigation — several rows open side by side.
export function UseTable({
  afterCgroups,
  cgroups,
  cgroupsFirst = false,
  containerScopes = false,
  cursor,
  expanded,
  hour,
  lanePoints,
  locale,
  metric,
  onCellSelect,
  onToggle,
  renderExpansion,
  visibleResources,
  withContent,
  t,
}: {
  readonly afterCgroups?: ReactNode | undefined
  readonly cgroups: boolean
  readonly cgroupsFirst?: boolean | undefined
  readonly containerScopes?: boolean | undefined
  readonly cursor: number
  readonly expanded: ReadonlySet<LedgerKey>
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly metric: string | null
  readonly onCellSelect: (key: UseResourceKey, metric: string) => void
  readonly onToggle: (key: LedgerKey) => void
  readonly renderExpansion: (key: LedgerKey) => ReactNode
  readonly visibleResources?: ReadonlySet<UseResourceKey> | undefined
  readonly withContent: ReadonlySet<UseResourceKey>
  readonly t: Translate
}) {
  // A row exists when the hour carries anything for its resource: lane
  // readings, group metrics or entity tables. Cells without a lane stay "—".
  const byLane = useMemo(() => lanePointsByLane(lanePoints), [lanePoints])
  const shown = useMemo(() => USE_RESOURCES.filter((resource) =>
    (visibleResources === undefined || visibleResources.has(resource.key))
    && (withContent.has(resource.key) || USE_COLUMNS.some((column) => {
      const cell = resource[column]
      return cell !== null && seriesHasReading(byLane.get(cell.lane) ?? [])
    }))), [byLane, visibleResources, withContent])
  const end = hour + 3_600_000_000
  if (shown.length === 0 && !cgroups) return null
  const cgroupRow = cgroups && <div data-testid="use-group-cgroups">
    <div className="use-row grid-cols-[minmax(96px,130px)_minmax(0,1fr)]" data-expanded={expanded.has("cgroups") || undefined} data-testid="use-row-cgroups">
      <button aria-expanded={expanded.has("cgroups")} className="use-resource flex cursor-pointer items-center gap-1 self-stretch border-0 bg-transparent px-2 py-[7px] text-left font-sans text-sm font-medium text-fg2 hover:bg-s3 coarse:min-h-11 max-[760px]:px-[5px]" data-testid="use-toggle-cgroups" onClick={() => onToggle("cgroups")} type="button">
        {expanded.has("cgroups") ? <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} /> : <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />}
        {t("section.cgroups")}
      </button>
      <span className="use-cell flex min-w-0 items-center px-2 font-sans text-sm text-fg4">{t("use.cgroups_hint")}</span>
    </div>
    {expanded.has("cgroups") && <div className="use-expansion border-b border-line2 bg-s1" data-testid="use-expansion-cgroups">{renderExpansion("cgroups")}</div>}
  </div>
  const resourceRow = (resource: UseResource) => {
    const open = expanded.has(resource.key)
    return <div data-testid={`use-group-${resource.key}`} key={resource.key}>
      <div className="use-row" data-expanded={open || undefined} data-testid={`use-row-${resource.key}`}>
        <button aria-expanded={open} className="use-resource flex cursor-pointer items-center gap-1 self-stretch border-0 bg-transparent px-2 py-[7px] text-left font-sans text-sm font-medium text-fg2 hover:bg-s3 coarse:min-h-11 max-[760px]:px-[5px]" data-testid={`use-toggle-${resource.key}`} onClick={() => onToggle(resource.key)} type="button">
          {open ? <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} /> : <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />}
          {t(containerScopes && resource.key === "network" ? "use.resource.namespace_network" : `use.resource.${resource.key}`)}
        </button>
        {USE_COLUMNS.map((column) => {
          const cell = resource[column]
          const points = cell === null ? [] : byLane.get(cell.lane) ?? []
          if (cell === null || !seriesHasReading(points)) {
            return <span className="use-cell relative flex min-w-0 items-center justify-center text-sm text-fg4" data-testid={`use-empty-${resource.key}-${column}`} key={column} title={t("use.not_measured")}>—</span>
          }
          const second = cell.second === undefined ? undefined : byLane.get(cell.second) ?? []
          const primary = seriesReading(points, cursor, locale, cell.kind, t("unit.per_second"))
          const secondary = second === undefined ? null : seriesReading(second, cursor, locale, cell.kind, t("unit.per_second"))
          const resourceLabel = t(containerScopes && resource.key === "network" ? "use.resource.namespace_network" : `use.resource.${resource.key}`)
          const laneLabels = [t(`use.lane.${cell.lane}`), ...(cell.second === undefined ? [] : [t(`use.lane.${cell.second}`)])]
          const readings = [primary, ...(secondary === null ? [] : [secondary])]
          const accessibleName = `${resourceLabel} · ${t(`use.${column}`)} · ${laneLabels.map((label, index) => `${label}: ${readings[index]}`).join(" · ")}`
          return <span className="use-cell relative min-w-0 px-2 py-1.5 coarse:min-h-11 max-[760px]:px-[5px]" key={column}>
            <button
              aria-label={accessibleName}
              aria-pressed={metric === cell.metric}
              className="use-cell-action absolute inset-0 z-0 cursor-pointer border-0 bg-transparent"
              data-testid={`use-cell-${resource.key}-${column}`}
              onClick={(event) => { event.stopPropagation(); onCellSelect(resource.key, cell.metric) }}
              title={accessibleName}
              type="button"
            />
            <span className="use-lane pointer-events-none relative z-[1] flex items-baseline justify-start gap-[7px] max-[760px]:flex-wrap max-[760px]:gap-x-[5px] max-[760px]:gap-y-0.5 max-[760px]:[&>span]:flex-none [&>span]:min-w-0 [&>span]:font-sans [&>span]:text-sm [&>span]:text-fg3 [&_strong]:ml-auto [&_strong]:flex-none [&_strong]:whitespace-nowrap [&_strong]:font-mono [&_strong]:text-sm [&_strong]:font-normal [&_strong]:tabular-nums [&_strong]:text-fg2 max-[760px]:[&_strong]:ml-0 max-[760px]:[&_strong]:w-full max-[760px]:[&_strong]:whitespace-normal max-[760px]:[&_strong]:leading-tight">
              <span>{laneLabels.join(" · ")}</span>
              <strong>{readings.join(" · ")}</strong>
            </span>
            <span className="pointer-events-none relative z-[1]"><SparkCell cursor={cursor} end={end} hour={hour} max={sparkScaleMax(cell.kind, [points, ...(second === undefined ? [] : [second])])} points={points} second={second} /></span>
          </span>
        })}
      </div>
      {open && <div className="use-expansion border-b border-line2 bg-s1" data-testid={`use-expansion-${resource.key}`}>{renderExpansion(resource.key)}</div>}
    </div>
  }
  const scope = (key: "container" | "namespace" | "host") => <h2 className="use-scope" data-testid={`use-scope-${key}`}>{t(`use.scope.${key}`)}</h2>
  const network = shown.filter(({ key }) => key === "network")
  const host = shown.filter(({ key }) => key !== "network")
  return <section aria-label={t("use.title")} className="use-table" data-testid="use-table">
    <header className="grid grid-cols-[minmax(96px,130px)_repeat(3,minmax(0,1fr))] border-b border-line2 text-sm font-medium text-fg3 [&>span]:px-2 [&>span]:py-[5px] max-[760px]:grid-cols-[80px_repeat(3,minmax(0,1fr))] max-[760px]:[&>span]:px-[5px]">
      <span>{t("use.resource")}</span>
      {USE_COLUMNS.map((column) => <span className="use-header-cell flex items-center" key={column}>
        <LabelHelp helpKey={`use.${column}.help`} labelKey={`use.${column}`} t={t} />
      </span>)}
    </header>
    {containerScopes ? <>
      {cgroups && <>{scope("container")}{cgroupRow}{afterCgroups !== undefined && <div className="border-b border-line2 bg-s1 [&_.activity-block]:rounded-none [&_.activity-block]:border-x-0 [&_.activity-block]:border-b-0 [&_.activity-block]:first:border-t-0" data-testid="use-cgroup-activity">{afterCgroups}</div>}</>}
      {network.length > 0 && <>{scope("namespace")}{network.map(resourceRow)}</>}
      {host.length > 0 && <>{scope("host")}{host.map(resourceRow)}</>}
    </> : <>
      {cgroupsFirst && cgroupRow}
      {shown.map(resourceRow)}
      {!cgroupsFirst && cgroupRow}
    </>}
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

export function reading(value: number, locale: Locale, kind: UseCell["kind"], perSecond: string): string {
  if (kind === "share") return humanPercent(value, locale)
  if (kind === "bytes") return humanBytes(value, locale, perSecond)
  if (kind === "count") return measure(value, locale)
  return measure(value, locale, perSecond)
}
