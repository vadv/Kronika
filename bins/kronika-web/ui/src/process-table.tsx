import { useEffect, useMemo, useState, type Dispatch } from "react"

import { acceptResponse, loadSeries, type Cell, type DataRow, type Finding } from "./api"
import { buildMetricSamples } from "./chart"
import { EntityTable, type EntityColumn, type TableOrder } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import {
  asNumber,
  cores,
  humanBytes,
  humanCores,
  identifier,
  measure,
  millisecondsPerSecond,
  processCommand,
  processDefaultSort,
  processKey,
  rawText,
  stateText,
  value,
  type Lens,
  type Locale,
} from "./model"
import { readingAt, SeriesChart, type ChartPoint } from "./series-chart"

export interface Field {
  readonly id: string
  readonly field?: string
  readonly label: string
  readonly help: string
  readonly kind: "id" | "command" | "state" | "number" | "rate" | "cores" | "kib" | "bytes" | "ns"
  readonly size: number
  readonly sticky?: "pid" | "command"
}

const PID: Field = { id: "pid", field: "pid", label: "col.pid.label", help: "col.pid.help", kind: "id", size: 62, sticky: "pid" }
const COMMAND: Field = { id: "command", label: "col.command.label", help: "col.command.help", kind: "command", size: 300, sticky: "command" }
const STATE: Field = { id: "state", field: "state", label: "col.state.label", help: "col.state.help", kind: "state", size: 60 }

export const LENS_FIELDS: Readonly<Record<Lens, readonly Field[]>> = {
  generic: [
    PID, COMMAND,
    idField("ppid", "col.ppid", 70), idField("uid", "col.uid", 70), idField("euid", "col.euid", 70),
    idField("gid", "col.gid", 70), idField("egid", "col.egid", 70),
    numberField("num_threads", "col.threads", 84), idField("tty", "col.tty", 70),
    idField("exit_signal", "col.exit_signal", 70), STATE,
  ],
  cpu: [
    PID, COMMAND, coresField("utime", "col.utime", 84),
    coresField("stime", "col.stime", 84), nsField("rundelay_ns", "col.rundelay", 96),
    rateField("blkdelay_ticks", "col.blkdelay", 84), rateField("nvcsw", "col.nvcsw", 84),
    rateField("nivcsw", "col.nivcsw", 84), idField("curcpu", "col.curcpu", 70), numberField("nice", "col.nice", 84),
    numberField("prio", "col.prio", 84), numberField("rtprio", "col.rtprio", 84), idField("policy", "col.policy", 70),
    STATE,
  ],
  memory: [
    PID, COMMAND, kibField("rmem_kb", "col.rmem", 96), kibField("vmem_kb", "col.vmem", 96),
    kibField("vswap_kb", "col.vswap", 96), rateField("minflt", "col.minflt", 84),
    rateField("majflt", "col.majflt", 84), STATE,
  ],
  disk: [
    PID, COMMAND, bytesField("read_bytes", "col.read_bytes", 96), bytesField("write_bytes", "col.write_bytes", 96),
    rateField("syscr", "col.syscr", 84),
    rateField("syscw", "col.syscw", 84), bytesField("rchar", "col.rchar", 96),
    bytesField("wchar", "col.wchar", 96), bytesField("cancelled_write_bytes", "col.cancelled_write", 96),
    rateField("blkdelay_ticks", "col.blkdelay", 84), STATE,
  ],
}

type SummaryKind = "B" | "B/s" | "cores" | "count" | "ms/s" | "1/s"

export interface ProcessSummaryMetric {
  readonly field: string
  readonly help: string
  readonly key: string
  readonly kind: SummaryKind
}

