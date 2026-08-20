import { useMemo } from "react"

import { sparkCursorX, sparkGeometry, SPARK_HEIGHT, SPARK_WIDTH } from "./spark"
import type { ChartPoint } from "./series-chart"

// The USE ledger cell: the hour's shape at sparkline size, with the shared
// cursor as a dashed tick.
export function SparkCell({ cursor, end, hour, max, points, second }: {
  readonly cursor: number
  readonly end: number
  readonly hour: number
  readonly max: number
  readonly points: readonly ChartPoint[]
  readonly second?: readonly ChartPoint[] | undefined
}) {
  const primary = useMemo(() => sparkGeometry(points, hour, end, max), [end, hour, max, points])
  const secondary = useMemo(() => second === undefined ? null : sparkGeometry(second, hour, end, max), [end, hour, max, second])
  const cursorX = sparkCursorX(cursor, hour, end)
  return <svg aria-hidden="true" className="block h-[22px] w-full min-w-0" preserveAspectRatio="none" viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}>
    {secondary !== null && <path d={secondary.path} fill="none" stroke="var(--color-series-2)" strokeWidth={1.4} vectorEffect="non-scaling-stroke" />}
    <path d={primary.path} fill="none" stroke="var(--color-series-1)" strokeWidth={1.4} vectorEffect="non-scaling-stroke" />
    {/* Zero-length strokes with round caps stay round under the stretched
        viewBox, where a circle would be squashed. */}
    {[...primary.dots, ...(secondary?.dots ?? [])].map((dot, index) => <path d={`M${dot} l0.01 0`} key={index} stroke={index < primary.dots.length ? "var(--color-series-1)" : "var(--color-series-2)"} strokeLinecap="round" strokeWidth={3} vectorEffect="non-scaling-stroke" />)}
    {cursorX !== null && <path d={`M${cursorX.toFixed(2)} 0 L${cursorX.toFixed(2)} ${SPARK_HEIGHT}`} stroke="var(--color-cursor)" strokeDasharray="3 3" strokeWidth={1} vectorEffect="non-scaling-stroke" />}
  </svg>
}
