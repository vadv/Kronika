import type { DataRow } from "./api"
import { asNumber, value } from "./model"

// os_process rows arrive as a server-ranked, paginated page — the opposite
// of what a tree needs. The tree lens fetches every process at one moment
// instead (app.tsx) and hands the flat list here to walk by ppid, exactly
// like ps -f: parent first, each child indented directly under it, natural
// process order rather than any ranking.

// ps-classic %CPU and %MEM are lifetime averages, not instantaneous deltas,
// so a single snapshot row is enough: %CPU divides total scheduled time by
// wall time since the process started, %MEM divides resident memory by the
// host total. TIME is the same scheduled-time total, in seconds.
function metrics(
  row: DataRow,
  cursorTs: number,
  ticksPerSecond: number | null,
  memTotalKb: number | null,
): Record<string, number | null> {
  const utime = asNumber(value(row, "utime"))
  const stime = asNumber(value(row, "stime"))
  const starttime = asNumber(value(row, "starttime"))
  const cpuTimeSeconds = utime === null || stime === null || ticksPerSecond === null || ticksPerSecond <= 0
    ? null
    : (utime + stime) / ticksPerSecond
  const elapsedSeconds = starttime === null ? null : (cursorTs - starttime) / 1_000_000
  const rmemKb = asNumber(value(row, "rmem_kb"))
  return {
    cpu_percent: cpuTimeSeconds === null || elapsedSeconds === null || elapsedSeconds <= 0
      ? null
      : (cpuTimeSeconds / elapsedSeconds) * 100,
    cpu_time_seconds: cpuTimeSeconds,
    mem_percent: rmemKb === null || memTotalKb === null || memTotalKb <= 0 ? null : (rmemKb / memTotalKb) * 100,
  }
}

export function buildProcessForest(
  rows: readonly DataRow[],
  cursorTs: number,
  ticksPerSecond: number | null,
  memTotalKb: number | null,
): readonly DataRow[] {
  const byPid = new Map<number, DataRow>()
  for (const row of rows) {
    const pid = asNumber(value(row, "pid"))
    if (pid !== null) byPid.set(pid, row)
  }

  const children = new Map<number, number[]>()
  const roots: number[] = []
  for (const [pid, row] of byPid) {
    const ppid = asNumber(value(row, "ppid"))
    if (ppid !== null && ppid !== pid && byPid.has(ppid)) {
      const list = children.get(ppid)
      if (list === undefined) children.set(ppid, [pid])
      else list.push(pid)
    } else {
      roots.push(pid)
    }
  }
  roots.sort((left, right) => left - right)

  const output: DataRow[] = []
  const visited = new Set<number>()

  const walk = (pid: number, ancestorBars: readonly string[], isLastChild: boolean, depth: number): void => {
    if (visited.has(pid)) return
    visited.add(pid)
    const row = byPid.get(pid)
    if (row === undefined) return
    const prefix = depth === 0 ? "" : ancestorBars.join("") + (isLastChild ? "└─ " : "├─ ")
    output.push({
      ...row,
      values: { ...row.values, ...metrics(row, cursorTs, ticksPerSecond, memTotalKb), process_tree_prefix: prefix },
    })
    const kids = (children.get(pid) ?? []).sort((left, right) => left - right)
    const childBars = depth === 0 ? ancestorBars : [...ancestorBars, isLastChild ? "   " : "│  "]
    kids.forEach((kid, index) => walk(kid, childBars, index === kids.length - 1, depth + 1))
  }

  for (const root of roots) walk(root, [], true, 0)
  // A pid whose recorded parent didn't make it into this snapshot's roots
  // list (a cycle, or a parent this pid points to but that never got added
  // as a root) still needs to appear rather than silently vanish.
  for (const pid of byPid.keys()) if (!visited.has(pid)) walk(pid, [], true, 0)

  return output
}
