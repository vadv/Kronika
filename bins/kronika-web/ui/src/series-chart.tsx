import { useMemo, useRef } from "react"

import { compact, floorHour, humanPercent, type Locale } from "./model"
import { UPlotChart, type ChartScale as SemanticScale, type RecordedSeries } from "./uplot-chart"

export interface ChartPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export interface NumericChartPoint extends ChartPoint {
  readonly value: number
}

export type ChartScale = "auto" | "percent" | "count" | "duration" | "nonnegative" | "signed"

export function SeriesChart({
  cursor,
  empty = "—",
  hour,
  label,
  locale,
  format,
  points,
  scale = "nonnegative",
  second,
  secondLabel,
  unit = "",
  onCursor,
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
  readonly secondLabel?: string | undefined
  readonly unit?: string | undefined
  readonly onCursor?: ((timestamp: number) => void) | undefined
}) {
  const numeric = numericChartPoints(points, second)
  const reading = readingAt(points, cursor)
  const hasData = numeric.length !== 0
  const formatValue = format ?? (scale === "percent" ? humanPercent : compact)
  const formatter = useRef(formatValue)
  formatter.current = formatValue
  const stableFormat = useMemo(() => (number: number, place: Locale) => formatter.current(number, place), [])
  const semantic: SemanticScale = scale === "percent" ? "percent" : scale === "auto" || scale === "signed" ? "signed" : "nonnegative"
  const series = useMemo<readonly RecordedSeries[]>(() => [
    { color: "cyan", id: "primary", label, points, scale: semantic, tick: stableFormat, unit, value: stableFormat },
    ...(second === undefined ? [] : [{ color: "amber" as const, id: "secondary", label: secondLabel ?? `${label} 2`, points: second, scale: semantic, tick: stableFormat, unit, value: stableFormat }]),
  ], [label, points, second, secondLabel, semantic, stableFormat, unit])
  return <div className="series-chart">
    {!hasData
      ? <><div className="series-reading"><span>{label}</span><span>—</span></div><p className="series-empty">{empty}</p></>
      : <UPlotChart cursor={cursor} hour={hour} locale={locale} onCursor={onCursor} reading={reading === null ? "—" : formatValue(reading, locale)} series={series} />}
  </div>
}

export function numericChartPoints(
  points: readonly ChartPoint[],
  second?: readonly ChartPoint[] | undefined,
): readonly NumericChartPoint[] {
  const both = second === undefined ? points : [...points, ...second]
  return both.filter((point): point is NumericChartPoint => point.value !== null && Number.isFinite(point.value))
}

export function readingAt(points: readonly ChartPoint[], cursor: number | undefined): number | null {
  return sampleAtOrBefore(points, cursor)?.value ?? null
}

export function sampleAtOrBefore(points: readonly ChartPoint[], cursor: number | undefined): ChartPoint | null {
  let chosen: ChartPoint | null = null
  for (const point of points) {
    if (cursor !== undefined && point.timestamp > cursor) continue
    if (chosen === null || point.timestamp >= chosen.timestamp) chosen = point
  }
  return chosen
}

export function uncollectedStart(points: readonly ChartPoint[], hour: number, now = Date.now() * 1_000): number | null {
  if (floorHour(now) !== hour) return null
  const end = hour + 3_600_000_000
  let latest = hour
  for (const point of points) {
    if (point.timestamp >= hour && point.timestamp < end) latest = Math.max(latest, point.timestamp)
  }
  return latest
}