export const PROCESS_SUMMARY_METRICS: Readonly<Record<Lens, readonly ProcessSummaryMetric[]>> = {
  generic: [
    summaryMetric("processes", "process.summary.processes", "count"),
    summaryMetric("threads", "process.summary.threads", "count"),
    summaryMetric("runnable", "process.summary.running", "count"),
    summaryMetric("postgresql", "process.summary.postgresql", "count"),
  ],
  cpu: [
    summaryMetric("user_cores", "process.summary.user_time", "cores"),
    summaryMetric("system_cores", "process.summary.system_time", "cores"),
    summaryMetric("run_delay_ms_per_second", "process.summary.run_delay", "ms/s"),
    summaryMetric("context_switches_per_second", "process.summary.context_switches", "1/s"),
  ],
  memory: [
    summaryMetric("resident_kib", "process.summary.resident", "B"),
    summaryMetric("virtual_kib", "process.summary.virtual", "B"),
    summaryMetric("swap_kib", "process.summary.swap", "B"),
    summaryMetric("major_faults_per_second", "process.summary.major_faults", "1/s"),
  ],
  disk: [
    summaryMetric("read_bytes_per_second", "process.summary.read", "B/s"),
    summaryMetric("write_bytes_per_second", "process.summary.written", "B/s"),
    summaryMetric("read_calls_per_second", "process.summary.read_calls", "1/s"),
    summaryMetric("write_calls_per_second", "process.summary.write_calls", "1/s"),
  ],
}

export const PROCESS_SUMMARY_FIELDS: readonly string[] = Object.values(PROCESS_SUMMARY_METRICS).flatMap((metrics) => metrics.map(({ field }) => field))

export interface ProcessSummaryState {
  readonly hour: number | null
  readonly history: readonly DataRow[]
  readonly status: "loading" | "ready" | "empty" | "error"
}

export type ProcessSummaryAction =
  | { readonly hour: number; readonly type: "loading" }
  | { readonly hour: number; readonly type: "error" }
  | { readonly hour: number; readonly type: "loaded"; readonly rows: readonly DataRow[] }

export const EMPTY_PROCESS_SUMMARY: ProcessSummaryState = { hour: null, history: [], status: "loading" }

export function processSummaryReducer(state: ProcessSummaryState, action: ProcessSummaryAction): ProcessSummaryState {
  if (action.type === "loading") {
    return action.hour === state.hour
      ? { ...state, status: "loading" }
      : { hour: action.hour, history: [], status: "loading" }
  }
  if (action.hour !== state.hour) return state
  if (action.type === "error") return { ...state, status: "error" }
  return { hour: action.hour, history: action.rows, status: action.rows.length === 0 ? "empty" : "ready" }
}

export function ProcessSummary({ cursor, dispatch, hour, lens, locale, state, t }: {
  readonly cursor: number
  readonly dispatch: Dispatch<ProcessSummaryAction>
  readonly hour: number
  readonly lens: Lens
  readonly locale: Locale
  readonly state: ProcessSummaryState
  readonly t: Translate
}) {
  const metrics = PROCESS_SUMMARY_METRICS[lens]
  const history = state.hour === hour ? state.history : []
  const status = state.hour === hour ? state.status : "loading"
  useEffect(() => {
    const controller = new AbortController()
    dispatch({ hour, type: "loading" })
    acceptResponse(loadSeries(hour, "os_process_summary", {}, PROCESS_SUMMARY_FIELDS, controller.signal), controller.signal,
      (rows) => dispatch({ hour, type: "loaded", rows }), () => dispatch({ hour, type: "error" }))
    return () => controller.abort()
  }, [hour])
  const statusKey = status === "loading" ? "process.summary.loading" : status === "error" ? "process.summary.error" : status === "empty" ? "status.no_data" : null
  return <section aria-label={t("process.summary.title")} className="process-summary grid grid-cols-4 border-l border-line2 max-[760px]:grid-cols-2">
    {metrics.map((metric) => {
      const output = processSummaryOutput(readingAt(processSummaryPoints(history, metric), cursor), metric, locale, t)
      return <div className="relative flex min-h-[42px] min-w-0 items-center gap-2 border-b border-r border-line px-2 pr-7" key={metric.field}>
        <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-xs uppercase text-fg3">{t(metric.key)}</span>
        <strong className="flex-none text-md font-[560] tabular-nums text-fg">{output}</strong>
        <LabelHelp helpKey={metric.help} iconOnly labelKey={metric.key} t={t} testId={`process-summary-help-${metric.field}`} />
      </div>
    })}
    {statusKey !== null && <p aria-live="polite" className="col-[1/-1] m-0 min-h-[26px] px-2 py-[7px] text-xs text-fg3" data-testid="process-summary-status">{t(statusKey)}</p>}
  </section>
}

