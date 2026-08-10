import { type ColumnDef, flexRender, getCoreRowModel, getSortedRowModel, type SortingState, useReactTable } from "@tanstack/react-table"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useMemo, useRef, useState } from "react"

import { LabelHelp, type Translate } from "./help"
import { formatUtc, type Locale, measure, type SystemSnapshot } from "./model"

interface Field {
  readonly id: keyof SystemSnapshot
  readonly label: string
  readonly help: string
  readonly kind: "time" | "percent" | "number" | "kib"
  readonly size: number
}

const FIELDS: readonly Field[] = [
  field("timestamp", "time", "time", 215),
  field("health", "health", "percent", 105),
  field("load1", "load1", "number", 100),
  field("load5", "load5", "number", 100),
  field("load15", "load15", "number", 105),
  field("memAvailable", "mem_available", "kib", 145),
  field("memTotal", "mem_total", "kib", 135),
  field("cpuPressure", "cpu_pressure", "number", 120),
  field("memoryPressure", "memory_pressure", "number", 145),
  field("ioPressure", "io_pressure", "number", 120),
]

export function SystemTable({
  cursor,
  locale,
  onCursor,
  rows,
  t,
}: {
  readonly cursor: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly rows: readonly SystemSnapshot[]
  readonly t: Translate
}) {
  const [sorting, setSorting] = useState<SortingState>([{ id: "timestamp", desc: true }])
  const columns = useMemo<ColumnDef<SystemSnapshot>[]>(() => FIELDS.map((entry) => ({
    accessorFn: (row) => row[entry.id],
    cell: ({ getValue }) => renderValue(getValue(), entry.kind, locale),
    header: ({ column }) => <div className="column-head">
      <button aria-label={column.getIsSorted() === "asc" ? t("common.sort_desc") : t("common.sort_asc")} onClick={column.getToggleSortingHandler()} type="button">{t(entry.label)}<span className="sort-mark">{column.getIsSorted() === "asc" ? "↑" : column.getIsSorted() === "desc" ? "↓" : ""}</span></button>
      <span className="column-help"><LabelHelp helpKey={entry.help} iconOnly labelKey={entry.label} t={t} /></span>
    </div>,
    id: entry.id,
    size: entry.size,
    sortUndefined: "last",
  })), [locale, t])
  const table = useReactTable({
    columns,
    data: rows as SystemSnapshot[],
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => String(row.timestamp),
    getSortedRowModel: getSortedRowModel(),
    onSortingChange: setSorting,
    state: { sorting },
  })
  const displayed = table.getRowModel().rows
  const scroll = useRef<HTMLDivElement>(null)
  const virtual = useVirtualizer({ count: displayed.length, estimateSize: () => 31, getScrollElement: () => scroll.current, overscan: 10 })
  const width = table.getTotalSize()
  const selected = closest(rows, cursor)
  return (
    <section className="system-table-shell">
      <h2>{t("system_table.title")}</h2>
      <div aria-label={t("system_table.title")} className="system-table" data-testid="system-snapshot-table" role="table">
        <div className="system-scroll" ref={scroll}>
          <div className="system-head" role="row" style={{ width }}>
            {table.getHeaderGroups()[0]?.headers.map((header) => <div className="system-header-cell" key={header.id} role="columnheader" style={{ width: header.getSize() }}>{flexRender(header.column.columnDef.header, header.getContext())}</div>)}
          </div>
          {displayed.length === 0
            ? <p className="table-empty">{t("system_table.empty")}</p>
            : <div className="system-body" style={{ height: virtual.getTotalSize(), width }}>
              {virtual.getVirtualItems().map((item) => {
                const row = displayed[item.index]
                if (row === undefined) return null
                return <div
                  aria-selected={row.original.timestamp === selected}
                  className="system-row"
                  key={row.id}
                  onClick={() => onCursor(row.original.timestamp)}
                  onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onCursor(row.original.timestamp) } }}
                  role="row"
                  style={{ height: item.size, transform: `translateY(${item.start}px)`, width }}
                  tabIndex={0}
                >{row.getVisibleCells().map((cell) => <div className="system-cell" key={cell.id} role="cell" style={{ width: cell.column.getSize() }}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</div>)}</div>
              })}
            </div>}
        </div>
      </div>
    </section>
  )
}

function field(id: keyof SystemSnapshot, key: string, kind: Field["kind"], size: number): Field {
  return { id, label: `system_table.${key}.label`, help: `system_table.${key}.help`, kind, size }
}

function renderValue(value: unknown, kind: Field["kind"], locale: Locale): string {
  if (kind === "time") return typeof value === "number" ? formatUtc(value) : "—"
  const cell = typeof value === "number" ? value : null
  if (kind === "percent") return measure(cell, locale, "%")
  if (kind === "kib") return measure(cell, locale, " KiB")
  return measure(cell, locale)
}

function closest(rows: readonly SystemSnapshot[], cursor: number): number | null {
  let timestamp: number | null = null
  let distance = Number.POSITIVE_INFINITY
  for (const row of rows) {
    const candidate = Math.abs(row.timestamp - cursor)
    if (candidate < distance || (candidate === distance && (timestamp === null || row.timestamp < timestamp))) {
      timestamp = row.timestamp
      distance = candidate
    }
  }
  return timestamp
}
