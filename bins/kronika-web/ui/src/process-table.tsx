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

const PID: Field = { id: "pid", field: "pid", label: "col.pid.label", help: "col.pid.help", kind: "id", size: 72, sticky: "pid" }
const START: Field = { id: "starttime", field: "starttime", label: "col.starttime.label", help: "col.starttime.help", kind: "time", size: 215, sticky: "start" }
const COMMAND: Field = { id: "command", label: "col.command.label", help: "col.command.help", kind: "command", size: 340, sticky: "command" }
const STATE: Field = { id: "state", field: "state", label: "col.state.label", help: "col.state.help", kind: "state", size: 72 }

const LENS_FIELDS: Readonly<Record<Lens, readonly Field[]>> = {
  generic: [
    PID, START, COMMAND, STATE,
    idField("ppid", "col.ppid", 82), idField("uid", "col.uid", 80), idField("euid", "col.euid", 80),
    idField("gid", "col.gid", 80), idField("egid", "col.egid", 80),
    numberField("num_threads", "col.threads", 90), idField("tty", "col.tty", 75),
    idField("scope", "col.scope", 75), idField("exit_signal", "col.exit_signal", 100),
  ],
  cpu: [
    PID, START, COMMAND, STATE,
    idField("curcpu", "col.curcpu", 75), numberField("utime", "col.utime", 125),
    numberField("stime", "col.stime", 135), nsField("rundelay_ns", "col.rundelay", 130),
    numberField("blkdelay_ticks", "col.blkdelay", 130), numberField("nvcsw", "col.nvcsw", 130),
    numberField("nivcsw", "col.nivcsw", 140), numberField("nice", "col.nice", 75),
    numberField("prio", "col.prio", 90), numberField("rtprio", "col.rtprio", 100), idField("policy", "col.policy", 90),
  ],
  memory: [
    PID, START, COMMAND, STATE,
    kibField("rmem_kb", "col.rmem", 125), kibField("vmem_kb", "col.vmem", 125),
    kibField("vswap_kb", "col.vswap", 115), numberField("minflt", "col.minflt", 125),
    numberField("majflt", "col.majflt", 125),
  ],
  disk: [
    PID, START, COMMAND, STATE,
    bytesField("read_bytes", "col.read_bytes", 145), bytesField("write_bytes", "col.write_bytes", 145),
    bytesField("cancelled_write_bytes", "col.cancelled_write", 155), numberField("syscr", "col.syscr", 120),
    numberField("syscw", "col.syscw", 120), numberField("rchar", "col.rchar", 140),
    numberField("wchar", "col.wchar", 140), numberField("blkdelay_ticks", "col.blkdelay", 130),
  ],
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
  const virtual = useVirtualizer({ count: displayed.length, estimateSize: () => 35, getScrollElement: () => scroll.current, overscan: 14 })
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
    case "time": output = formatUtc(asNumber(cell)); break
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

function idField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "id", size } }
function numberField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "number", size } }
function kibField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "kib", size } }
function bytesField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "bytes", size } }
function nsField(field: string, key: string, size: number): Field { return { id: field, field, label: `${key}.label`, help: `${key}.help`, kind: "ns", size } }

function stickyClass(meta: unknown, head: boolean): string {
  const sticky = (meta as { readonly sticky?: "pid" | "start" | "command" } | undefined)?.sticky
  return [head ? "process-header-cell" : "process-cell", sticky === "pid" ? "sticky-pid" : "", sticky === "start" ? "sticky-start" : "", sticky === "command" ? "sticky-command" : ""].filter(Boolean).join(" ")
}