export function processSummaryPoints(rows: readonly DataRow[], metric: ProcessSummaryMetric): readonly ChartPoint[] {
  return buildMetricSamples(rows, (row) => {
    if (!Object.hasOwn(row.values, metric.field)) return undefined
    const stored = asNumber(value(row, metric.field))
    return stored === null ? null : metric.kind === "B" ? stored * 1024 : stored
  })
}

export function processSummaryOutput(reading: number | null, metric: ProcessSummaryMetric, locale: Locale, t: Translate): string {
  if (reading === null) return "—"
  if (metric.kind === "B") return humanBytes(reading, locale)
  if (metric.kind === "B/s") return humanBytes(reading, locale, t("unit.per_second"))
  if (metric.kind === "cores") return humanCores(reading, locale)
  if (metric.kind === "ms/s") return measure(reading, locale, t("unit.ms_per_second"))
  return measure(reading, locale, metric.kind === "1/s" ? t("unit.per_second") : "")
}

export function processSummaryFormat(metric: ProcessSummaryMetric, t: Translate): (reading: number, locale: Locale) => string {
  return (reading, locale) => processSummaryOutput(reading, metric, locale, t)
}

export function processSummaryUnit(metric: ProcessSummaryMetric, locale: Locale, t: Translate): string {
  if (metric.kind === "B" || metric.kind === "B/s") return metric.kind
  if (metric.kind === "cores") return t("unit.cores").trim()
  if (metric.kind === "ms/s") return t("unit.ms_per_second").trim()
  if (metric.kind === "1/s") return `1${t("unit.per_second")}`
  return locale === "ru" ? "количество" : "count"
}

function summaryMetric(field: string, key: string, kind: SummaryKind): ProcessSummaryMetric {
  return { field, help: `${key}.help`, key, kind }
}

export function ProcessTable({
  contextLabel,
  finding,
  findingField,
  lens,
  linkedPids,
  locale,
  onOrder,
  onContextClear,
  onPattern,
  onSelect,
  order,
  pattern,
  rows,
  selectedKey,
  t,
  ticksPerSecond,
}: {
  readonly contextLabel?: string | undefined
  readonly finding?: Finding | null
  readonly findingField?: string | null | undefined
  readonly lens: Lens
  readonly linkedPids: ReadonlySet<number>
  readonly locale: Locale
  readonly onOrder: (order: TableOrder | null) => void
  readonly onContextClear?: (() => void) | undefined
  readonly onPattern: (pattern: string) => void
  readonly order: TableOrder | null
  readonly onSelect: (row: DataRow) => void
  readonly pattern: string
  readonly rows: readonly DataRow[]
  readonly selectedKey: string | null
  readonly t: Translate
  readonly ticksPerSecond: number | null
}) {
  const columns = useMemo<readonly EntityColumn[]>(() => LENS_FIELDS[lens].map((field) => {
    const help = processHeaderHelp(field)
    return {
      field: field.id,
      ...(field.kind === "command" ? { filterValue: processCommand } : {}),
      ...(help === undefined ? {} : { help }),
      kind: entityKind(field.kind),
      label: field.label,
      render: (row) => <CellValue field={field} locale={locale} linked={linkedPids.has(asNumber(value(row, "pid")) ?? -1)} row={row} t={t} ticksPerSecond={ticksPerSecond} />,
      sortValue: (row) => sortable(row, field),
      ...(field.sticky === undefined ? {} : { sticky: `sticky-${field.sticky}` }),
      width: field.size,
    }
  }), [lens, linkedPids, locale, t, ticksPerSecond])
  const defaultOrder = lens === "generic"
    ? { column: "pid", descending: false }
    : { column: processDefaultSort(lens, rows), descending: true }
  return <EntityTable
    className="process-table min-w-0 overflow-hidden border border-line2 bg-s1 charts-hidden:flex charts-hidden:flex-col"
    columns={columns}
    contextLabel={contextLabel}
    empty={t("table.empty")}
    finding={finding}
    findingField={findingField}
    label={t("table.processes")}
    locale={locale}
    onOrder={onOrder}
    onContextClear={onContextClear}
    onPattern={onPattern}
    onSelect={onSelect}
    order={order ?? defaultOrder}
    pattern={pattern}
    rowKey={processKey}
    rowLabel={(row) => t("table.activate", { pid: identifier(value(row, "pid")) })}
    rows={rows}
    selectedKey={selectedKey}
    t={t}
    testId="process-table"
  />
}

