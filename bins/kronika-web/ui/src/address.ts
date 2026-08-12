/** The screen as it can be written down: everything a person chose, and
 *  nothing that follows from it. The hour follows from `at`, the open dock
 *  follows from `row`, so neither is here. */
export interface Address {
  readonly at: number | null
  readonly view: View
  readonly lens: Lens
  readonly pgLens: PgLens
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
  | "events"

type Lens = "generic" | "cpu" | "memory" | "disk"
export type PgLens = "load" | "per_call" | "io" | "resources" | "stability" | "timing" | "identity"

const VIEWS: readonly View[] = [
  "host.system", "host.processes",
  "pg.overview", "pg.activity", "pg.statements", "pg.plans", "pg.locks", "pg.databases",
  "events",
]

const LENSES: readonly Lens[] = ["generic", "cpu", "memory", "disk"]
const PG_LENSES: readonly PgLens[] = ["load", "per_call", "io", "resources", "stability", "timing", "identity"]

export const DEFAULT_ADDRESS: Address = {
  at: null,
  view: "host.processes",
  lens: "generic",
  pgLens: "load",
  sort: null,
  row: null,
  find: "",
}

/** A link out of a chat arrives with its tail cut off and its keys from a
 *  newer build. An unreadable value falls back; a white screen would be the
 *  worst answer a link can give. */
export function readAddress(search: string): Address {
  const parameters = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search)
  const at = Number.parseInt(parameters.get("at") ?? "", 10)
  const view = parameters.get("view")
  const lens = parameters.get("lens")
  const pgLens = parameters.get("pg_lens")
  const sort = parameters.get("sort") ?? ""
  const column = sort.startsWith("-") ? sort.slice(1) : sort
  const resolvedView = VIEWS.find((known) => known === view) ?? DEFAULT_ADDRESS.view
  return {
    at: Number.isSafeInteger(at) && at > 0 ? at : null,
    view: resolvedView,
    lens: LENSES.find((known) => known === lens) ?? DEFAULT_ADDRESS.lens,
    pgLens: PG_LENSES.find((known) => known === pgLens) ?? DEFAULT_ADDRESS.pgLens,
    sort: column === "" ? null : { column, descending: sort.startsWith("-") },
    row: resolvedView === "host.processes" ? parameters.get("row") : null,
    find: parameters.get("find") ?? "",
  }
}

/** Only what differs from the default is written, so a plain screen keeps a
 *  plain link. */
export function writeAddress(address: Address): string {
  const parameters = new URLSearchParams()
  if (address.at !== null) parameters.set("at", String(address.at))
  if (address.view !== DEFAULT_ADDRESS.view) parameters.set("view", address.view)
  if (address.lens !== DEFAULT_ADDRESS.lens && address.view === "host.processes") parameters.set("lens", address.lens)
  if (address.pgLens !== DEFAULT_ADDRESS.pgLens && (address.view === "pg.statements" || address.view === "pg.plans")) parameters.set("pg_lens", address.pgLens)
  if (address.sort !== null) parameters.set("sort", `${address.sort.descending ? "-" : ""}${address.sort.column}`)
  if (address.view === "host.processes" && address.row !== null && address.row !== "") parameters.set("row", address.row)
  if (address.find !== "") parameters.set("find", address.find)
  const query = parameters.toString()
  return query === "" ? "/" : `/?${query}`
}

/** The screen is one source and one section; three parameters would allow a
 *  combination no screen corresponds to. */
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

export function pgSectionOf(view: View): "overview" | "activity" | "statements" | "plans" | "locks" | "databases" {
  const section = view.startsWith("pg.") ? view.slice(3) : "overview"
  return section as "overview" | "activity" | "statements" | "plans" | "locks" | "databases"
}

/** Everything but the moment: two addresses that differ only in `at` are the
 *  same step of navigation, and one drag should not fill the history. */
export function stepOf(address: string): string {
  return address.replace(/(\?|&)at=\d+&?/, "$1").replace(/[?&]$/, "")
}
