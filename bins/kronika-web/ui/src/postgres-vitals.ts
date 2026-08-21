import type { DataRow } from "./api"
import { asNumber, rawText, value } from "./model"
import { intervalMetric } from "./postgres-metrics"
import type { ChartPoint } from "./series-chart"

// Instance vitals for the Overview: recorded per-database counters summed
// into one instance series, gauges reduced per snapshot moment, and plain
// row counts. Pairing follows the shipped convention: adjacent rows of one
// (typeId, identity) in time order, deltas through intervalMetric.

export interface VitalSeries {
  // Rate per second at each snapshot moment that had a previous reading.
  readonly points: readonly ChartPoint[]
  // The hour's total: the sum of every recorded delta, null when none.
  readonly total: number | null
}

const EMPTY_VITAL: VitalSeries = { points: [], total: null }

// Sum the per-second rates of `fields` over every identity in the section.
export function sumCounterVital(rows: readonly DataRow[], fields: readonly string[], identity = "datid"): VitalSeries {
  const ordered = byIdentity(rows, identity)
  const rates = new Map<number, { segmentId: string; value: number }>()
  let total: number | null = null
  for (const series of ordered.values()) {
    for (let index = 1; index < series.length; index += 1) {
      const earlier = series[index - 1]
      const later = series[index]
      if (earlier === undefined || later === undefined || earlier.typeId !== later.typeId) continue
      for (const field of fields) {
        const rate = intervalMetric(earlier, later, field)
        if (rate === null) continue
        const slot = rates.get(later.timestamp)
        if (slot === undefined) rates.set(later.timestamp, { segmentId: later.segmentId, value: rate })
        else slot.value += rate
        total = (total ?? 0) + rate * ((later.timestamp - earlier.timestamp) / 1_000_000)
      }
    }
  }
  if (rates.size === 0) return EMPTY_VITAL
  return {
    points: [...rates.entries()]
      .sort(([left], [right]) => left - right)
      .map(([timestamp, slot]) => ({ segmentId: slot.segmentId, timestamp, value: slot.value })),
    total,
  }
}

// One point per snapshot moment: the sum (or the maximum) of a recorded gauge.
export function gaugeSeries(rows: readonly DataRow[], field: string, reduce: "sum" | "max"): readonly ChartPoint[] {
  const moments = new Map<number, { segmentId: string; value: number }>()
  for (const row of rows) {
    const stored = asNumber(value(row, field))
    if (stored === null) continue
    const slot = moments.get(row.timestamp)
    if (slot === undefined) moments.set(row.timestamp, { segmentId: row.segmentId, value: stored })
    else slot.value = reduce === "sum" ? slot.value + stored : Math.max(slot.value, stored)
  }
  return [...moments.entries()]
    .sort(([left], [right]) => left - right)
    .map(([timestamp, slot]) => ({ segmentId: slot.segmentId, timestamp, value: slot.value }))
}

// One point per snapshot moment: how many rows match.
export function countSeries(rows: readonly DataRow[], matches: (row: DataRow) => boolean): readonly ChartPoint[] {
  const moments = new Map<number, { segmentId: string; value: number }>()
  for (const row of rows) {
    const slot = moments.get(row.timestamp)
    if (slot === undefined) moments.set(row.timestamp, { segmentId: row.segmentId, value: matches(row) ? 1 : 0 })
    else if (matches(row)) slot.value += 1
  }
  return [...moments.entries()]
    .sort(([left], [right]) => left - right)
    .map(([timestamp, slot]) => ({ segmentId: slot.segmentId, timestamp, value: slot.value }))
}

export function peakPoint(points: readonly ChartPoint[]): ChartPoint | null {
  let peak: ChartPoint | null = null
  for (const point of points) {
    if (point.value === null) continue
    if (peak === null || peak.value === null || point.value > peak.value) peak = point
  }
  return peak
}

export function lastValue(points: readonly ChartPoint[]): number | null {
  for (let index = points.length - 1; index >= 0; index -= 1) {
    const stored = points[index]
    if (stored !== undefined && stored.value !== null) return stored.value
  }
  return null
}

// The hour share of `part` in `whole`, from the totals of two counter vitals.
export function shareOfTotals(part: VitalSeries, whole: VitalSeries): number | null {
  if (part.total === null || whole.total === null || whole.total <= 0) return null
  return part.total / whole.total
}

export interface SettingChange {
  readonly timestamp: number
  readonly name: string
  readonly from: string | null
  readonly to: string | null
}

// pg_settings records rows on change; every row after a name's first recorded
// moment in the hour is a change of that setting.
export function settingChanges(rows: readonly DataRow[]): readonly SettingChange[] {
  const byName = new Map<string, DataRow[]>()
  for (const row of [...rows].sort((left, right) => left.timestamp - right.timestamp)) {
    const name = rawText(value(row, "name"))
    if (name === null) continue
    const series = byName.get(name)
    if (series === undefined) byName.set(name, [row])
    else series.push(row)
  }
  const changes: SettingChange[] = []
  for (const [name, series] of byName) {
    for (let index = 1; index < series.length; index += 1) {
      const earlier = series[index - 1]
      const later = series[index]
      if (earlier === undefined || later === undefined) continue
      const from = rawText(value(earlier, "setting"))
      const to = rawText(value(later, "setting"))
      if (from === to) continue
      changes.push({ timestamp: later.timestamp, name, from, to })
    }
  }
  return changes.sort((left, right) => left.timestamp - right.timestamp || left.name.localeCompare(right.name))
}

// The value of one setting at the cursor: the last recorded row at or before
// it. The section records on change, so when the hour's first row for a name
// comes later than the cursor, no change happened in between and that first
// row already carries the cursor's value.
export function settingAt(rows: readonly DataRow[], name: string, cursor: number): string | null {
  let before: DataRow | null = null
  let after: DataRow | null = null
  for (const row of rows) {
    if (rawText(value(row, "name")) !== name) continue
    if (row.timestamp <= cursor) {
      if (before === null || row.timestamp > before.timestamp) before = row
    } else if (after === null || row.timestamp < after.timestamp) {
      after = row
    }
  }
  const found = before ?? after
  return found === null ? null : rawText(value(found, "setting"))
}

function byIdentity(rows: readonly DataRow[], identity: string): ReadonlyMap<string, readonly DataRow[]> {
  const ordered = new Map<string, DataRow[]>()
  for (const row of [...rows].sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))) {
    const key = `${row.typeId}\u{1f}${rawText(value(row, identity)) ?? ""}`
    const series = ordered.get(key)
    if (series === undefined) ordered.set(key, [row])
    else series.push(row)
  }
  return ordered
}
