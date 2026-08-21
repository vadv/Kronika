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
  // Whole-window counter delta or gauge maximum; null without a usable total.
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
  return {
    intervals,
    rows,
    totals,
    others,
    totalsTotal: sumTotals(ranked, cumulative),
    othersTotal: sumTotals(rest, cumulative),
    othersCount: rest.length,
    entityCount: ranked.length,
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

// Carry the latest earlier sample across empty counter intervals.
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

// Square-root steps preserve contrast in skewed rows. Zero is distinct from null.
export function heatmapIntensity(value: number, max: number): number {
  if (value <= 0 || max <= 0) return 0
  return Math.max(1, Math.min(HEATMAP_STEPS, Math.ceil(Math.sqrt(value / max) * HEATMAP_STEPS)))
}

export function heatmapEntityKey(values: readonly (string | null)[]): string {
  return JSON.stringify(values)
}

export interface HeatmapBand {
  readonly total: number | null
  readonly cells: readonly (number | null)[]
}

export interface HeatmapViewRow {
  readonly typeId: string
  readonly identity: readonly (string | null)[]
  readonly labels: readonly (string | null)[]
  // Distinct identities represented by a grouped row; otherwise null.
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

// Fold rows hidden by the compact view into the Others band.
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
