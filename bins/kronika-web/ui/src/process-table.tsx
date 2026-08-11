import {
  type ColumnDef,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type SortingState,
  useReactTable,
} from "@tanstack/react-table"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useEffect, useMemo, useRef, useState } from "react"

import type { Cell, DataRow } from "./api"
import { LabelHelp, type Translate } from "./help"
import {
  asNumber,
  formatUtc,
  formatUtcCell,
  identifier,
  measure,
  processCommand,
  processKey,
  rawText,
  stateText,
  value,
  type Lens,
  type Locale,
} from "./model"

interface Field {
  readonly id: string
  readonly field?: string
  readonly label: string
  readonly help: string
  readonly kind: "id" | "command" | "state" | "time" | "number" | "kib" | "bytes" | "ns"
  readonly size: number
  readonly sticky?: "pid" | "start" | "command"
}

const PID: Field = { id: "pid", field: "pid", label: "col.pid.label", help: "col.pid.help", kind: "id", size: 62, sticky: "pid" }
const START: Field = { id: "starttime", field: "starttime", label: "col.starttime.label", help: "col.starttime.help", kind: "time", size: 142, sticky: "start" }
const COMMAND: Field = { id: "command", label: "col.command.label", help: "col.command.help", kind: "command", size: 300, sticky: "command" }
const STATE: Field = { id: "state", field: "state", label: "col.state.label", help: "col.state.help", kind: "state", size: 60 }

const LENS_FIELDS: Readonly<Record<Lens, readonly Field[]>> = {
  generic: [
    PID, START, COMMAND, STATE,
    idField("ppid", "col.ppid", 70), idField("uid", "col.uid", 70), idField("euid", "col.euid", 70),
    idField("gid", "col.gid", 70), idField("egid", "col.egid", 70),
    numberField("num_threads", "col.threads", 84), idField("tty", "col.tty", 70),
    idField("exit_signal", "col.exit_signal", 70),
  ],
  cpu: [
    PID, START, COMMAND, STATE,
    idField("curcpu", "col.curcpu", 70), numberField("utime", "col.utime", 84),
    numberField("stime", "col.stime", 84), nsField("rundelay_ns", "col.rundelay", 84),
    numberField("blkdelay_ticks", "col.blkdelay", 84), numberField("nvcsw", "col.nvcsw", 84),
    numberField("nivcsw", "col.nivcsw", 84), numberField("nice", "col.nice", 84),
    numberField("prio", "col.prio", 84), numberField("rtprio", "col.rtprio", 84), idField("policy", "col.policy", 70),
  ],
  memory: [
    PID, START, COMMAND, STATE,
    kibField("rmem_kb", "col.rmem", 96), kibField("vmem_kb", "col.vmem", 96),
    kibField("vswap_kb", "col.vswap", 96), numberField("minflt", "col.minflt", 84),
    numberField("majflt", "col.majflt", 84),
  ],
  disk: [
    PID, START, COMMAND, STATE,
    bytesField("read_bytes", "col.read_bytes", 96), bytesField("write_bytes", "col.write_bytes", 96),
    bytesField("cancelled_write_bytes", "col.cancelled_write", 96), numberField("syscr", "col.syscr", 84),
    numberField("syscw", "col.syscw", 84), numberField("rchar", "col.rchar", 84),
    numberField("wchar", "col.wchar", 84), numberField("blkdelay_ticks", "col.blkdelay", 84),
  ],
}

