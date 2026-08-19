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
  readonly tablespaceOid: string | null
  readonly sort: { readonly column: string; readonly descending: boolean } | null
  readonly row: string | null
  readonly find: string
  readonly metric: string | null
  readonly mode: HostMode | null
}

export type View =
  | "host.overview"
  | "host.cpu"
  | "host.memory"
  | "host.storage"
  | "host.network"
  | "host.cgroups"
  | "processes"
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
// The machine is read one resource at a time; the overview says which one is
// tight and the rest answer for themselves.
export type HostSection = "overview" | "cpu" | "memory" | "storage" | "network" | "cgroups"
export const HOST_SECTIONS: readonly HostSection[] = ["overview", "cpu", "memory", "storage", "network", "cgroups"]
export type HostMode = "history" | "topology" | "io" | "filesystems" | "cpu" | "memory" | "tasks"
export type Source = "host" | "processes" | "postgresql" | "events"
export type PgLens = "load" | "per_call" | "io" | "resources" | "stability" | "timing" | "identity"
  | "access" | "changes" | "maintenance" | "size_buffers" | "freeze"
  | "usage" | "low_activity" | "state"
export type PgLevel = "database" | "schema" | "tablespace" | "object"

const VIEWS: readonly View[] = [
  "host.overview", "host.cpu", "host.memory", "host.storage", "host.network", "host.cgroups",
  "processes",
  "pg.overview", "pg.activity", "pg.statements", "pg.plans", "pg.locks", "pg.databases", "pg.tables", "pg.indexes",
  "events",
]

const LENSES: readonly Lens[] = ["generic", "cpu", "memory", "disk"]
const PG_LENSES: readonly PgLens[] = ["load", "per_call", "io", "resources", "stability", "timing", "identity", "access", "changes", "maintenance", "size_buffers", "freeze", "usage", "low_activity", "state"]
const PG_LEVELS: readonly PgLevel[] = ["database", "schema", "tablespace", "object"]

export const DEFAULT_ADDRESS: Address = {
  at: null,
  view: "processes",
  lens: "cpu",
  pgLens: "load",
  pgLevel: "object",
  datid: null,
  schema: null,
  relid: null,
  indexrelid: null,
  tablespaceOid: null,
  sort: null,
  row: null,
  find: "",
  metric: null,
  mode: null,
}

export function readAddress(search: string): Address {
  const parameters = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search)
  const at = Number.parseInt(parameters.get("at") ?? "", 10)
  const view = parameters.get("view")
  const lens = parameters.get("lens")
  const pgLens = parameters.get("pg_lens")
  const pgLevel = PG_LEVELS.find((known) => known === parameters.get("level")) ?? DEFAULT_ADDRESS.pgLevel
  const resolvedView = VIEWS.find((known) => known === view) ?? DEFAULT_ADDRESS.view
  const relation = resolvedView === "pg.tables" || resolvedView === "pg.indexes"
  const postgresEntity = isPostgresEntityView(resolvedView)
  const host = resolvedView.startsWith("host.")
  const hostSection = hostSectionOf(resolvedView)
  const datid = relation && (pgLevel === "schema" || pgLevel === "object") ? oid(parameters.get("datid")) : null
  const sort = parameters.get("sort") ?? ""
  const column = sort.startsWith("-") ? sort.slice(1) : sort
  return {
    at: Number.isSafeInteger(at) && at > 0 ? at : null,
    view: resolvedView,
    lens: LENSES.find((known) => known === lens) ?? DEFAULT_ADDRESS.lens,
    pgLens: PG_LENSES.find((known) => known === pgLens) ?? DEFAULT_ADDRESS.pgLens,
    pgLevel: relation ? pgLevel : DEFAULT_ADDRESS.pgLevel,
    datid,
    schema: relation && pgLevel === "object" && datid !== null ? parameters.get("schema") : null,
    relid: relation && pgLevel === "object" && datid !== null ? oid(parameters.get("relid")) : null,
    indexrelid: resolvedView === "pg.indexes" && pgLevel === "object" && datid !== null ? oid(parameters.get("indexrelid")) : null,
    tablespaceOid: relation && pgLevel === "object" ? oid(parameters.get("tablespace_oid")) : null,
    sort: column === "" ? null : { column, descending: sort.startsWith("-") },
    row: postgresEntity
      ? postgresEntityRow(parameters.get("row"))
      : resolvedView === "processes" || relation || host || resolvedView === "events" ? parameters.get("row") : null,
    find: parameters.get("find") ?? "",
    metric: host && /^[a-z0-9_.-]+$/.test(parameters.get("metric") ?? "") ? parameters.get("metric") : null,
    mode: hostModeOf(hostSection, parameters.get("mode")),
  }
}

