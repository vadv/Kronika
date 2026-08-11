import { useId } from "react"

import type { Locale } from "./model"
import { TimeTicks } from "./time-ticks"

export interface ChartPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

type NumericChartPoint = ChartPoint & { readonly value: number }

export function SeriesChart({
  hour,
  label,
  locale,
  format,
  points,
}: {
  readonly hour: number
  readonly label: string
  readonly locale: Locale
  /** How the extremes read: a chart of bytes says bytes, not a bare number. */
  readonly format?: ((value: number, locale: Locale) => string) | undefined
  readonly points: readonly ChartPoint[]
}) {
  const title = useId()
  const end = hour + 3_600_000_000
  const numeric = points.filter((point): point is NumericChartPoint => point.value !== null && Number.isFinite(point.value))
  const values = numeric.map((point) => point.value)
  const low = values.length === 0 ? 0 : Math.min(...values)
  const high = values.length === 0 ? 0 : Math.max(...values)
  const span = high - low || 1
  const paths = chartRuns(points)
  return <figure className="series-chart">
    <figcaption id={title}>
      <span>{label}</span>
      <span>{values.length === 0 ? "—" : `${(format ?? number)(low, locale)}–${(format ?? number)(high, locale)}`}</span>
    </figcaption>
    <svg aria-labelledby={title} preserveAspectRatio="none" role="img" viewBox="0 0 920 126">
      {[0, 1, 2, 3, 4, 5, 6].map((tick) => <line className="mini-grid" key={tick} x1={tick / 6 * 920} x2={tick / 6 * 920} y1="5" y2="105" />)}
      <line className="mini-zero" x1="0" x2="920" y1="105" y2="105" />
      {[...paths.entries()].map(([segmentId, stored]) => {
        const path = stored.slice().sort((left, right) => left.timestamp - right.timestamp).map((point, index) => {
          const x = Math.max(0, Math.min(920, (point.timestamp - hour) / (end - hour) * 920))
          const y = 101 - (point.value - low) / span * 92
          return `${index === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`
        }).join(" ")
        return <path className="mini-series" d={path} key={segmentId} />
      })}
    </svg>
    <TimeTicks className="mini-time-ticks" hour={hour} />
  </figure>
}

export function chartRuns(points: readonly ChartPoint[]): ReadonlyMap<string, readonly NumericChartPoint[]> {
  const bySegment = new Map<string, ChartPoint[]>()
  for (const point of points) {
    const stored = bySegment.get(point.segmentId) ?? []
    stored.push(point)
    bySegment.set(point.segmentId, stored)
  }
  const runs = new Map<string, readonly NumericChartPoint[]>()
  for (const [segmentId, stored] of bySegment) {
    let run: NumericChartPoint[] = []
    let index = 0
    const flush = () => {
      if (run.length !== 0) runs.set(`${segmentId}:${index}`, run)
      run = []
      index += 1
    }
    for (const point of stored.slice().sort((left, right) => left.timestamp - right.timestamp)) {
      if (point.value === null || !Number.isFinite(point.value)) {
        flush()
      } else {
        run.push(point as NumericChartPoint)
      }
    }
    flush()
  }
  return runs
}

function number(value: number, locale: Locale): string {
  if (Math.abs(value) >= 1_000_000) return value.toExponential(2)
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 2, useGrouping: false }).format(value)
}
