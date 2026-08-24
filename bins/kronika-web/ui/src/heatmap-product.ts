import stored from "../../product-heatmap.json" with { type: "json" }

export type HeatmapSurfaceId = "processes" | "statements" | "plans" | "databases" | "tables" | "indexes" | "cgroups"
export type HeatmapGroupId = "identity" | "command" | "database" | "schema" | "tablespace"
export type HeatmapCutId =
  | "cpu" | "rss" | "io_read" | "io_write" | "majflt" | "run_delay"
  | "exec_time" | "calls" | "rows" | "shared_read" | "shared_dirtied" | "temp_written" | "wal_bytes"
  | "commits" | "rollbacks" | "db_read" | "temp_bytes" | "deadlocks"
  | "writes" | "seq_read" | "heap_read" | "dead_tuples" | "autovacuum_time"
  | "idx_scan" | "idx_tup_read" | "idx_blks_read"
  | "cg_cpu" | "cg_throttled" | "cg_read" | "cg_write" | "cg_rios" | "cg_wios"

export type ActivityValueKind = "milliseconds" | "seconds" | "microseconds" | "nanoseconds" | "count" | "bytes"

export interface ActivityCut {
  readonly id: HeatmapCutId
  readonly kind: ActivityValueKind
  readonly scaleBy?: "block_size" | "clock_ticks" | "kib"
}

export interface HeatmapProductSurface {
  readonly id: HeatmapSurfaceId
  readonly defaultCut: HeatmapCutId
  readonly defaultGroup: HeatmapGroupId
  readonly defaultColumns: number
  readonly cuts: readonly ActivityCut[]
}

type StoredUnit = "blocks" | "bytes" | "clock_ticks" | "count" | "kibibytes" | "microseconds" | "milliseconds" | "nanoseconds" | "seconds"

type StoredConversion =
  | { readonly kind: "identity" }
  | { readonly kind: "fixed_multiply"; readonly factor: number; readonly target_unit: StoredUnit }
  | { readonly kind: "recorded_multiply"; readonly locator: string; readonly target_unit: StoredUnit }
  | { readonly kind: "recorded_divide"; readonly locator: string; readonly target_unit: StoredUnit }

interface StoredCut {
  readonly id: HeatmapCutId
  readonly section: string
  readonly fields: readonly string[]
  readonly labels: readonly string[]
  readonly raw_unit: StoredUnit
  readonly conversion: StoredConversion
}

interface StoredGroup {
  readonly id: HeatmapGroupId
  readonly fields: readonly string[]
}

interface StoredSurface {
  readonly id: HeatmapSurfaceId
  readonly default_cut: HeatmapCutId
  readonly default_group: HeatmapGroupId
  readonly default_columns: number
  readonly groups: readonly StoredGroup[]
  readonly cuts: readonly StoredCut[]
}

interface StoredRegistry {
  readonly version: number
  readonly policy: {
    readonly default_top: number
    readonly max_top: number
    readonly max_columns: number
  }
  readonly surfaces: readonly StoredSurface[]
}

const registry = stored as StoredRegistry

const productSurfaces = new Map<HeatmapSurfaceId, HeatmapProductSurface>(registry.surfaces.map((surface) => [
  surface.id,
  {
    id: surface.id,
    defaultCut: surface.default_cut,
    defaultGroup: surface.default_group,
    defaultColumns: surface.default_columns,
    cuts: surface.cuts.map(activityCut),
  },
]))

export const HEATMAP_DEFAULT_TOP = registry.policy.default_top

export function heatmapSurface(id: HeatmapSurfaceId): HeatmapProductSurface {
  const surface = productSurfaces.get(id)
  if (surface === undefined) throw new Error(`unknown Heatmap surface ${id}`)
  return surface
}

export function heatmapCuts(surface: HeatmapProductSurface, ids: readonly HeatmapCutId[]): readonly ActivityCut[] {
  return ids.map((id) => {
    const cut = surface.cuts.find((candidate) => candidate.id === id)
    if (cut === undefined) throw new Error(`unknown Heatmap cut ${surface.id}/${id}`)
    return cut
  })
}

export interface HeatmapFixtureRecipe {
  readonly section: string
  readonly fields: readonly string[]
  readonly labels: readonly string[]
  readonly groupFields: readonly string[]
  readonly columns: number
}

export function heatmapFixtureRecipe(
  surfaceId: HeatmapSurfaceId,
  cutId: HeatmapCutId,
  groupId: HeatmapGroupId,
): HeatmapFixtureRecipe {
  const surface = registry.surfaces.find((candidate) => candidate.id === surfaceId)
  if (surface === undefined) throw new Error(`unknown Heatmap surface ${surfaceId}`)
  const cut = surface.cuts.find((candidate) => candidate.id === cutId)
  if (cut === undefined) throw new Error(`unknown Heatmap cut ${surfaceId}/${cutId}`)
  const group = surface.groups.find((candidate) => candidate.id === groupId)
  if (group === undefined) throw new Error(`unknown Heatmap group ${surfaceId}/${groupId}`)
  return {
    section: cut.section,
    fields: cut.fields,
    labels: group.fields.length === 0 ? cut.labels : [],
    groupFields: group.fields,
    columns: surface.default_columns,
  }
}

function activityCut(cut: StoredCut): ActivityCut {
  if (cut.conversion.kind === "recorded_multiply"
      && cut.conversion.locator === "pg_settings.block_size"
      && cut.conversion.target_unit === "bytes") {
    return { id: cut.id, kind: "bytes", scaleBy: "block_size" }
  }
  if (cut.conversion.kind === "recorded_divide"
      && cut.conversion.locator === "instance_metadata.clock_ticks_per_sec"
      && cut.conversion.target_unit === "seconds") {
    return { id: cut.id, kind: "seconds", scaleBy: "clock_ticks" }
  }
  if (cut.conversion.kind === "fixed_multiply"
      && cut.conversion.factor === 1_024
      && cut.conversion.target_unit === "bytes") {
    return { id: cut.id, kind: "bytes", scaleBy: "kib" }
  }
  if (cut.conversion.kind !== "identity") {
    throw new Error(`unsupported Heatmap conversion ${cut.id}/${cut.conversion.kind}`)
  }
  return { id: cut.id, kind: displayKind(cut.raw_unit) }
}

function displayKind(unit: StoredUnit): ActivityValueKind {
  if (unit === "bytes") return "bytes"
  if (unit === "milliseconds") return "milliseconds"
  if (unit === "seconds") return "seconds"
  if (unit === "microseconds") return "microseconds"
  if (unit === "nanoseconds") return "nanoseconds"
  if (unit === "count") return "count"
  throw new Error(`Heatmap raw unit ${unit} requires a presentation conversion`)
}
