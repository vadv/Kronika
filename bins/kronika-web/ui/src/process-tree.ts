import type { DataRow } from "./api"
import { asNumber, value } from "./model"

// The tree lens supplies canonical rows in parent-first order. The UI only
// adds cells needed to present the ps-shaped table.

// A snapshot row carries the cumulative columns as rates: the layout record
// names them under `rates`, so utime and stime arrive as jiffies per second.
// Lifetime CPU time arrives separately, as cpu_time_ticks.
function metrics(row: DataRow, memTotalKb: number | null, ticksPerSecond: number | null): Record<string, number | string | null> {
  const userTicks = asNumber(value(row, "utime"))
  const systemTicks = asNumber(value(row, "stime"))
  const cores = userTicks === null || systemTicks === null || ticksPerSecond === null || ticksPerSecond <= 0
    ? null
    : (userTicks + systemTicks) / ticksPerSecond
  const rmemKb = asNumber(value(row, "rmem_kb"))
  const lifetimeTicks = asNumber(value(row, "cpu_time_ticks"))
  return {
    cpu_percent: cores === null ? null : cores * 100,
    cpu_time_seconds: lifetimeTicks === null || ticksPerSecond === null || ticksPerSecond <= 0 ? null : lifetimeTicks / ticksPerSecond,
    mem_percent: rmemKb === null || memTotalKb === null || memTotalKb <= 0 ? null : (rmemKb / memTotalKb) * 100,
    process_stat: processStat(row),
  }
}

// The STAT column of ps: the state letter, then the flags this collection can
// tell. Session leadership and foreground group are not recorded, so `s` and
// `+` never appear.
function processStat(row: DataRow): string | null {
  const state = asNumber(value(row, "state"))
  if (state === null) return null
  const nice = asNumber(value(row, "nice"))
  const threads = asNumber(value(row, "num_threads"))
  const priority = nice === null || nice === 0 ? "" : nice < 0 ? "<" : "N"
  return `${String.fromCharCode(state)}${priority}${threads !== null && threads > 1 ? "l" : ""}`
}

export function decorateProcessTree(
  rows: readonly DataRow[],
  memTotalKb: number | null,
  ticksPerSecond: number | null,
): readonly DataRow[] {
  const byPid = new Map<number, DataRow>()
  const children = new Map<number, number[]>()
  for (const row of rows) {
    const pid = asNumber(value(row, "pid"))
    const parent = asNumber(value(row, "process_tree_parent_pid"))
    if (pid !== null) byPid.set(pid, row)
    if (pid !== null && parent !== null) {
      const list = children.get(parent)
      if (list === undefined) children.set(parent, [pid])
      else list.push(pid)
    }
  }

  const lastChild = new Map([...children].flatMap(([parent, pids]) => {
    const pid = pids.at(-1)
    return pid === undefined ? [] : [[parent, pid] as const]
  }))
  const prefix = (row: DataRow): string => {
    const depth = asNumber(value(row, "process_tree_depth")) ?? 0
    if (depth <= 0) return ""
    const ancestors: number[] = []
    const seen = new Set<number>()
    let parent = asNumber(value(row, "process_tree_parent_pid"))
    while (parent !== null && ancestors.length < depth - 1 && !seen.has(parent)) {
      seen.add(parent)
      const ancestor = byPid.get(parent)
      if (ancestor === undefined) break
      ancestors.push(parent)
      parent = asNumber(value(ancestor, "process_tree_parent_pid"))
    }
    const bars = ancestors.reverse().map((pid) => {
      const ancestor = byPid.get(pid)
      const ancestorParent = ancestor === undefined ? null : asNumber(value(ancestor, "process_tree_parent_pid"))
      return ancestorParent !== null && lastChild.get(ancestorParent) === pid ? "    " : "|   "
    })
    return ` ${bars.join("")}\\_ `
  }

  return rows.map((row) => ({
    ...row,
    values: {
      ...row.values,
      ...metrics(row, memTotalKb, ticksPerSecond),
      process_tree_prefix: prefix(row),
    },
  }))
}
