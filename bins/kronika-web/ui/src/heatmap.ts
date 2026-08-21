// The heatmap contract from DESIGN.md "Heatmap values", plus the view model
// the renderer consumes. The server derives the ranked top view next to the
// segments; the bundled-fixture build derives the same shape here, in the
// client. A counter cell is the last value in the interval minus the
// identity's latest value at or before the interval's start within the
// requested window, divided by the
// elapsed time between those two observations — one in-interval sample plus a
// preceding baseline is enough. Missing input, no baseline, a non-positive
// observed duration or a negative delta produce null, a zero delta produces
// 0. A gauge cell is the last sample in the interval, or null. Ranking uses
// the whole requested window and does not change with the number of columns.

export const HOUR_MICROS = 3_600_000_000

export interface HeatmapSample {
  readonly entity: string
  readonly timestamp: number
  readonly value: number | null
}

export interface HeatmapInterval {
  readonly start: number
  readonly end: number
}

export interface HeatmapRow {
  readonly entity: string
  // Counter: absolute delta over the whole window. Gauge: maximum sample.
  // Null when the window has no usable ranking value (a reset, one sample).
  readonly total: number | null
  readonly cells: readonly (number | null)[]
}

export interface Heatmap {
  readonly intervals: readonly HeatmapInterval[]
  readonly rows: readonly HeatmapRow[]
  readonly totals: readonly (number | null)[]
  readonly others: readonly (number | null)[]
  readonly totalsTotal: number | null
  readonly othersTotal: number | null
  readonly othersCount: number
  readonly entityCount: number
  // Shared color scale over the ranked rows and the others band. The totals
  // band dwarfs individual entities and gets its own scale in the renderer.
  readonly max: number
}

interface ColumnState {
  count: number
  firstTs: number
  firstValue: number
  lastTs: number
  lastValue: number
}

interface EntityState {
  readonly columns: (ColumnState | undefined)[]
  window: ColumnState | undefined
  gaugeMax: number | undefined
}

export function heatmapIntervals(hour: number, columns: number): readonly HeatmapInterval[] {
  return Array.from({ length: columns }, (_, index) => ({
    start: hour + Math.floor((index * HOUR_MICROS) / columns),
    end: hour + Math.floor(((index + 1) * HOUR_MICROS) / columns) - 1,
  }))
}

export function heatmap(
  samples: readonly HeatmapSample[],
  cumulative: boolean,
  hour: number,
  columns: number,
  top: number,
): Heatmap {
  const intervals = heatmapIntervals(hour, columns)
  const end = hour + HOUR_MICROS
  const entities = new Map<string, EntityState>()
  for (const sample of samples) {
    if (sample.value === null || !Number.isFinite(sample.value) || sample.timestamp < hour || sample.timestamp >= end) continue
    let state = entities.get(sample.entity)
    if (state === undefined) {
      state = { columns: new Array<ColumnState | undefined>(columns), window: undefined, gaugeMax: undefined }
      entities.set(sample.entity, state)
    }
    const column = Math.min(columns - 1, Math.floor(((sample.timestamp - hour) * columns) / HOUR_MICROS))
    state.columns[column] = observe(state.columns[column], sample.timestamp, sample.value)
    state.window = observe(state.window, sample.timestamp, sample.value)
    if (state.gaugeMax === undefined || sample.value > state.gaugeMax) state.gaugeMax = sample.value
  }

  const ranked = [...entities.entries()]
    .map(([entity, state]) => ({
      entity,
      total: cumulative ? counterDelta(state.window) : state.gaugeMax ?? null,
      cells: carriedCells(state.columns, cumulative),
    }))
    .sort((left, right) => compareTotals(left.total, right.total) || (left.entity < right.entity ? -1 : 1))

  const rows = ranked.slice(0, top)
  const rest = ranked.slice(top)
  const totals = sumCells(ranked, columns)
  const others = sumCells(rest, columns)
  let max = 0
  for (const row of [...rows.map((row) => row.cells), others]) {
    for (const cell of row) if (cell !== null && cell > max) max = cell
  }
  return {
    intervals,
    rows,
    totals,
    others,
    totalsTotal: sumTotals(ranked, cumulative),
    othersTotal: sumTotals(rest, cumulative),
    othersCount: rest.length,
    entityCount: ranked.length,
    max,
  }
}

function observe(state: ColumnState | undefined, timestamp: number, value: number): ColumnState {
  if (state === undefined) return { count: 1, firstTs: timestamp, firstValue: value, lastTs: timestamp, lastValue: value }
  state.count += 1
  if (timestamp < state.firstTs) {
    state.firstTs = timestamp
    state.firstValue = value
  }
  if (timestamp >= state.lastTs) {
    state.lastTs = timestamp
    state.lastValue = value
  }
  return state
}

