import type { Cell, DataRow } from "./api"
import type { Translate } from "./help"

export type Locale = "en" | "ru"
export type Lens = "generic" | "cpu" | "memory" | "disk"

export function shownMoment(
  sections: Readonly<Record<string, readonly DataRow[]>>,
  cursor: number,
): number | null {
  let latest: number | null = null
  for (const rows of Object.values(sections)) {
    for (const row of rows) {
      if (row.timestamp <= cursor && (latest === null || row.timestamp > latest)) latest = row.timestamp
    }
  }
  return latest
}

export function processLens(field: string | null): Lens {
  if (["rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt"].includes(field ?? "")) return "memory"
  if (["read_bytes", "write_bytes", "cancelled_write_bytes", "syscr", "syscw", "rchar", "wchar"].includes(field ?? "")) return "disk"
  if (["utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "nice", "prio", "rtprio", "policy", "curcpu"].includes(field ?? "")) return "cpu"
  return "generic"
}

export function processDefaultSort(lens: Lens, rows: readonly DataRow[]): string {
  if (lens === "cpu") return "utime"
  if (lens === "memory") return "rmem_kb"
  if (lens !== "disk") return "pid"
  const fields = ["read_bytes", "write_bytes", "syscr", "syscw"] as const
  return fields.find((field) => rows.some((row) => (asNumber(value(row, field)) ?? 0) > 0))
    ?? fields.find((field) => rows.some((row) => asNumber(value(row, field)) !== null))
    ?? fields[0]
}

export function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}

export interface TimePair {
  readonly primary: string
  readonly secondary: string | null
}

export function nearestTime(rows: readonly DataRow[], target: number): number | null {
  let nearest: number | null = null
  let distance = Number.POSITIVE_INFINITY
  for (const row of rows) {
    const candidate = Math.abs(row.timestamp - target)
    if (candidate < distance || (candidate === distance && (nearest === null || row.timestamp < nearest))) {
      nearest = row.timestamp
      distance = candidate
    }
  }
  return nearest
}

export function snapshot(rows: readonly DataRow[], target: number): readonly DataRow[] {
  const timestamp = nearestTime(rows, target)
  return timestamp === null ? [] : rows.filter((row) => row.timestamp === timestamp)
}

export function value(row: DataRow | null, field: string): Cell {
  return row?.values[field] ?? null
}

export function asNumber(cell: Cell): number | null {
  if (typeof cell === "number") return Number.isFinite(cell) ? cell : null
  if (typeof cell === "string" && cell.trim() !== "") {
    const number = Number(cell)
    return Number.isFinite(number) ? number : null
  }
  return null
}

export function rawText(cell: Cell): string | null {
  if (typeof cell === "string") return cell
  if (typeof cell === "number" || typeof cell === "boolean") return String(cell)
  if (cell === null) return null
  if (Array.isArray(cell)) return JSON.stringify(cell)
  const payload = cell as Readonly<Record<string, unknown>>
  if (payload.representation === "text" && typeof payload.stored_text === "string") return payload.stored_text
  if (payload.representation === "bytes") {
    if (typeof payload.stored_base64 === "string") return payload.stored_base64
    if (typeof payload.base64 === "string") return payload.base64
  }
  return JSON.stringify(payload)
}

export function processCommand(row: DataRow): string {
  const command = rawText(value(row, "cmdline"))
  if (command !== null && command.trim() !== "") return command
  return rawText(value(row, "comm")) ?? "—"
}

export function processKey(row: DataRow): string {
  return `${identifier(value(row, "pid"))}:${identifier(value(row, "starttime"))}`
}

export function identifier(cell: Cell): string {
  return rawText(cell) ?? "—"
}

export function stateText(cell: Cell): string {
  const number = asNumber(cell)
  return number === null ? identifier(cell) : String.fromCharCode(number)
}

export function measure(cell: Cell, locale: Locale, suffix = ""): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  return `${compact(number, locale)}${suffix}`
}

export function humanPercent(cell: Cell, locale: Locale): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  return `${compact(number, locale)}${locale === "ru" ? "\u00a0" : ""}%`
}

