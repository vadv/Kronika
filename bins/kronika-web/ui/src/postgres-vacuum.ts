import type { Cell, DataRow } from "./api"

export type VacuumRisk = "ordinary" | "heavy" | "dangerous"

export interface VacuumNoMovement {
  readonly field: string
  readonly samples: number
  readonly spanUs: number
}

export interface VacuumProcessLoad {
  readonly beforeAt: number
  readonly afterAt: number
  readonly cpuMs: number | null
  readonly cpuShare: number | null
  readonly blockWaitMs: number | null
  readonly runDelayNs: number | null
  readonly readBytes: number | null
  readonly writeBytes: number | null
  readonly majorFaults: number | null
}

export interface VacuumSample {
  readonly row: DataRow
  readonly phase: string
  readonly risk: VacuumRisk
  readonly cadenceSeconds: number | null
}

export interface VacuumRelation {
  readonly database: string | null
  readonly schema: string | null
  readonly name: string | null
  readonly relid: string
  readonly isAutovacuum: boolean
}

export interface VacuumEpisode {
  readonly rows: readonly DataRow[]
  readonly samples: readonly VacuumSample[]
  readonly last: DataRow
  readonly firstAt: number
  readonly lastAt: number
  readonly spanUs: number
  readonly atSample: boolean
  readonly phase: {
    readonly name: string
    readonly risk: VacuumRisk
    readonly firstAt: number
    readonly lastAt: number
    readonly spanUs: number
    readonly sampleCount: number
    readonly indexVacuumCount: number | null
    readonly noMovement: VacuumNoMovement | null
  }
  readonly progress: readonly number[]
  readonly indexProgress: {
    readonly applicable: boolean
    readonly processed: number | null
    readonly total: number | null
  } | null
  readonly delayDeltaMs: number | null
  readonly processLoad: VacuumProcessLoad | null
  readonly processCurrent: DataRow | null
  readonly relation: VacuumRelation
}

export interface VacuumProduct {
  readonly episodes: readonly VacuumEpisode[]
  readonly atTimestamp: number | null
  readonly cadenceSeconds: number | null
  readonly availableFields: ReadonlySet<string>
}

export function parseVacuumProduct(records: readonly Record<string, unknown>[]): VacuumProduct {
  const products = records.filter((record) => record.record === "vacuum")
  if (products.length !== 1) throw new Error("Vacuum response must contain exactly one product record")
  const product = products[0] as Record<string, unknown>
  const rawEpisodes = array(product.episodes, "Vacuum episodes")
  const episodes = rawEpisodes.map((raw, index) => parseEpisode(object(raw, `Vacuum episode ${index}`)))
  const availableFields = new Set<string>()
  for (const episode of episodes) {
    for (const row of episode.rows) {
      for (const field of Object.keys(row.values)) availableFields.add(field)
    }
  }
  for (const field of optionalArray(product.available_fields, "Vacuum available fields")) {
    availableFields.add(text(field, "Vacuum available field"))
  }
  const anchor = object(product.anchor, "Vacuum anchor")
  const atTimestamp = optionalInteger(anchor.selected_at_us, "Vacuum selected timestamp")
  const cadenceSeconds = optionalNumber(anchor.cadence_seconds, "Vacuum selected cadence")
  return { episodes, atTimestamp, cadenceSeconds, availableFields }
}

function parseEpisode(raw: Record<string, unknown>): VacuumEpisode {
  const latest = parseRow(object(raw.latest_row, "Vacuum latest row"))
  const rawSamples = array(raw.samples, "Vacuum samples")
  const samples = rawSamples.map((value, index) => parseSample(object(value, `Vacuum sample ${index}`)))
  if (samples.length === 0) throw new Error("Vacuum episode has no samples")
  const phase = object(raw.phase, "Vacuum phase")
  const noMovement = phase.no_movement === null || phase.no_movement === undefined
    ? null
    : parseNoMovement(object(phase.no_movement, "Vacuum no-movement reading"))
  const progress = object(raw.progress, "Vacuum progress")
  const heapScan = optionalArray(progress.heap_scan, "Vacuum heap scan")
    .map((value) => number(object(value, "Vacuum progress point").percent, "Vacuum progress percent"))
  const indexProgress = progress.index === undefined || progress.index === null
    ? null
    : object(progress.index, "Vacuum index progress")
  const process = raw.process === undefined || raw.process === null
    ? null
    : object(raw.process, "Vacuum process")
  const relation = object(raw.relation, "Vacuum relation")
  return {
    rows: samples.map((sample) => sample.row),
    samples,
    last: latest,
    firstAt: integer(raw.first_at_us, "Vacuum first timestamp"),
    lastAt: integer(raw.last_at_us, "Vacuum last timestamp"),
    spanUs: integer(raw.span_us, "Vacuum episode span"),
    atSample: object(raw.observation, "Vacuum observation").at_sample === true,
    phase: {
      name: text(phase.name, "Vacuum phase name"),
      risk: risk(phase.risk),
      firstAt: integer(phase.first_at_us, "Vacuum phase first timestamp"),
      lastAt: integer(phase.last_at_us, "Vacuum phase last timestamp"),
      spanUs: integer(phase.span_us, "Vacuum phase span"),
      sampleCount: integer(phase.sample_count, "Vacuum phase samples"),
      indexVacuumCount: optionalNumber(phase.index_vacuum_count, "Vacuum index cycle"),
      noMovement,
    },
    progress: heapScan,
    indexProgress: indexProgress === null ? null : {
      applicable: indexProgress.applicable === true,
      processed: optionalNumber(indexProgress.processed, "Vacuum indexes processed"),
      total: optionalNumber(indexProgress.total, "Vacuum indexes total"),
    },
    delayDeltaMs: optionalNumber(raw.delay_delta_ms, "Vacuum delay delta"),
    processLoad: process === null ? null : parseProcessLoad(process),
    processCurrent: process?.current_row === undefined || process.current_row === null
      ? null
      : parseProcessRow(object(process.current_row, "Vacuum current process row")),
    relation: {
      database: optionalText(relation.database, "Vacuum relation database"),
      schema: optionalText(relation.schema, "Vacuum relation schema"),
      name: optionalText(relation.name, "Vacuum relation name"),
      relid: text(relation.relid, "Vacuum relation id"),
      isAutovacuum: relation.is_autovacuum === true,
    },
  }
}