// A counter cell measures from the latest observation at or before the
// interval start, carried across empty and boundary intervals, so a sparse
// cadence still fills every later column.
function carriedCells(columns: readonly (ColumnState | undefined)[], cumulative: boolean): (number | null)[] {
  let carry: { readonly ts: number; readonly value: number } | null = null
  return columns.map((state) => {
    if (state === undefined) return null
    if (!cumulative) return state.lastValue
    const base = carry !== null && carry.ts < state.firstTs
      ? carry
      : { ts: state.firstTs, value: state.firstValue }
    carry = { ts: state.lastTs, value: state.lastValue }
    if (state.lastTs <= base.ts) return null
    const delta = state.lastValue - base.value
    if (delta < 0) return null
    return delta === 0 ? 0 : delta / ((state.lastTs - base.ts) / 1e6)
  })
}

function counterDelta(state: ColumnState | undefined): number | null {
  if (state === undefined || state.count < 2 || state.lastTs <= state.firstTs) return null
  const delta = state.lastValue - state.firstValue
  return delta < 0 ? null : delta
}

function compareTotals(left: number | null, right: number | null): number {
  if (left === null && right === null) return 0
  if (left === null) return 1
  if (right === null) return -1
  return right - left
}

function sumCells(rows: readonly { readonly cells: readonly (number | null)[] }[], columns: number): readonly (number | null)[] {
  const sums = new Array<number | null>(columns).fill(null)
  for (const row of rows) {
    for (const [index, cell] of row.cells.entries()) {
      if (cell === null) continue
      sums[index] = (sums[index] ?? 0) + cell
    }
  }
  return sums
}

function sumTotals(rows: readonly HeatmapRow[], cumulative: boolean): number | null {
  let sum: number | null = null
  for (const row of rows) {
    if (row.total === null) continue
    sum = cumulative ? (sum ?? 0) + row.total : Math.max(sum ?? 0, row.total)
  }
  return sum
}

export const HEATMAP_STEPS = 6

// Square-root stepping keeps the long tail of a skewed hour visible: a cell at
// a quarter of the maximum still lands three steps up, not one. Zero is step
// 0, a real value distinct from null, which draws nothing at all.
export function heatmapIntensity(value: number, max: number): number {
  if (value <= 0 || max <= 0) return 0
  return Math.max(1, Math.min(HEATMAP_STEPS, Math.ceil(Math.sqrt(value / max) * HEATMAP_STEPS)))
}

export function heatmapEntityKey(values: readonly (string | null)[]): string {
  return JSON.stringify(values)
}

// The server's /api/heatmap response and the bundled-fixture derivation share
// this shape; the renderer never knows which produced it.
export interface HeatmapBand {
  readonly total: number | null
  readonly cells: readonly (number | null)[]
}

export interface HeatmapViewRow {
  readonly typeId: string
  readonly identity: readonly (string | null)[]
  readonly labels: readonly (string | null)[]
  // Distinct identities aggregated into the row when the request grouped.
  readonly members: number | null
  readonly total: number | null
  readonly cells: readonly (number | null)[]
}

export interface HeatmapView {
  readonly cumulative: boolean
  readonly intervals: readonly HeatmapInterval[]
  readonly rows: readonly HeatmapViewRow[]
  readonly totals: HeatmapBand
  readonly others: HeatmapBand
  readonly othersCount: number
  readonly entityCount: number
}

// The block shows fewer rows than the server ranked; the rows beyond the fold
// join the others band with plain null-aware arithmetic.
export function collapseHeatmapView(view: HeatmapView, top: number): HeatmapView {
  if (view.rows.length <= top) return view
  const kept = view.rows.slice(0, top)
  const folded = view.rows.slice(top)
  const cells = view.others.cells.map((cell, index) => {
    const contributions = [cell, ...folded.map((row) => row.cells[index] ?? null)].filter((stored): stored is number => stored !== null)
    return contributions.length === 0 ? null : contributions.reduce((sum, stored) => sum + stored, 0)
  })
  const totals = [view.others.total, ...folded.map((row) => row.total)].filter((stored): stored is number => stored !== null)
  const total = totals.length === 0
    ? null
    : view.cumulative ? totals.reduce((sum, stored) => sum + stored, 0) : Math.max(...totals)
  return {
    ...view,
    rows: kept,
    others: { cells, total },
    othersCount: view.othersCount + folded.length,
  }
}

export function heatmapViewMax(view: HeatmapView): number {
  let max = 0
  for (const cells of [...view.rows.map((row) => row.cells), view.others.cells]) {
    for (const cell of cells) if (cell !== null && cell > max) max = cell
  }
  return max
}