export function ProcessSummary({ lens, linkedPids, locale, rows, t }: { readonly lens: Lens; readonly linkedPids: ReadonlySet<number>; readonly locale: Locale; readonly rows: readonly DataRow[]; readonly t: Translate }) {
  const metrics = summaryMetrics(rows, lens, linkedPids)
  return <section aria-label={t("process.summary.title")} className="process-summary">
    {metrics.map(({ key, output, unit }) => <article key={key}><span>{t(key)}</span><strong>{output === null ? "—" : measure(output, locale, unit)}</strong></article>)}
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
}: {
  readonly lens: Lens
  readonly linkedPids: ReadonlySet<number>
  readonly locale: Locale
  readonly onSelect: (row: DataRow) => void
  readonly rows: readonly DataRow[]
  readonly selectedKey: string | null
  readonly t: Translate
}) {
  const [sorting, setSorting] = useState<SortingState>([])
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
    cell: ({ row }) => <CellValue field={field} locale={locale} linked={linkedPids.has(asNumber(value(row.original, "pid")) ?? -1)} row={row.original} />,
    meta: { sticky: field.sticky },
  })), [lens, linkedPids, locale, t])
  const table = useReactTable({
    columns,
    data: rows as DataRow[],
    getCoreRowModel: getCoreRowModel(),
    getRowId: processKey,
    getSortedRowModel: getSortedRowModel(),
    onSortingChange: setSorting,
    state: { sorting },
  })
  const displayed = table.getRowModel().rows
  const scroll = useRef<HTMLDivElement>(null)
  const virtual = useVirtualizer({ count: displayed.length, estimateSize: () => 23, getScrollElement: () => scroll.current, overscan: 14 })
  const width = table.getTotalSize()
  return (
    <div aria-label={t("table.processes")} className="process-table" data-testid="process-table" role="table">
      <div className="process-scroll" ref={scroll}>
        <div className="process-head" role="row" style={{ width }}>
          {table.getHeaderGroups()[0]?.headers.map((header) => <div className={stickyClass(header.column.columnDef.meta, true)} key={header.id} role="columnheader" style={{ width: header.getSize() }}>{flexRender(header.column.columnDef.header, header.getContext())}</div>)}
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

function CellValue({ field, linked, locale, row }: { readonly field: Field; readonly linked: boolean; readonly locale: Locale; readonly row: DataRow }) {
  const cell = field.field === undefined ? null : value(row, field.field)
  let output: string
  switch (field.kind) {
    case "command": output = processCommand(row); break
    case "state": output = stateText(cell); break
    case "time": output = formatUtcCell(asNumber(cell)); break
    case "number": output = measure(cell, locale); break
    case "kib": output = measure(cell, locale, " KiB"); break
    case "bytes": output = measure(cell, locale, " B"); break
    case "ns": output = measure(cell, locale, " ns"); break
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

function summaryMetrics(rows: readonly DataRow[], lens: Lens, linkedPids: ReadonlySet<number>): readonly { readonly key: string; readonly output: number | null; readonly unit: string }[] {
  if (lens === "generic") return [
    { key: "process.summary.processes", output: rows.length, unit: "" },
    { key: "process.summary.threads", output: sum(rows, "num_threads"), unit: "" },
    { key: "process.summary.running", output: rows.filter((row) => stateText(value(row, "state")) === "R").length, unit: "" },
    { key: "process.summary.postgresql", output: rows.filter((row) => linkedPids.has(asNumber(value(row, "pid")) ?? -1)).length, unit: "" },
  ]
  if (lens === "cpu") return [
    { key: "process.summary.user_time", output: sum(rows, "utime"), unit: " ticks" },
    { key: "process.summary.system_time", output: sum(rows, "stime"), unit: " ticks" },
    { key: "process.summary.run_delay", output: sum(rows, "rundelay_ns"), unit: " ns" },
    { key: "process.summary.context_switches", output: combine(rows, "nvcsw", "nivcsw"), unit: "" },
  ]
  if (lens === "memory") return [
    { key: "process.summary.resident", output: sum(rows, "rmem_kb"), unit: " KiB" },
    { key: "process.summary.virtual", output: sum(rows, "vmem_kb"), unit: " KiB" },
    { key: "process.summary.swap", output: sum(rows, "vswap_kb"), unit: " KiB" },
    { key: "process.summary.major_faults", output: sum(rows, "majflt"), unit: "" },
  ]
  return [
    { key: "process.summary.read", output: sum(rows, "read_bytes"), unit: " B" },
    { key: "process.summary.written", output: sum(rows, "write_bytes"), unit: " B" },
    { key: "process.summary.read_calls", output: sum(rows, "syscr"), unit: "" },
    { key: "process.summary.write_calls", output: sum(rows, "syscw"), unit: "" },
  ]
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

function idField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "id", size } }
function numberField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "number", size } }
function kibField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "kib", size } }
function bytesField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "bytes", size } }
function nsField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "ns", size } }

function stickyClass(meta: unknown, head: boolean): string {
  const sticky = (meta as { readonly sticky?: "pid" | "start" | "command" } | undefined)?.sticky
  return [head ? "process-header-cell" : "process-cell", sticky === "pid" ? "sticky-pid" : "", sticky === "start" ? "sticky-start" : "", sticky === "command" ? "sticky-command" : ""].filter(Boolean).join(" ")
}