export function writeAddress(address: Address): string {
  const parameters = new URLSearchParams()
  const relation = address.view === "pg.tables" || address.view === "pg.indexes"
  if (address.at !== null) parameters.set("at", String(address.at))
  if (address.view !== DEFAULT_ADDRESS.view) parameters.set("view", address.view)
  if (address.lens !== DEFAULT_ADDRESS.lens && address.view === "processes") parameters.set("lens", address.lens)
  if (address.pgLens !== DEFAULT_ADDRESS.pgLens && (relation || address.view === "pg.statements" || address.view === "pg.plans")) parameters.set("pg_lens", address.pgLens)
  if (relation && address.pgLevel !== DEFAULT_ADDRESS.pgLevel) parameters.set("level", address.pgLevel)
  if (relation && address.pgLevel !== "database" && address.datid !== null) parameters.set("datid", address.datid)
  if (relation && address.pgLevel === "object" && address.schema !== null) parameters.set("schema", address.schema)
  if (relation && address.pgLevel === "object" && address.relid !== null) parameters.set("relid", address.relid)
  if (address.view === "pg.indexes" && address.pgLevel === "object" && address.indexrelid !== null) parameters.set("indexrelid", address.indexrelid)
  if (relation && address.pgLevel === "object" && address.tablespaceOid !== null) parameters.set("tablespace_oid", address.tablespaceOid)
  if (address.sort !== null) parameters.set("sort", `${address.sort.descending ? "-" : ""}${address.sort.column}`)
  if ((address.view === "processes" || relation || address.view.startsWith("host.") || address.view === "events"
      || (isPostgresEntityView(address.view) && postgresEntityRow(address.row) !== null))
    && address.row !== null && address.row !== "") parameters.set("row", address.row)
  if (address.find !== "") parameters.set("find", address.find)
  if (address.view.startsWith("host.") && address.metric !== null) parameters.set("metric", address.metric)
  if (address.view.startsWith("host.") && address.mode !== null && address.mode !== defaultHostMode(hostSectionOf(address.view))) parameters.set("mode", address.mode)
  const query = parameters.toString()
  return query === "" ? "/" : `/?${query}`
}

function oid(stored: string | null): string | null {
  return stored !== null && /^[1-9]\d*$/.test(stored) && Number(stored) <= 4_294_967_295 ? stored : null
}

function isPostgresEntityView(view: View): boolean {
  return view === "pg.activity" || view === "pg.statements" || view === "pg.plans"
    || view === "pg.locks" || view === "pg.databases"
}

function postgresEntityRow(stored: string | null): string | null {
  return stored !== null && /^[^:]+:\d+:[^:]+$/.test(stored) ? stored : null
}

export function viewOf(source: string, hostSection: string, pgSection: string): View {
  if (source === "events") return "events"
  if (source === "processes") return "processes"
  if (source === "postgresql") return `pg.${pgSection}` as View
  return `host.${hostSection}` as View
}

export function sourceOf(view: View): Source {
  if (view === "events" || view === "processes") return view
  return view.startsWith("pg.") ? "postgresql" : "host"
}

export function hostSectionOf(view: View): HostSection {
  const section = view.startsWith("host.") ? view.slice(5) : "overview"
  return HOST_SECTIONS.includes(section as HostSection) ? section as HostSection : "overview"
}

export function defaultHostMode(section: HostSection): HostMode | null {
  if (section === "cpu") return "history"
  if (section === "storage") return "io"
  if (section === "cgroups") return "cpu"
  return null
}

export function hostModeOf(section: HostSection, stored: string | null): HostMode | null {
  const allowed: Readonly<Record<HostSection, readonly HostMode[]>> = {
    overview: [],
    cpu: ["history", "topology"],
    memory: [],
    storage: ["io", "filesystems", "topology"],
    network: [],
    cgroups: ["cpu", "memory", "io", "tasks"],
  }
  return allowed[section].find((mode) => mode === stored) ?? defaultHostMode(section)
}

export function pgSectionOf(view: View): "overview" | "activity" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes" {
  const section = view.startsWith("pg.") ? view.slice(3) : "overview"
  return section as "overview" | "activity" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes"
}

export function stepOf(address: string): string {
  return address.replace(/(\?|&)at=\d+&?/, "$1").replace(/[?&]$/, "")
}
