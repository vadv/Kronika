const COMPACT_NODE_TYPES: Readonly<Record<string, string>> = {
  a: "Result", b: "ModifyTable", c: "Append", d: "Merge Append", e: "Recursive Union",
  f: "BitmapAnd", g: "BitmapOr", h: "Seq Scan", i: "Index Scan", j: "Index Only Scan",
  k: "Bitmap Index Scan", l: "Bitmap Heap Scan", m: "Tid Scan", n: "Subquery Scan",
  o: "Function Scan", p: "Values Scan", q: "CTE Scan", r: "WorkTable Scan",
  s: "Foreign Scan", t: "Nested Loop", u: "Merge Join", v: "Hash Join",
  w: "Materialize", x: "Sort", y: "Group", z: "Aggregate", "0": "WindowAgg",
  "1": "Unique", "2": "Hash", "3": "SetOp", "4": "LockRows", "5": "Limit",
  B: "Sample Scan", "6": "Gather", "7": "ProjectSet", "8": "Table Function Scan",
  "9": "Named Tuplestore Scan", A: "Gather Merge", C: "Incremental Sort",
  D: "Tid Range Scan", E: "Memoize",
}

const COMPACT_ENUMS: Readonly<Record<string, Readonly<Record<string, string>>>> = {
  d: { b: "Backward", f: "Forward", n: "NoMovement" },
  g: { h: "Hashed", m: "Mixed", p: "Plain", s: "Sorted" },
  j: { a: "Anti", f: "Full", i: "Inner", l: "Left", r: "Right", s: "Semi" },
  "!": { d: "Delete", i: "Insert", u: "Update" },
}

const COMPACT_ATTRIBUTES: readonly (readonly [string, string])[] = [
  ["q", "Subplan"], ["a", "Alias"], ["f", "Function"], ["c", "CTE"], ["d", "Scan direction"], ["g", "Strategy"], ["j", "Join type"], ["!", "Operation"],
  ["1", "Startup cost"], ["2", "Total cost"], ["3", "Plan rows"], ["4", "Plan width"],
  ["A", "Actual startup time"], ["B", "Actual total time"], ["C", "Actual rows"], ["D", "Actual loops"],
  ["{", "Workers planned"], ["}", "Workers launched"],
  ["5", "Filter"], ["6", "Join filter"], ["7", "Hash condition"], ["8", "Index condition"],
  ["9", "TID condition"], ["0", "Recheck condition"], ["k", "Sort key"], ["o", "Output"],
  ["F", "Shared hit blocks"], ["G", "Shared read blocks"], ["H", "Shared dirtied blocks"], ["I", "Shared written blocks"],
]

export interface PlanAttribute {
  readonly label: string
  readonly value: string
}

export interface PlanNode {
  readonly attributes: readonly PlanAttribute[]
  readonly children: readonly PlanNode[]
  readonly index: string | null
  readonly nodeType: string
  readonly relation: string | null
}

export type PlanPresentation =
  | { readonly kind: "tree"; readonly root: PlanNode; readonly summary: string }
  | { readonly kind: "text"; readonly lines: readonly string[]; readonly summary: string }
  | { readonly kind: "raw"; readonly summary: string }

export function presentPlan(raw: string): PlanPresentation {
  const text = raw.trim()
  if (text === "") return { kind: "raw", summary: "—" }
  const parsed = parseJson(text)
  const root = parsed === null ? compactRootFromChopped(text) : planRoot(parsed)
  if (root !== null) {
    const node = parsePlanNode(root)
    if (node !== null) return { kind: "tree", root: node, summary: planSummary(node) }
  }
  if (looksLikeTextPlan(text)) {
    const lines = text.split(/\r?\n/)
    return { kind: "text", lines, summary: textPlanSummary(lines) }
  }
  return { kind: "raw", summary: text.length > 84 ? `${text.slice(0, 81)}…` : text }
}

export function planSummary(node: PlanNode): string {
  const subject = node.relation === null ? "" : ` on ${node.relation}`
  const index = node.index === null ? "" : ` using ${node.index}`
  const children = node.children.slice(0, 2).map((child) => child.nodeType)
  const descent = children.length === 0 ? "" : ` → ${children.join(" + ")}${node.children.length > 2 ? ` +${node.children.length - 2}` : ""}`
  return `${node.nodeType}${subject}${index}${descent}`
}

function parseJson(text: string): unknown | null {
  try { return JSON.parse(text) as unknown } catch { return null }
}

function planRoot(value: unknown): Record<string, unknown> | null {
  if (Array.isArray(value)) return value.length === 0 ? null : planRoot(value[0])
  if (!record(value)) return null
  const root = value.Plan ?? value.plan ?? value.p
  if (record(root)) return root
  return nodeType(value) === null ? null : value
}

