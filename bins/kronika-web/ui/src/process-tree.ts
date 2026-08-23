import type { DataRow } from "./api"
import { asNumber, value } from "./model"

// The tree lens supplies an unpaged snapshot for parent-first ordering.

export interface ProcessMetricInputs {
  readonly intervalSeconds: number | null
  readonly memTotalKb: number | null
  readonly previousTicks: ReadonlyMap<number, number>
  readonly ticksPerSecond: number | null
}

export function scheduledTicks(row: DataRow): number | null {
  const utime = asNumber(value(row, "utime"))
  const stime = asNumber(value(row, "stime"))
  return utime === null || stime === null ? null : utime + stime
}

// CPU% is interval CPU time divided by one core; TIME is lifetime CPU time.
// A missing or rolled-back baseline yields null.
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
  // Walk unvisited PIDs so cycles cannot drop rows.
  for (const pid of byPid.keys()) if (!visited.has(pid)) walk(pid, [], true, 0)

  return output
}
