export type NumericPoint<Point extends { readonly value: number | null }> = Point & { readonly value: number }

export function numericRuns<Point extends { readonly segmentId: string; readonly timestamp: number; readonly value: number | null }>(
  points: readonly Point[],
  compareSegments: (left: string, right: string) => number,
): ReadonlyMap<string, readonly NumericPoint<Point>[]> {
  const runs = new Map<string, readonly NumericPoint<Point>[]>()
  let run: NumericPoint<Point>[] = []
  let index = 0
  const flush = () => {
    if (run.length !== 0) runs.set(String(index), run)
    run = []
    index += 1
  }
  for (const point of points.slice().sort((left, right) => left.timestamp - right.timestamp || compareSegments(left.segmentId, right.segmentId))) {
    if (point.value === null || !Number.isFinite(point.value)) flush()
    else run.push(point as NumericPoint<Point>)
  }
  flush()
  return runs
}

export function niceCeiling(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  return (normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10) * magnitude
}

export function svgPath<Point>(points: readonly Point[], coordinates: (point: Point) => readonly [number, number]): string {
  return points.map((point, index) => {
    const [x, y] = coordinates(point)
    return `${index === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`
  }).join(" ")
}