export function humanDuration(cell: Cell, locale: Locale): string {
  const milliseconds = asNumber(cell)
  if (milliseconds === null) return "—"
  const units = locale === "ru"
    ? { "hour": "ч", minute: "м", millisecond: "мс", "second": "с" }
    : { "hour": "h", minute: "m", millisecond: "ms", "second": "s" }
  if (Math.abs(milliseconds) < 1_000) return `${compact(milliseconds, locale)} ${units.millisecond}`
  if (Math.abs(milliseconds) < 60_000) return `${decimals(Math.trunc(milliseconds / 100) / 10, locale, 1)} ${units.second}`
  const seconds = Math.floor(Math.abs(milliseconds) / 1_000)
  const sign = milliseconds < 0 ? "−" : ""
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${sign}${minutes}${units.minute} ${String(seconds % 60).padStart(2, "0")}${units.second}`
  const hours = Math.floor(minutes / 60)
  if (hours >= 24) return `${sign}${Math.floor(hours / 24)}${locale === "ru" ? "д" : "d"} ${String(hours % 24).padStart(2, "0")}${units.hour}`
  return `${sign}${hours}${units.hour} ${String(minutes % 60).padStart(2, "0")}${units.minute}`
}

export function compact(value: number, locale: Locale): string {
  const abs = Math.abs(value), notation = abs >= 1e15 || abs > 0 && abs < 1e-6 ? "scientific" : abs >= 1e3 ? "compact" : undefined
  return isFinite(value) ? new Intl.NumberFormat(locale, { maximumSignificantDigits: 3, notation }).format(value) : "—"
}

export function estimatedRows(cell: Cell, locale: Locale, t: Translate): TimePair | null {
  if (!(typeof cell === "number" && Number.isSafeInteger(cell) || typeof cell === "string" && /^\d+$/.test(cell))) return null
  const value = BigInt(cell)
  if (value < 0n) return null
  const compactKind = value >= 1_000n ? "many" : rowPlural(value, locale)
  const exactKind = rowPlural(value, locale)
  const notation = value >= 1_000_000_000_000_000n ? "scientific" : value >= 1_000n ? "compact" : undefined
  const compactValue = new Intl.NumberFormat(locale, { maximumSignificantDigits: 3, notation }).format(value)
  const exactValue = new Intl.NumberFormat(locale).format(value)
  return {
    primary: t(`unit.estimated_rows.${compactKind}`, { value: compactValue }),
    secondary: t(`unit.estimated_rows.${exactKind}`, { value: exactValue }),
  }
}

function decimals(value: number, locale: Locale, digits: number): string {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value)
}

function rowPlural(value: bigint, locale: Locale): "one" | "few" | "many" {
  if (locale === "en") return value === 1n ? "one" : "many"
  const last = value % 10n, lastTwo = value % 100n
  if (last === 1n && lastTwo !== 11n) return "one"
  return last >= 2n && last <= 4n && (lastTwo < 12n || lastTwo > 14n) ? "few" : "many"
}

const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"] as const

export function humanBytes(cell: Cell, locale: Locale, suffix = ""): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  const sign = number < 0 ? "-" : ""
  let scaled = Math.abs(number)
  let step = 0
  while (scaled >= 1024 && step < BYTE_UNITS.length - 1) {
    scaled /= 1024
    step += 1
  }
  const output = step === 0 && !Number.isInteger(scaled)
    ? compact(scaled, locale)
    : new Intl.NumberFormat(locale, { maximumFractionDigits: scaled >= 100 || step === 0 ? 0 : 1 }).format(scaled)
  return `${sign}${output} ${BYTE_UNITS[step]}${suffix}`
}

export function cores(cell: Cell, locale: Locale, ticksPerSecond: number | null): string {
  const number = asNumber(cell)
  if (number === null || ticksPerSecond === null || ticksPerSecond <= 0) return "—"
  return compact(number / ticksPerSecond, locale)
}

export function millisecondsPerSecond(cell: Cell, locale: Locale): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  return compact(number / 1_000_000, locale)
}

export function activityFor(
  process: DataRow | null,
  activities: readonly DataRow[],
  cursor: number,
): { readonly row: DataRow | null; readonly snapshotTime: number | null } {
  if (process === null) return { row: null, snapshotTime: null }
  const activitySnapshot = snapshot(activities, cursor)
  const snapshotTime = activitySnapshot[0]?.timestamp ?? null
  const pid = asNumber(value(process, "pid"))
  const row = pid === null
    ? null
    : activitySnapshot.find((activity) => asNumber(value(activity, "pid")) === pid) ?? null
  return { row, snapshotTime }
}

export function interpolate(template: string, slots: Readonly<Record<string, string | number>>): string {
  return template.replace(/\{([a-z][a-z0-9_]*)\}/gi, (_whole, name: string) => String(slots[name] ?? `{${name}}`))
}
