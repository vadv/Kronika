import type { DisplayTimeZone } from "./display-time"

const MICROSECONDS_PER_SECOND = 1_000_000
const SECONDS_PER_HOUR = 3_600
const MAX_LOCAL_OFFSET_SECONDS = 24 * SECONDS_PER_HOUR
const LOCAL_DATE_TIME = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/

type CivilSecond = readonly [number, number, number, number, number, number]

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
  const fromSecond = hourMicroseconds / MICROSECONDS_PER_SECOND
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
  const values: CivilSecond = [
    Number(match[1]), Number(match[2]), Number(match[3]),
    Number(match[4]), Number(match[5]), Number(match[6] ?? "0"),
  ]
  const [year, month, day, hour, minute, second] = values
  if (month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 59) return null

  const wallSecond = utcSecond(values)
  if (wallSecond === null || !matchesCivilSecond(new Date(wallSecond * 1_000), values, true)) return null
  if (mode === "utc") return wallSecond
  if (preferred !== undefined && Number.isSafeInteger(preferred)
      && matchesCivilSecond(new Date(preferred * 1_000), values, false)) return preferred
  return resolveLocalCivilSecond(values, wallSecond, preferred)
}

function resolveLocalCivilSecond(values: CivilSecond, wallSecond: number, preferred?: number): number | null {
  const target = preferredTarget(preferred, wallSecond)
  const date = new Date(0)
  let selected: number | null = null
  let selectedDistance = Number.POSITIVE_INFINITY
  for (let offset = -MAX_LOCAL_OFFSET_SECONDS; offset <= MAX_LOCAL_OFFSET_SECONDS; offset += 1) {
    const candidate = wallSecond + offset
    date.setTime(candidate * 1_000)
    if (!matchesCivilSecond(date, values, false)) continue
    const distance = target === null ? 0 : Math.abs(candidate - target)
    if (selected === null || distance < selectedDistance) {
      selected = candidate
      selectedDistance = distance
    }
  }
  return selected
}

function preferredTarget(preferred: number | undefined, wallSecond: number): number | null {
  if (preferred === undefined || !Number.isSafeInteger(preferred)) return null
  const date = new Date(preferred * 1_000)
  if (!Number.isFinite(date.getTime())) return null
  const preferredWall = utcSecond([
    date.getFullYear(), date.getMonth() + 1, date.getDate(),
    date.getHours(), date.getMinutes(), date.getSeconds(),
  ])
  if (preferredWall === null) return null
  const target = preferred + wallSecond - preferredWall
  return Number.isSafeInteger(target) ? target : null
}

function utcSecond(values: CivilSecond): number | null {
  const [year, month, day, hour, minute, second] = values
  const date = new Date(0)
  date.setUTCFullYear(year, month - 1, day)
  date.setUTCHours(hour, minute, second, 0)
  const parsed = date.getTime() / 1_000
  return Number.isSafeInteger(parsed) ? parsed : null
}

function matchesCivilSecond(date: Date, values: CivilSecond, utc: boolean): boolean {
  if (!Number.isFinite(date.getTime())) return false
  return utc
    ? date.getUTCFullYear() === values[0] && date.getUTCMonth() + 1 === values[1] && date.getUTCDate() === values[2]
      && date.getUTCHours() === values[3] && date.getUTCMinutes() === values[4] && date.getUTCSeconds() === values[5]
    : date.getFullYear() === values[0] && date.getMonth() + 1 === values[1] && date.getDate() === values[2]
      && date.getHours() === values[3] && date.getMinutes() === values[4] && date.getSeconds() === values[5]
}

function two(value: number | undefined): string {
  return String(value ?? 0).padStart(2, "0")
}
