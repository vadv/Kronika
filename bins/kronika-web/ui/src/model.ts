import type { Cell, DataRow } from "./api"

export type Locale = "en" | "ru"
export type Lens = "generic" | "cpu" | "memory" | "disk"

export function processLens(field: string | null): Lens {
  if (["rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt"].includes(field ?? "")) return "memory"
  if (["read_bytes", "write_bytes", "cancelled_write_bytes", "syscr", "syscw", "rchar", "wchar"].includes(field ?? "")) return "disk"
  if (["utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "nice", "prio", "rtprio", "policy", "curcpu"].includes(field ?? "")) return "cpu"
  return "generic"
}

export function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}

export function inputDay(timestamp: number): string {
  return new Date(timestamp / 1_000).toISOString().slice(0, 10)
}

export function inputHour(timestamp: number): number {
  return new Date(timestamp / 1_000).getUTCHours()
}

export function selectedHour(day: string, hour: number): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day)
  if (match === null) return null
  const year = Number(match[1])
  const month = Number(match[2])
  const date = Number(match[3])
  const milliseconds = Date.UTC(year, month - 1, date, hour)
  return Number.isFinite(milliseconds) ? milliseconds * 1_000 : null
}

export function formatUtc(timestamp: number | null): string {
  if (timestamp === null || !Number.isFinite(timestamp)) return "—"
  return new Date(Math.trunc(timestamp / 1_000)).toISOString().replace("T", " ").replace("Z", " UTC")
}

export function shortUtc(timestamp: number): string {
  return new Date(Math.trunc(timestamp / 1_000)).toISOString().slice(11, 23)
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

export function nearestRow(rows: readonly DataRow[], target: number): DataRow | null {
  const timestamp = nearestTime(rows, target)
  return timestamp === null ? null : rows.find((row) => row.timestamp === timestamp) ?? null
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
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(number)}${suffix}`
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