function parseSample(raw: Record<string, unknown>): VacuumSample {
  const storedRow = raw.row === undefined ? raw : object(raw.row, "Vacuum sample row")
  const row = parseRow(storedRow)
  return {
    row,
    phase: text(raw.phase ?? row.values.phase, "Vacuum sample phase"),
    risk: risk(raw.risk),
    cadenceSeconds: optionalNumber(raw.cadence_seconds, "Vacuum sample cadence"),
  }
}

function parseRow(raw: Record<string, unknown>): DataRow {
  const rawValues = object(raw.values, "Vacuum row values")
  return {
    segmentId: text(raw.segment_id, "Vacuum row segment"),
    logicalName: "pg_stat_progress_vacuum",
    typeId: text(raw.type_id, "Vacuum row layout"),
    ordinal: text(raw.ordinal ?? raw.row_ordinal, "Vacuum row ordinal"),
    timestamp: integer(raw.timestamp ?? raw.timestamp_us, "Vacuum row timestamp"),
    values: rawValues as Readonly<Record<string, Cell>>,
  }
}

function parseNoMovement(raw: Record<string, unknown>): VacuumNoMovement {
  return {
    field: text(raw.field, "Vacuum no-movement field"),
    samples: integer(raw.samples, "Vacuum no-movement samples"),
    spanUs: integer(raw.span_us, "Vacuum no-movement span"),
  }
}

function parseProcessLoad(raw: Record<string, unknown>): VacuumProcessLoad | null {
  if (raw.load === null) return null
  const load = raw.load === undefined ? raw : object(raw.load, "Vacuum process load")
  const beforeAt = optionalInteger(load.before_at_us ?? load.before_timestamp_us, "Vacuum process first timestamp")
  const afterAt = optionalInteger(load.after_at_us ?? load.after_timestamp_us, "Vacuum process last timestamp")
  if (beforeAt === null || afterAt === null) return null
  return {
    beforeAt,
    afterAt,
    cpuMs: optionalNumber(load.cpu_ms, "Vacuum process CPU"),
    cpuShare: optionalNumber(load.cpu_share_percent, "Vacuum process CPU share"),
    blockWaitMs: optionalNumber(load.block_wait_ms, "Vacuum process block wait"),
    runDelayNs: optionalNumber(load.run_delay_ns, "Vacuum process run delay"),
    readBytes: optionalNumber(load.read_bytes, "Vacuum process reads"),
    writeBytes: optionalNumber(load.write_bytes, "Vacuum process writes"),
    majorFaults: optionalNumber(load.major_faults, "Vacuum process major faults"),
  }
}

function parseProcessRow(raw: Record<string, unknown>): DataRow {
  const values = object(raw.values, "Vacuum process row values")
  return {
    segmentId: text(raw.segment_id, "Vacuum process row segment"),
    logicalName: "os_process",
    typeId: text(raw.type_id, "Vacuum process row layout"),
    ordinal: text(raw.ordinal ?? raw.row_ordinal, "Vacuum process row ordinal"),
    timestamp: integer(raw.timestamp ?? raw.timestamp_us, "Vacuum process row timestamp"),
    values: values as Readonly<Record<string, Cell>>,
  }
}

function risk(value: unknown): VacuumRisk {
  if (value === "ordinary" || value === "heavy" || value === "dangerous") return value
  throw new Error("Vacuum response has an invalid phase risk")
}

function object(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error(`${name} is not an object`)
  return value as Record<string, unknown>
}

function array(value: unknown, name: string): readonly unknown[] {
  if (!Array.isArray(value)) throw new Error(`${name} is not an array`)
  return value
}

function optionalArray(value: unknown, name: string): readonly unknown[] {
  return value === undefined || value === null ? [] : array(value, name)
}

function text(value: unknown, name: string): string {
  if (typeof value === "string") return value
  if (typeof value === "number" && Number.isInteger(value)) return String(value)
  throw new Error(`${name} is not textual`)
}

function optionalText(value: unknown, name: string): string | null {
  return value === undefined || value === null ? null : text(value, name)
}

function integer(value: unknown, name: string): number {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN
  if (!Number.isSafeInteger(parsed)) throw new Error(`${name} is not a safe integer`)
  return parsed
}

function optionalInteger(value: unknown, name: string): number | null {
  return value === undefined || value === null ? null : integer(value, name)
}

function number(value: unknown, name: string): number {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN
  if (!Number.isFinite(parsed)) throw new Error(`${name} is not finite`)
  return parsed
}

function optionalNumber(value: unknown, name: string): number | null {
  return value === undefined || value === null ? null : number(value, name)
}