export function formatCell(kind: Field["kind"], cell: Cell, locale: Locale, t: Translate, ticksPerSecond: number | null): string {
  switch (kind) {
    case "state": return stateText(cell)
    case "number": return measure(cell, locale)
    case "rate": return measure(cell, locale, t("unit.per_second"))
    case "cores": return cores(cell, locale, ticksPerSecond, "")
    case "kib": return humanBytes(kib(asNumber(cell)), locale)
    case "bytes": return humanBytes(cell, locale, t("unit.per_second"))
    case "ns": return millisecondsPerSecond(cell, locale) + t("unit.ms_per_second")
    case "id": return identifier(cell)
    case "command": return ""
  }
}

export function CellValue({ field, linked, locale, row, t, ticksPerSecond }: { readonly field: Field; readonly linked: boolean; readonly locale: Locale; readonly row: DataRow; readonly t: Translate; readonly ticksPerSecond: number | null }) {
  const cell = field.field === undefined ? null : value(row, field.field)
  const output = field.kind === "command" ? processCommand(row) : formatCell(field.kind, cell, locale, t, ticksPerSecond)
  return <span className={`block overflow-hidden text-ellipsis whitespace-nowrap ${field.kind === "command" ? "w-full text-fg" : "numeric-cell tabular-nums"}`} title={output}>{field.kind === "command" && linked && <span className="mr-1.5 inline-block border border-accent-line bg-accent-soft px-1 py-0.5 align-[1px] text-xs font-bold tracking-[.06em] text-accent2">PG</span>}{output}</span>
}

function sortable(row: DataRow, field: Field): string | number | null {
  if (field.kind === "command") return processCommand(row)
  const cell = field.field === undefined ? null : value(row, field.field)
  if (field.kind === "state") return stateText(cell)
  if (field.kind === "id" && field.id !== "pid") return rawText(cell)
  return asNumber(cell) ?? rawText(cell)
}

function entityKind(kind: Field["kind"]): NonNullable<EntityColumn["kind"]> {
  if (kind === "id") return "id"
  if (kind === "command" || kind === "state") return "text"
  return "number"
}

function processHeaderHelp(field: Field): string | undefined {
  return field.id === "pid" || field.id === "command" || field.id === "num_threads" ? undefined : field.help
}

function kib(number: number | null): number | null {
  return number === null ? null : number * 1024
}

function rateField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "rate", size } }

function coresField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "cores", size } }

function idField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "id", size } }
function numberField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "number", size } }
function kibField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "kib", size } }
function bytesField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "bytes", size } }
function nsField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "ns", size } }
