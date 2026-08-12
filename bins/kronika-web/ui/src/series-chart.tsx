import { useId } from "react"

import { niceCeiling, numericRuns, svgPath, type NumericPoint } from "./chart"
import { compact, type Locale } from "./model"
import { TimeTicks } from "./time-ticks"

export interface ChartPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

type NumericChartPoint = NumericPoint<ChartPoint>
export type ChartScale = "auto" | "percent" | "count" | "duration"

export function SeriesChart({
  cursor,
  empty = "—",
  hour,
  label,
  locale,
  format,
  points,
  scale = "auto",
  second,
}: {
  readonly cursor?: number | undefined
  readonly empty?: string | undefined
  readonly hour: number
  readonly label: string
  readonly locale: Locale
  readonly format?: ((value: number, locale: Locale) => string) | undefined
  readonly points: readonly ChartPoint[]
  readonly scale?: ChartScale | undefined
  readonly second?: readonly ChartPoint[] | undefined
}) {
  const title = useId()
  const end = hour + 3_600_000_000
  const numeric = numericChartPoints(points, second)
  const values = numeric.map((point) => point.value)
  const { low, high } = chartDomain(values, scale)
  const span = high - low || 1
  const paths = chartRuns(points)
  const companion: ReadonlyMap<string, readonly NumericChartPoint[]> = second === undefined ? new Map() : chartRuns(second)
  const reading = readingAt(points, cursor)
  const exact = cursor === undefined ? undefined : numeric.find((point) => point.timestamp === cursor)
  const hasData = numeric.length !== 0
  return <figure className="series-chart">
    <figcaption id={title}>
      <span>{label}</span>
      <span>{reading === null ? "—" : (format ?? compact)(reading, locale)}</span>
    </figcaption>
    {!hasData
      ? <p className="series-empty">{empty}</p>
      : <><svg aria-labelledby={title} preserveAspectRatio="none" role="img" viewBox="0 0 920 126">
      {[0, 1, 2, 3, 4, 5, 6].map((tick) => <line className="mini-grid" key={tick} x1={tick / 6 * 920} x2={tick / 6 * 920} y1="5" y2="105" />)}
      <line className="mini-zero" x1="0" x2="920" y1="105" y2="105" />
      {cursor !== undefined && cursor >= hour && cursor < end
        && <line className="cursor-line" x1={(cursor - hour) / (end - hour) * 920} x2={(cursor - hour) / (end - hour) * 920} y1="5" y2="105" />}
      {[...companion.entries()].map(([segmentId, stored]) => {
        const path = svgPath(stored.slice().sort((left, right) => left.timestamp - right.timestamp), (point) => [
          Math.max(0, Math.min(920, (point.timestamp - hour) / (end - hour) * 920)),
          101 - (point.value - low) / span * 92,
        ])
        return <path className="mini-series mini-second" d={path} key={`second:${segmentId}`} />
      })}
      {[...paths.entries()].map(([segmentId, stored]) => {
        const path = svgPath(stored.slice().sort((left, right) => left.timestamp - right.timestamp), (point) => [
          Math.max(0, Math.min(920, (point.timestamp - hour) / (end - hour) * 920)),
          101 - (point.value - low) / span * 92,
        ])
        return <path className="mini-series" d={path} key={segmentId} />
      })}
      {exact !== undefined && <circle className="mini-selected-point" cx={Math.max(0, Math.min(920, (exact.timestamp - hour) / (end - hour) * 920))} cy={101 - (exact.value - low) / span * 92} r="3.5" />}
      </svg>
      <span aria-hidden="true" className="series-ceiling">{(format ?? compact)(high, locale)}</span>
      {low !== 0 && <span aria-hidden="true" className="series-floor">{(format ?? compact)(low, locale)}</span>}
      <TimeTicks className="mini-time-ticks" hour={hour} /></>}
  </figure>
}

export function numericChartPoints(
  points: readonly ChartPoint[],
  second?: readonly ChartPoint[] | undefined,
): readonly NumericChartPoint[] {
  const both = second === undefined ? points : [...points, ...second]
  return both.filter((point): point is NumericChartPoint => point.value !== null && Number.isFinite(point.value))
}

export function readingAt(points: readonly ChartPoint[], cursor: number | undefined): number | null {
  let chosen: ChartPoint | null = null
  for (const point of points) {
    if (cursor !== undefined && point.timestamp > cursor) continue
    if (chosen === null || point.timestamp > chosen.timestamp) chosen = point
  }
  return chosen?.value ?? null
}

export function chartDomain(values: readonly number[], scale: ChartScale): { readonly low: number; readonly high: number } {
  if (scale === "percent") return { low: 0, high: 100 }
  if (scale === "count" || scale === "duration") {
    return { low: 0, high: niceCeiling(Math.max(0, ...values)) }
  }
  const low = Math.min(0, ...values)
  const high = values.length === 0 ? 1 : Math.max(...values)
  return { low, high: high === low ? low + 1 : high }
}

export function chartRuns(points: readonly ChartPoint[]): ReadonlyMap<string, readonly NumericChartPoint[]> {
  return numericRuns(points, (left, right) => left.localeCompare(right))
}