// pg_store_plans' compact stream can carry adjacent top-level fragments. The
// plan object itself is still exact JSON, so isolate that balanced value and
// leave every unrecognised suffix available through Raw.
function compactRootFromChopped(text: string): Record<string, unknown> | null {
  const marker = text.indexOf('"p"')
  if (marker < 0) return null
  const start = text.indexOf("{", marker + 3)
  if (start < 0) return null
  let depth = 0
  let quoted = false
  let escaped = false
  for (let index = start; index < text.length; index += 1) {
    const character = text[index]!
    if (quoted) {
      if (escaped) escaped = false
      else if (character === "\\") escaped = true
      else if (character === '"') quoted = false
      continue
    }
    if (character === '"') quoted = true
    else if (character === "{") depth += 1
    else if (character === "}" && --depth === 0) {
      const parsed = parseJson(text.slice(start, index + 1))
      return record(parsed) ? parsed : null
    }
  }
  return null
}

function parsePlanNode(source: Record<string, unknown>): PlanNode | null {
  const type = nodeType(source)
  if (type === null) return null
  const compact = typeof source.t === "string" && COMPACT_NODE_TYPES[source.t] !== undefined
  const relationName = stringValue(source, compact ? "n" : "Relation Name")
  const schemaName = stringValue(source, compact ? "s" : "Schema")
  const relation = relationName === null ? null : schemaName === null ? relationName : `${schemaName}.${relationName}`
  const index = stringValue(source, compact ? "i" : "Index Name")
  const childValue = source[compact ? "l" : "Plans"]
  const children = Array.isArray(childValue) ? childValue.flatMap((child) => {
    const parsed = record(child) ? parsePlanNode(child) : null
    return parsed === null ? [] : [parsed]
  }) : []
  const attributes = compact ? compactAttributes(source) : explainAttributes(source)
  return { attributes, children, index, nodeType: type, relation }
}

function nodeType(source: Record<string, unknown>): string | null {
  const long = stringValue(source, "Node Type")
  if (long !== null) return long
  const compact = stringValue(source, "t")
  return compact === null ? null : COMPACT_NODE_TYPES[compact] ?? null
}

function compactAttributes(source: Record<string, unknown>): readonly PlanAttribute[] {
  return COMPACT_ATTRIBUTES.flatMap(([key, label]) => {
    const stored = source[key]
    if (!factual(stored)) return []
    const mapped = typeof stored === "string" ? COMPACT_ENUMS[key]?.[stored] ?? stored : stored
    return [{ label, value: attributeValue(mapped) }]
  })
}

const EXPLAIN_IGNORED = new Set(["Node Type", "Plans", "Relation Name", "Schema", "Index Name", "Parent Relationship"])
const EXPLAIN_ATTRIBUTES = [
  "Operation", "Strategy", "Join Type", "Scan Direction", "Subplan Name", "Alias", "Function Name", "CTE Name",
  "Startup Cost", "Total Cost", "Plan Rows", "Plan Width", "Actual Startup Time", "Actual Total Time", "Actual Rows", "Actual Loops",
  "Workers Planned", "Workers Launched", "Filter", "Rows Removed by Filter", "Join Filter", "Hash Cond", "Index Cond", "Recheck Cond",
  "Sort Key", "Sort Method", "Group Key", "Output", "Shared Hit Blocks", "Shared Read Blocks", "Shared Dirtied Blocks", "Shared Written Blocks",
] as const

function explainAttributes(source: Record<string, unknown>): readonly PlanAttribute[] {
  const preferred = EXPLAIN_ATTRIBUTES.flatMap((label) => factual(source[label]) ? [{ label, value: attributeValue(source[label]) }] : [])
  const known = new Set([...EXPLAIN_IGNORED, ...EXPLAIN_ATTRIBUTES])
  const remaining = Object.entries(source).flatMap(([label, stored]) => known.has(label) || !factual(stored) || record(stored)
    ? [] : [{ label, value: attributeValue(stored) }])
  return [...preferred, ...remaining]
}

function factual(value: unknown): boolean {
  return typeof value === "string" || typeof value === "number" || typeof value === "boolean"
    || Array.isArray(value) && value.every((entry) => factual(entry))
}

function attributeValue(value: unknown): string {
  if (Array.isArray(value)) return value.map(attributeValue).join(", ")
  return String(value)
}

function stringValue(source: Record<string, unknown>, key: string): string | null {
  return typeof source[key] === "string" && source[key] !== "" ? source[key] : null
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function looksLikeTextPlan(text: string): boolean {
  const first = text.split(/\r?\n/, 1)[0] ?? ""
  return /\b(cost=|actual time=|rows=\d+|loops=\d+)\b/.test(first)
    || /^(?:\s*->\s*)?[A-Z][A-Za-z ]+(?:\s+on\s+\S+)?/.test(first) && text.includes("\n")
}

function textPlanSummary(lines: readonly string[]): string {
  const first = lines.find((line) => line.trim() !== "")?.trim().replace(/^->\s*/, "") ?? "—"
  const withoutFacts = first.replace(/\s+\((?:cost|actual time)=[^)]+\).*$/, "")
  return withoutFacts.length > 84 ? `${withoutFacts.slice(0, 81)}…` : withoutFacts
}
