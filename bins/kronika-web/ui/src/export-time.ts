import type { DisplayTimeZone } from "./display-time"

const MICROSECONDS_PER_SECOND = 1_000_000
const SECONDS_PER_HOUR = 3_600
const LOCAL_DATE_TIME = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/

export interface ExportRangeDefaults {
  readonly from: string
  readonly fromSecond: number
  readonly to: string
  readonly toSecond: number
}

export type ExportRangeError = "required" | "invalid" | "order"

export type ParsedExportRange =
  | { readonly ok: true; readonly from: number; readonly to: number }
  | { readonly ok: false; readonly error: ExportRangeError }

export function exportRangeDefaults(hourMicroseconds: number, mode: DisplayTimeZone): ExportRangeDefaults {
  if (!Number.isSafeInteger(hourMicroseconds) || hourMicroseconds % MICROSECONDS_PER_SECOND !== 0) {
    throw new RangeError("the selected hour must have whole-second precision")
  }
  const selectedSecond = hourMicroseconds / MICROSECONDS_PER_SECOND
  const selected = new Date(selectedSecond * 1_000)
  const fromSecond = mode === "utc"
    ? selectedSecond
    : selectedSecond - selected.getMinutes() * 60 - selected.getSeconds()
  const toSecond = fromSecond + SECONDS_PER_HOUR - 1
  return {
    from: formatExportSecond(fromSecond, mode),
    fromSecond,
    to: formatExportSecond(toSecond, mode),
    toSecond,
  }
}

export function parseExportRange(
  from: string,
  to: string,
  mode: DisplayTimeZone,
  preferred?: { readonly from?: number; readonly to?: number },
): ParsedExportRange {
  if (from.trim() === "" || to.trim() === "") return { ok: false, error: "required" }
  const parsedFrom = parseExportSecond(from, mode, preferred?.from)
  const parsedTo = parseExportSecond(to, mode, preferred?.to)
  if (parsedFrom === null || parsedTo === null) return { ok: false, error: "invalid" }
  if (parsedFrom > parsedTo) return { ok: false, error: "order" }
  return { ok: true, from: parsedFrom, to: parsedTo }
}

export function formatExportSecond(second: number, mode: DisplayTimeZone): string {
  if (!Number.isSafeInteger(second)) throw new RangeError("export time must be a safe whole second")
  const date = new Date(second * 1_000)
  if (!Number.isFinite(date.getTime())) throw new RangeError("export time is outside the browser date range")
  const read = mode === "utc"
    ? [date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), date.getUTCHours(), date.getUTCMinutes(), date.getUTCSeconds()]
    : [date.getFullYear(), date.getMonth() + 1, date.getDate(), date.getHours(), date.getMinutes(), date.getSeconds()]
  const year = read[0]
  if (year === undefined || year < 0 || year > 9_999) throw new RangeError("export time cannot be represented by the input")
  return `${String(year).padStart(4, "0")}-${two(read[1])}-${two(read[2])}T${two(read[3])}:${two(read[4])}:${two(read[5])}`
}

function parseExportSecond(text: string, mode: DisplayTimeZone, preferred?: number): number | null {
  const match = LOCAL_DATE_TIME.exec(text.trim())
  if (match === null) return null
  const values = match.slice(1).map((value) => Number(value ?? "0"))
  const [year, month, day, hour, minute, second] = values
  if (year === undefined || month === undefined || day === undefined || hour === undefined || minute === undefined || second === undefined
      || month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 59) return null
  const canonical = `${String(year).padStart(4, "0")}-${two(month)}-${two(day)}T${two(hour)}:${two(minute)}:${two(second)}`
  if (preferred !== undefined && Number.isSafeInteger(preferred) && formatExportSecond(preferred, mode) === canonical) return preferred

  const date = new Date(0)
  if (mode === "utc") {
    date.setUTCFullYear(year, month - 1, day)
    date.setUTCHours(hour, minute, second, 0)
  } else {
    date.setFullYear(year, month - 1, day)
    date.setHours(hour, minute, second, 0)
  }
  const parsed = date.getTime() / 1_000
  if (!Number.isSafeInteger(parsed)) return null
  return formatExportSecond(parsed, mode) === canonical ? parsed : null
}

function two(value: number | undefined): string {
  return String(value ?? 0).padStart(2, "0")
}
