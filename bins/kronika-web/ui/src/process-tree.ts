import type { DataRow } from "./api"
import { asNumber, value } from "./model"

// os_process rows arrive as a server-ranked, paginated page — the opposite
// of what a tree needs. The tree lens fetches every process at one moment
// instead (app.tsx) and hands the flat list here to walk by ppid, exactly
// like ps -f: parent first, each child indented directly under it, natural
// process order rather than any ranking.

export interface ProcessMetricInputs {
  /** Wall seconds between the two recorded snapshots being compared. */
  readonly intervalSeconds: number | null
  readonly memTotalKb: number | null
  /** `utime + stime` at the previous recorded snapshot, by pid. */
  readonly previousTicks: ReadonlyMap<number, number>
  readonly ticksPerSecond: number | null
}

export function scheduledTicks(row: DataRow): number | null {
  const utime = asNumber(value(row, "utime"))
  const stime = asNumber(value(row, "stime"))
  return utime === null || stime === null ? null : utime + stime
}

// %CPU is what top shows: the share of one core this process burned between
// the two recorded snapshots, not the ps lifetime average -- a backend that
// pinned a core for the last interval but has run for days averages to zero,
// which is what made the column read 0% on a busy host. TIME stays the
// lifetime total, as it is in both tools. Without a preceding snapshot there
// is no interval to divide by and %CPU is missing rather than guessed.
function metrics(row: DataRow, pid: number | null, inputs: ProcessMetricInputs): Record<string, number | null> {
  const { intervalSeconds, memTotalKb, previousTicks, ticksPerSecond } = inputs
  const ticks = scheduledTicks(row)
  const before = pid === null ? undefined : previousTicks.get(pid)
  const usable = ticksPerSecond !== null && ticksPerSecond > 0
  const burned = ticks === null || before === undefined || ticks < before ? null : ticks - before
  const rmemKb = asNumber(value(row, "rmem_kb"))
  return {
    cpu_percent: burned === null || !usable || intervalSeconds === null || intervalSeconds <= 0
      ? null
      : (burned / ticksPerSecond / intervalSeconds) * 100,
    cpu_time_seconds: ticks === null || !usable ? null : ticks / ticksPerSecond,
    mem_percent: rmemKb === null || memTotalKb === null || memTotalKb <= 0 ? null : (rmemKb / memTotalKb) * 100,
  }
}

export function buildProcessForest(
  rows: readonly DataRow[],
  inputs: ProcessMetricInputs,
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
      values: { ...row.values, ...metrics(row, pid, inputs), process_tree_prefix: prefix },
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
