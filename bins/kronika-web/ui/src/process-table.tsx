import { useMemo } from "react"

import type { Cell, DataRow, Finding } from "./api"
import { EntityTable, type EntityColumn, type TableOrder } from "./entity-table"
import type { Translate } from "./help"
import {
  asNumber,
  cores,
  humanBytes,
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

export function ProcessSummary({ lens, linkedPids, locale, rows, t, ticksPerSecond }: { readonly lens: Lens; readonly linkedPids: ReadonlySet<number>; readonly locale: Locale; readonly rows: readonly DataRow[]; readonly t: Translate; readonly ticksPerSecond: number | null }) {
  const metrics = summaryMetrics(rows, lens, linkedPids, ticksPerSecond, locale, t)
  return <section aria-label={t("process.summary.title")} className="process-summary">
    {metrics.map(({ key, output }) => <article key={key}><span>{t(key)}</span><strong>{output}</strong></article>)}
  </section>
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
    className="process-table"
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
    case "cores": return cores(cell, locale, ticksPerSecond) + t("unit.cores")
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
  return <span className={field.kind === "command" ? "command-cell" : "numeric-cell"} title={output}>{field.kind === "command" && linked && <span className="pg-badge">PG</span>}{output}</span>
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

function summaryMetrics(
  rows: readonly DataRow[],
  lens: Lens,
  linkedPids: ReadonlySet<number>,
  ticksPerSecond: number | null,
  locale: Locale,
  t: Translate,
): readonly { readonly key: string; readonly output: string }[] {
  const count = (number: number | null) => number === null ? "—" : measure(number, locale)
  const perSecond = (number: number | null) => number === null ? "—" : measure(number, locale) + t("unit.per_second")
  if (lens === "generic") return [
    { key: "process.summary.processes", output: count(rows.length) },
    { key: "process.summary.threads", output: count(sum(rows, "num_threads")) },
    { key: "process.summary.running", output: count(rows.filter((row) => stateText(value(row, "state")) === "R").length) },
    { key: "process.summary.postgresql", output: count(rows.filter((row) => linkedPids.has(asNumber(value(row, "pid")) ?? -1)).length) },
  ]
  if (lens === "cpu") return [
    { key: "process.summary.user_time", output: cores(sum(rows, "utime"), locale, ticksPerSecond) + t("unit.cores") },
    { key: "process.summary.system_time", output: cores(sum(rows, "stime"), locale, ticksPerSecond) + t("unit.cores") },
    { key: "process.summary.run_delay", output: millisecondsPerSecond(sum(rows, "rundelay_ns"), locale) + t("unit.ms_per_second") },
    { key: "process.summary.context_switches", output: perSecond(combine(rows, "nvcsw", "nivcsw")) },
  ]
  if (lens === "memory") return [
    { key: "process.summary.resident", output: humanBytes(kib(sum(rows, "rmem_kb")), locale) },
    { key: "process.summary.virtual", output: humanBytes(kib(sum(rows, "vmem_kb")), locale) },
    { key: "process.summary.swap", output: humanBytes(kib(sum(rows, "vswap_kb")), locale) },
    { key: "process.summary.major_faults", output: perSecond(sum(rows, "majflt")) },
  ]
  return [
    { key: "process.summary.read", output: humanBytes(sum(rows, "read_bytes"), locale, t("unit.per_second")) },
    { key: "process.summary.written", output: humanBytes(sum(rows, "write_bytes"), locale, t("unit.per_second")) },
    { key: "process.summary.read_calls", output: perSecond(sum(rows, "syscr")) },
    { key: "process.summary.write_calls", output: perSecond(sum(rows, "syscw")) },
  ]
}

function kib(number: number | null): number | null {
  return number === null ? null : number * 1024
}

function sum(rows: readonly DataRow[], field: string): number | null {
  const values = rows.flatMap((row) => {
    const number = asNumber(value(row, field))
    return number === null ? [] : [number]
  })
  return values.length === 0 ? null : values.reduce((total, number) => total + number, 0)
}

function combine(rows: readonly DataRow[], left: string, right: string): number | null {
  const leftValue = sum(rows, left)
  const rightValue = sum(rows, right)
  return leftValue === null && rightValue === null ? null : (leftValue ?? 0) + (rightValue ?? 0)
}

function rateField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "rate", size } }

function coresField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "cores", size } }

function idField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "id", size } }
function numberField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "number", size } }
function kibField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "kib", size } }
function bytesField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "bytes", size } }
function nsField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "ns", size } }
