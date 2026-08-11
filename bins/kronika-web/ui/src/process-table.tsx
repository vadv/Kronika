import {
  type ColumnDef,
  type ColumnSizingState,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type SortingState,
  useReactTable,
} from "@tanstack/react-table"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import type { Cell, DataRow } from "./api"
import { fittedWidth, widestCell } from "./column-size"
import { LabelHelp, type Translate } from "./help"
import {
  asNumber,
  cores,
  formatUtc,
  formatUtcCell,
  humanBytes,
  identifier,
  measure,
  millisecondsPerSecond,
  processCommand,
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
  readonly kind: "id" | "command" | "state" | "time" | "number" | "rate" | "cores" | "kib" | "bytes" | "ns"
  readonly size: number
  readonly sticky?: "pid" | "start" | "command"
}

const PID: Field = { id: "pid", field: "pid", label: "col.pid.label", help: "col.pid.help", kind: "id", size: 62, sticky: "pid" }
const START: Field = { id: "starttime", field: "starttime", label: "col.starttime.label", help: "col.starttime.help", kind: "time", size: 142, sticky: "start" }
const COMMAND: Field = { id: "command", label: "col.command.label", help: "col.command.help", kind: "command", size: 300, sticky: "command" }
const STATE: Field = { id: "state", field: "state", label: "col.state.label", help: "col.state.help", kind: "state", size: 60 }

export const LENS_FIELDS: Readonly<Record<Lens, readonly Field[]>> = {
  generic: [
    PID, START, COMMAND, STATE,
    idField("ppid", "col.ppid", 70), idField("uid", "col.uid", 70), idField("euid", "col.euid", 70),
    idField("gid", "col.gid", 70), idField("egid", "col.egid", 70),
    numberField("num_threads", "col.threads", 84), idField("tty", "col.tty", 70),
    idField("exit_signal", "col.exit_signal", 70),
  ],
  cpu: [
    PID, START, COMMAND, STATE,
    idField("curcpu", "col.curcpu", 70), coresField("utime", "col.utime", 84),
    coresField("stime", "col.stime", 84), nsField("rundelay_ns", "col.rundelay", 96),
    rateField("blkdelay_ticks", "col.blkdelay", 84), rateField("nvcsw", "col.nvcsw", 84),
    rateField("nivcsw", "col.nivcsw", 84), numberField("nice", "col.nice", 84),
    numberField("prio", "col.prio", 84), numberField("rtprio", "col.rtprio", 84), idField("policy", "col.policy", 70),
  ],
  memory: [
    PID, START, COMMAND, STATE,
    kibField("rmem_kb", "col.rmem", 96), kibField("vmem_kb", "col.vmem", 96),
    kibField("vswap_kb", "col.vswap", 96), rateField("minflt", "col.minflt", 84),
    rateField("majflt", "col.majflt", 84),
  ],
  disk: [
    PID, START, COMMAND, STATE,
    bytesField("read_bytes", "col.read_bytes", 96), bytesField("write_bytes", "col.write_bytes", 96),
    bytesField("cancelled_write_bytes", "col.cancelled_write", 96), rateField("syscr", "col.syscr", 84),
    rateField("syscw", "col.syscw", 84), bytesField("rchar", "col.rchar", 96),
    bytesField("wchar", "col.wchar", 96), rateField("blkdelay_ticks", "col.blkdelay", 84),
  ],
}

export function ProcessSummary({ lens, linkedPids, locale, rows, t, ticksPerSecond }: { readonly lens: Lens; readonly linkedPids: ReadonlySet<number>; readonly locale: Locale; readonly rows: readonly DataRow[]; readonly t: Translate; readonly ticksPerSecond: number | null }) {
  const metrics = summaryMetrics(rows, lens, linkedPids, ticksPerSecond, locale, t)
  return <section aria-label={t("process.summary.title")} className="process-summary">
    {metrics.map(({ key, output }) => <article key={key}><span>{t(key)}</span><strong>{output}</strong></article>)}
  </section>
}

