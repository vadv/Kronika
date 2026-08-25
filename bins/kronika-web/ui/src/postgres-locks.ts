import type { DataRow } from "./api"
import { asNumber, value } from "./model"

// The graph product supplies canonical rows in parent-first order. The UI only
// adds the connector prefix used to present those rows.
export function decorateLockGraph(rows: readonly DataRow[]): readonly DataRow[] {
  const byPid = new Map<number, DataRow>()
  const children = new Map<number, number[]>()
  for (const row of rows) {
    const pid = asNumber(value(row, "pid"))
    const parent = asNumber(value(row, "lock_tree_parent_pid"))
    if (pid !== null) byPid.set(pid, row)
    if (pid !== null && parent !== null) {
      const siblings = children.get(parent)
      if (siblings === undefined) children.set(parent, [pid])
      else siblings.push(pid)
    }
  }
  const lastChild = new Map([...children].flatMap(([parent, pids]) => {
    const pid = pids.at(-1)
    return pid === undefined ? [] : [[parent, pid] as const]
  }))

  const prefix = (row: DataRow): string => {
    const pid = asNumber(value(row, "pid"))
    const depth = asNumber(value(row, "lock_tree_depth")) ?? 1
    const parent = asNumber(value(row, "lock_tree_parent_pid"))
    if (pid === null || parent === null || depth <= 1) return ""

    const ancestors: number[] = []
    const seen = new Set<number>()
    let current: number | null = parent
    while (current !== null && ancestors.length < depth - 1 && !seen.has(current)) {
      seen.add(current)
      ancestors.push(current)
      const ancestor = byPid.get(current)
      current = ancestor === undefined ? null : asNumber(value(ancestor, "lock_tree_parent_pid"))
    }
    const path = ancestors.reverse().slice(1).map((ancestorPid) => {
      const ancestor = byPid.get(ancestorPid)
      const ancestorParent = ancestor === undefined ? null : asNumber(value(ancestor, "lock_tree_parent_pid"))
      return ancestorParent !== null && lastChild.get(ancestorParent) === ancestorPid ? "   " : "│  "
    })
    return `${path.join("")}${lastChild.get(parent) === pid ? "└─ " : "├─ "}`
  }

  return rows.map((row) => ({
    ...row,
    values: {
      ...row.values,
      lock_tree_prefix: prefix(row),
    },
  }))
}
