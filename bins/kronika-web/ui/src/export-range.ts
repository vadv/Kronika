import type { SegmentBound } from "./api"

const MICROS = 1_000_000
const HOUR_SECONDS = 3_600

// An export range is two inclusive whole Unix seconds, the same contract the
// server accepts. Everything here is arithmetic on those seconds so the strip
// never has to parse what it shows.
export interface ExportRange {
  readonly from: number
  readonly to: number
}

export type ExportPreset = "hour" | "around5" | "around15" | "around30"

export const EXPORT_PRESETS: readonly ExportPreset[] = ["hour", "around5", "around15", "around30"]

const AROUND_MINUTES: Readonly<Record<Exclude<ExportPreset, "hour">, number>> = { around5: 5, around15: 15, around30: 30 }

export function hourRange(hourMicros: number): ExportRange {
  const from = Math.floor(hourMicros / MICROS)
  return { from, to: from + HOUR_SECONDS - 1 }
}

// A window around the cursor: N minutes before it and N minutes after it,
// whole seconds, exactly 2N minutes long.
export function aroundCursor(cursorMicros: number, minutes: number): ExportRange {
  const centre = Math.floor(cursorMicros / MICROS)
  return { from: centre - minutes * 60, to: centre + minutes * 60 - 1 }
}

export function presetRange(preset: ExportPreset, hourMicros: number, cursorMicros: number): ExportRange {
  return preset === "hour" ? hourRange(hourMicros) : aroundCursor(cursorMicros, AROUND_MINUTES[preset])
}

export function activePreset(range: ExportRange, hourMicros: number, cursorMicros: number): ExportPreset | null {
  return EXPORT_PRESETS.find((preset) => sameRange(presetRange(preset, hourMicros, cursorMicros), range)) ?? null
}

export function shiftRange(range: ExportRange, seconds: number): ExportRange {
  return { from: range.from + seconds, to: range.to + seconds }
}

export function sameRange(left: ExportRange, right: ExportRange): boolean {
  return left.from === right.from && left.to === right.to
}

export function rangeSeconds(range: ExportRange): number {
  return range.to - range.from + 1
}

export function validRange(range: ExportRange): boolean {
  return Number.isSafeInteger(range.from) && Number.isSafeInteger(range.to) && range.from > 0 && range.from <= range.to
}

// The server names the file from both inclusive UTC seconds; naming it here
// lets the strip show the name before anything is requested.
export function exportFilename(range: ExportRange): string {
  return `kronika-${utcStamp(range.from)}-${utcStamp(range.to)}-utc.html`
}

function utcStamp(second: number): string {
  const date = new Date(second * 1_000)
  const two = (value: number) => String(value).padStart(2, "0")
  return `${date.getUTCFullYear()}-${two(date.getUTCMonth() + 1)}-${two(date.getUTCDate())}-${two(date.getUTCHours())}${two(date.getUTCMinutes())}${two(date.getUTCSeconds())}`
}

export interface RangeCoverage {
  // Recorded seconds inside the range, clamped to it; null when nothing was recorded there.
  readonly recorded: ExportRange | null
  // Gaps longer than the tolerance between recorded segments inside the range, in seconds.
  readonly gaps: readonly ExportRange[]
}

// What the export will actually contain, from the segment bounds the hour
// already loaded. Segments are recorded intervals; the space between two of
// them counts as a gap only when it exceeds the collector's ordinary pause.
export function rangeCoverage(range: ExportRange, segments: readonly SegmentBound[], gapToleranceSeconds = 60): RangeCoverage {
  const intervals = segments
    .map((segment) => ({ from: Math.floor(segment.minTs / MICROS), to: Math.floor(segment.maxTs / MICROS) }))
    .filter((interval) => interval.to >= range.from && interval.from <= range.to)
    .map((interval) => ({ from: Math.max(interval.from, range.from), to: Math.min(interval.to, range.to) }))
    .sort((left, right) => left.from - right.from)
  if (intervals.length === 0) return { recorded: null, gaps: [] }
  const gaps: ExportRange[] = []
  let recordedTo = intervals[0]?.to ?? range.from
  for (const interval of intervals.slice(1)) {
    if (interval.from - recordedTo > gapToleranceSeconds) gaps.push({ from: recordedTo + 1, to: interval.from - 1 })
    recordedTo = Math.max(recordedTo, interval.to)
  }
  return {
    recorded: { from: intervals[0]?.from ?? range.from, to: recordedTo },
    gaps,
  }
}

// The part of the range the current hour's timeline can show, in microseconds.
export function rangeOnHour(range: ExportRange, hourMicros: number): { readonly from: number; readonly to: number } | null {
  const from = Math.max(range.from * MICROS, hourMicros)
  const to = Math.min((range.to + 1) * MICROS, hourMicros + HOUR_SECONDS * MICROS)
  return from < to ? { from, to } : null
}

export const EXPORT_SECONDS_KEY = "kronika.export-seconds"

export function readLastExportSeconds(storage: Pick<Storage, "getItem">): number | null {
  try {
    const stored = storage.getItem(EXPORT_SECONDS_KEY)
    if (stored === null) return null
    const seconds = Number(stored)
    return Number.isFinite(seconds) && seconds > 0 ? seconds : null
  } catch {
    return null
  }
}

export function writeLastExportSeconds(storage: Pick<Storage, "setItem">, seconds: number): void {
  try {
    storage.setItem(EXPORT_SECONDS_KEY, String(Math.round(seconds * 10) / 10))
  } catch {}
}