export function ProcessTable({
  lens,
  linkedPids,
  locale,
  onSelect,
  rows,
  selectedKey,
  t,
  ticksPerSecond,
}: {
  readonly lens: Lens
  readonly linkedPids: ReadonlySet<number>
  readonly locale: Locale
  readonly onSelect: (row: DataRow) => void
  readonly rows: readonly DataRow[]
  readonly selectedKey: string | null
  readonly t: Translate
  readonly ticksPerSecond: number | null
}) {
  const [sorting, setSorting] = useState<SortingState>([])
  const [sizing, setSizing] = useState<ColumnSizingState>({})
  useEffect(() => {
    setSorting(lens === "generic" ? [{ id: "pid", desc: false }] : [{ id: defaultSort(lens), desc: true }])
  }, [lens])
  const columns = useMemo<ColumnDef<DataRow>[]>(() => LENS_FIELDS[lens].map((field) => ({
    id: field.id,
    accessorFn: (row) => sortable(row, field),
    size: field.size,
    sortUndefined: "last",
    header: ({ column }) => (
      <div className="column-head">
        <button
          aria-label={column.getIsSorted() === "asc" ? t("common.sort_desc") : t("common.sort_asc")}
          onClick={column.getToggleSortingHandler()}
          type="button"
        >{t(field.label)}<span className="sort-mark">{column.getIsSorted() === "asc" ? "↑" : column.getIsSorted() === "desc" ? "↓" : ""}</span></button>
        <span className="column-help"><LabelHelp helpKey={field.help} iconOnly labelKey={field.label} t={t} /></span>
      </div>
    ),
    cell: ({ row }) => <CellValue field={field} locale={locale} linked={linkedPids.has(asNumber(value(row.original, "pid")) ?? -1)} row={row.original} t={t} ticksPerSecond={ticksPerSecond} />,
    meta: { sticky: field.sticky },
  })), [lens, linkedPids, locale, t, ticksPerSecond])
  const table = useReactTable({
    columns,
    data: rows as DataRow[],
    getCoreRowModel: getCoreRowModel(),
    getRowId: processKey,
    getSortedRowModel: getSortedRowModel(),
    columnResizeMode: "onChange",
    enableColumnResizing: true,
    onColumnSizingChange: setSizing,
    onSortingChange: setSorting,
    state: { columnSizing: sizing, sorting },
  })
  const displayed = table.getRowModel().rows
  const scroll = useRef<HTMLDivElement>(null)
  // A grip is dragged to resize and double clicked to fit: the width of the
  // widest cell on screen, which is what a person means by "fit".
  const fit = useCallback((id: string, index: number) => {
    const root = scroll.current
    if (root === null) return
    setSizing((current) => ({ ...current, [id]: fittedWidth(widestCell(root, index)) }))
  }, [])
  const virtual = useVirtualizer({ count: displayed.length, estimateSize: () => 23, getScrollElement: () => scroll.current, overscan: 14 })
  const width = table.getTotalSize()
  return (
    <div aria-label={t("table.processes")} className="process-table" data-testid="process-table" role="table">
      <div className="process-scroll" ref={scroll}>
        <div className="process-head" role="row" style={{ width }}>
          {table.getHeaderGroups()[0]?.headers.map((header, index) => <div className={stickyClass(header.column.columnDef.meta, true)} key={header.id} role="columnheader" style={{ width: header.getSize() }}>
            {flexRender(header.column.columnDef.header, header.getContext())}
            <span className="column-grip" onDoubleClick={() => fit(header.column.id, index)} onMouseDown={header.getResizeHandler()} onTouchStart={header.getResizeHandler()} />
          </div>)}
        </div>
        {displayed.length === 0
          ? <p className="table-empty">{t("table.empty")}</p>
          : <div className="virtual-body" style={{ height: virtual.getTotalSize(), width }}>
            {virtual.getVirtualItems().map((item) => {
              const row = displayed[item.index]
              if (row === undefined) return null
              const pid = asNumber(value(row.original, "pid"))
              const linked = pid !== null && linkedPids.has(pid)
              const selected = processKey(row.original) === selectedKey
              return (
                <div
                  aria-label={t("table.activate", { pid: identifier(value(row.original, "pid")) })}
                  aria-selected={selected}
                  className="process-row"
                  data-pg-linked={linked || undefined}
                  data-testid={linked ? "pg-linked-row" : undefined}
                  key={row.id}
                  onClick={() => onSelect(row.original)}
                  onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelect(row.original) } }}
                  role="row"
                  style={{ height: item.size, transform: `translateY(${item.start}px)`, width }}
                  tabIndex={0}
                >
                  {row.getVisibleCells().map((cell) => <div className={stickyClass(cell.column.columnDef.meta, false)} key={cell.id} role="cell" style={{ width: cell.column.getSize() }}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</div>)}
                </div>
              )
            })}
          </div>}
      </div>
    </div>
  )
}

export function CellValue({ field, linked, locale, row, t, ticksPerSecond }: { readonly field: Field; readonly linked: boolean; readonly locale: Locale; readonly row: DataRow; readonly t: Translate; readonly ticksPerSecond: number | null }) {
  const cell = field.field === undefined ? null : value(row, field.field)
  let output: string
  switch (field.kind) {
    case "command": output = processCommand(row); break
    case "state": output = stateText(cell); break
    case "time": output = formatUtcCell(asNumber(cell)); break
    case "number": output = measure(cell, locale); break
    case "rate": output = measure(cell, locale, t("unit.per_second")); break
    case "cores": output = cores(cell, locale, ticksPerSecond) + t("unit.cores"); break
    case "kib": output = humanBytes(kib(asNumber(cell)), locale); break
    case "bytes": output = humanBytes(cell, locale, t("unit.per_second")); break
    case "ns": output = millisecondsPerSecond(cell, locale) + t("unit.ms_per_second"); break
    case "id": output = identifier(cell); break
  }
  return <span className={field.kind === "command" ? "command-cell" : "numeric-cell"} title={output}>{field.kind === "command" && linked && <span className="pg-badge">PG</span>}{output}</span>
}

function sortable(row: DataRow, field: Field): string | number | null {
  if (field.kind === "command") return processCommand(row)
  const cell = field.field === undefined ? null : value(row, field.field)
  if (field.kind === "state") return stateText(cell)
  if (field.kind === "id" && field.id !== "pid") return rawText(cell)
  return asNumber(cell) ?? rawText(cell)
}

function defaultSort(lens: Lens): string {
  if (lens === "cpu") return "utime"
  if (lens === "memory") return "rmem_kb"
  if (lens === "disk") return "read_bytes"
  return "pid"
}

/** Every cumulative column arrives as a rate, so a summary reads per second
 *  and in the unit a person thinks in: cores, bytes, milliseconds. */
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

function stickyClass(meta: unknown, head: boolean): string {
  const sticky = (meta as { readonly sticky?: "pid" | "start" | "command" } | undefined)?.sticky
  return [head ? "process-header-cell" : "process-cell", sticky === "pid" ? "sticky-pid" : "", sticky === "start" ? "sticky-start" : "", sticky === "command" ? "sticky-command" : ""].filter(Boolean).join(" ")
}
