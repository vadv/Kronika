export interface Address {
  readonly at: number | null
  readonly view: View
  readonly lens: Lens
  readonly pgLens: PgLens
  readonly pgLevel: PgLevel
  readonly datid: string | null
  readonly schema: string | null
  readonly relid: string | null
  readonly indexrelid: string | null
  readonly sort: { readonly column: string; readonly descending: boolean } | null
  readonly row: string | null
  readonly find: string
}

export type View =
  | "host.system"
  | "host.processes"
  | "pg.overview"
  | "pg.activity"
  | "pg.statements"
  | "pg.plans"
  | "pg.locks"
  | "pg.databases"
  | "pg.tables"
  | "pg.indexes"
  | "events"

type Lens = "generic" | "cpu" | "memory" | "disk"
export type PgLens = "load" | "per_call" | "io" | "resources" | "stability" | "timing" | "identity"
  | "access" | "changes" | "maintenance" | "size_buffers" | "freeze"
  | "usage" | "low_activity" | "state"
export type PgLevel = "database" | "schema" | "object"

const VIEWS: readonly View[] = [
  "host.system", "host.processes",
  "pg.overview", "pg.activity", "pg.statements", "pg.plans", "pg.locks", "pg.databases", "pg.tables", "pg.indexes",
  "events",
]

const LENSES: readonly Lens[] = ["generic", "cpu", "memory", "disk"]
const PG_LENSES: readonly PgLens[] = ["load", "per_call", "io", "resources", "stability", "timing", "identity", "access", "changes", "maintenance", "size_buffers", "freeze", "usage", "low_activity", "state"]
const PG_LEVELS: readonly PgLevel[] = ["database", "schema", "object"]

export const DEFAULT_ADDRESS: Address = {
  at: null,
  view: "host.processes",
  lens: "cpu",
  pgLens: "load",
  pgLevel: "database",
  datid: null,
  schema: null,
  relid: null,
  indexrelid: null,
  sort: null,
  row: null,
  find: "",
}

export function readAddress(search: string): Address {
  const parameters = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search)
  const at = Number.parseInt(parameters.get("at") ?? "", 10)
  const view = parameters.get("view")
  const lens = parameters.get("lens")
  const pgLens = parameters.get("pg_lens")
  const pgLevel = PG_LEVELS.find((known) => known === parameters.get("level")) ?? "database"
  const resolvedView = VIEWS.find((known) => known === view) ?? DEFAULT_ADDRESS.view
  const oid = (name: string) => /^[1-9]\d*$/.test(parameters.get(name) ?? "") ? parameters.get(name) : null
  const relation = resolvedView === "pg.tables" || resolvedView === "pg.indexes"
  const datid = relation && pgLevel !== "database" ? oid("datid") : null
  const sort = parameters.get("sort") ?? ""
  const column = sort.startsWith("-") ? sort.slice(1) : sort
  return {
    at: Number.isSafeInteger(at) && at > 0 ? at : null,
    view: resolvedView,
    lens: LENSES.find((known) => known === lens) ?? DEFAULT_ADDRESS.lens,
    pgLens: PG_LENSES.find((known) => known === pgLens) ?? DEFAULT_ADDRESS.pgLens,
    pgLevel: !relation || pgLevel === "database" || datid === null ? "database" : pgLevel,
    datid,
    schema: relation && pgLevel === "object" && datid !== null ? parameters.get("schema") : null,
    relid: relation && pgLevel === "object" && datid !== null ? oid("relid") : null,
    indexrelid: resolvedView === "pg.indexes" && pgLevel === "object" && datid !== null ? oid("indexrelid") : null,
    sort: column === "" ? null : { column, descending: sort.startsWith("-") },
    row: resolvedView === "host.processes" || relation ? parameters.get("row") : null,
    find: parameters.get("find") ?? "",
  }
}

export function writeAddress(address: Address): string {
  const parameters = new URLSearchParams()
  const relation = address.view === "pg.tables" || address.view === "pg.indexes"
  if (address.at !== null) parameters.set("at", String(address.at))
  if (address.view !== DEFAULT_ADDRESS.view) parameters.set("view", address.view)
  if (address.lens !== DEFAULT_ADDRESS.lens && address.view === "host.processes") parameters.set("lens", address.lens)
  if (address.pgLens !== DEFAULT_ADDRESS.pgLens && (relation || address.view === "pg.statements" || address.view === "pg.plans")) parameters.set("pg_lens", address.pgLens)
  if (relation && address.pgLevel !== "database") parameters.set("level", address.pgLevel)
  if (relation && address.pgLevel !== "database" && address.datid !== null) parameters.set("datid", address.datid)
  if (relation && address.pgLevel === "object" && address.schema !== null) parameters.set("schema", address.schema)
  if (relation && address.pgLevel === "object" && address.relid !== null) parameters.set("relid", address.relid)
  if (address.view === "pg.indexes" && address.pgLevel === "object" && address.indexrelid !== null) parameters.set("indexrelid", address.indexrelid)
  if (address.sort !== null) parameters.set("sort", `${address.sort.descending ? "-" : ""}${address.sort.column}`)
  if ((address.view === "host.processes" || relation)
    && address.row !== null && address.row !== "") parameters.set("row", address.row)
  if (address.find !== "") parameters.set("find", address.find)
  const query = parameters.toString()
  return query === "" ? "/" : `/?${query}`
}

export function viewOf(source: string, hostSection: string, pgSection: string): View {
  if (source === "events") return "events"
  if (source === "postgresql") return `pg.${pgSection}` as View
  return hostSection === "system" ? "host.system" : "host.processes"
}

export function sourceOf(view: View): "host" | "postgresql" | "events" {
  if (view === "events") return "events"
  return view.startsWith("pg.") ? "postgresql" : "host"
}

export function hostSectionOf(view: View): "system" | "processes" {
  return view === "host.system" ? "system" : "processes"
}

export function pgSectionOf(view: View): "overview" | "activity" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes" {
  const section = view.startsWith("pg.") ? view.slice(3) : "overview"
  return section as "overview" | "activity" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes"
}

export function stepOf(address: string): string {
  return address.replace(/(\?|&)at=\d+&?/, "$1").replace(/[?&]$/, "")
}
