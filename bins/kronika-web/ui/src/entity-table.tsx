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
import { useCallback, useMemo, useRef, useState } from "react"

import type { Cell, DataRow } from "./api"
import { fittedWidth, widestCell } from "./column-size"
import { LabelHelp, type Translate } from "./help"
import { asNumber, formatUtc, identifier, measure, rawText, value, type Locale } from "./model"

export interface EntityColumn {
  readonly field: string
  readonly label: string
  readonly help?: string
  /** The server divided this column by the interval: it reads per second. */
  readonly rate?: boolean
  readonly kind?: "id" | "number" | "text" | "timestamp" | "bytes" | "kib" | "milliseconds" | "microseconds" | "percent" | "boolean"
  readonly width?: number
  readonly sticky?: boolean
}

/** A column and a direction, as the server understands them. */
export interface TableOrder {
  readonly column: string
  readonly descending: boolean
}

export function EntityTable({
  columns: fields,
  empty,
  label,
  locale,
  onOrder,
  onSelect,
  order,
  rowKey = defaultKey,
  rows,
  selectedKey,
  testId,
  t,
}: {
  readonly columns: readonly EntityColumn[]
  readonly empty: string
  readonly label: string
  readonly locale: Locale
  /** Set when the server ordered and cut the rows; the header then asks it
   *  for another order rather than reshuffling what arrived. */
  readonly onOrder?: ((order: TableOrder | null) => void) | undefined
  readonly onSelect?: (row: DataRow) => void
  readonly order?: TableOrder | undefined
  readonly rowKey?: (row: DataRow) => string
  readonly rows: readonly DataRow[]
  readonly selectedKey?: string | null
  readonly testId?: string
  readonly t?: Translate
}) {
  // A table the server cut to its top rows cannot be reordered here: sorting
  // the visible two hundred by another column answers a different question
  // than the two hundred largest by that column.
  const [sorting, setSorting] = useState<SortingState>([])
  const [sizing, setSizing] = useState<ColumnSizingState>({})
  const ordering: SortingState = order === undefined
    ? sorting
    : [{ id: order.column, desc: order.descending }]
  const parent = useRef<HTMLDivElement>(null)
  const columns = useMemo<ColumnDef<DataRow>[]>(() => fields.map((field, index) => ({
    accessorFn: (row) => sortable(value(row, field.field), field.kind),
    cell: ({ row }) => <Cell cell={value(row.original, field.field)} kind={field.kind} locale={locale} rate={field.rate} />,
    header: () => t === undefined ? field.label : t(field.label),
    id: field.field,
    meta: {
      sticky: field.sticky === true,
      stickyLeft: fields.slice(0, index).reduce((left, candidate) => left + (candidate.sticky === true ? candidate.width ?? 128 : 0), 0),
      help: field.help,
      label: field.label,
    },
    size: field.width ?? 128,
  })), [fields, locale, t])
  // The table keeps its own model keyed on this reference. A fresh array every
  // render rebuilds that model every render, and the process table next door
  // does not do it.
  const data = useMemo(() => [...rows], [rows])
  const table = useReactTable({
    columns,
    data,
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => rowKey(row),
    getSortedRowModel: getSortedRowModel(),
    manualSorting: order !== undefined,
    onSortingChange: (updater) => {
      const next = typeof updater === "function" ? updater(ordering) : updater
      if (order === undefined) {
        setSorting(next)
        return
      }
      const first = next[0]
      onOrder?.(first === undefined ? null : { column: first.id, descending: first.desc })
    },
    columnResizeMode: "onChange",
    enableColumnResizing: true,
    onColumnSizingChange: setSizing,
    state: { columnSizing: sizing, sorting: ordering },
  })
  // A grip is dragged to resize and double clicked to fit: the width of the
  // widest cell on screen, which is what a person means by "fit".
  const fit = useCallback((id: string, index: number) => {
    const root = parent.current
    if (root === null) return
    setSizing((current) => ({ ...current, [id]: fittedWidth(widestCell(root, index)) }))
  }, [])
  const rendered = table.getRowModel().rows
  const virtual = useVirtualizer({ count: rendered.length, estimateSize: () => 23, getScrollElement: () => parent.current, overscan: 10 })
  const width = table.getTotalSize()
  return <section aria-label={label} className="entity-table" data-testid={testId}>
    <div className="entity-scroll" ref={parent} role="table">
      <div className="entity-head" role="row" style={{ width }}>
        {table.getHeaderGroups()[0]?.headers.map((header, index) => {
          const sorted = header.column.getIsSorted()
          return <div className={sticky(header.column.columnDef.meta, true)} key={header.id} role="columnheader" style={{ left: stickyLeft(header.column.columnDef.meta), width: header.getSize() }}>
            <button className="entity-sort" onClick={header.column.getToggleSortingHandler()} type="button">
              <span>{flexRender(header.column.columnDef.header, header.getContext())}</span>
              {sorted !== false && <i>{sorted === "asc" ? "↑" : "↓"}</i>}
            </button>
            {t !== undefined && columnHelp(header.column.columnDef.meta) !== null && <LabelHelp helpKey={columnHelp(header.column.columnDef.meta)!.help} iconOnly labelKey={columnHelp(header.column.columnDef.meta)!.label} t={t} />}
            <span className="column-grip" onDoubleClick={() => fit(header.column.id, index)} onMouseDown={header.getResizeHandler()} onTouchStart={header.getResizeHandler()} />
          </div>
        })}
      </div>
      {rendered.length === 0
        ? <p className="table-empty">{empty}</p>
        : <div className="virtual-body" style={{ height: virtual.getTotalSize(), width }}>
          {virtual.getVirtualItems().map((item) => {
            const row = rendered[item.index]
            if (row === undefined) return null
            const key = rowKey(row.original)
            return <div
              aria-selected={selectedKey === key}
              className="entity-row"
              key={row.id}
              onClick={() => onSelect?.(row.original)}
              onKeyDown={(event) => {
                if (onSelect === undefined || (event.key !== "Enter" && event.key !== " ")) return
                event.preventDefault()
                onSelect(row.original)
              }}
              role="row"
              style={{ height: item.size, transform: `translateY(${item.start}px)`, width }}
              tabIndex={onSelect === undefined ? undefined : 0}
            >
              {row.getVisibleCells().map((cell) => <div className={sticky(cell.column.columnDef.meta, false)} key={cell.id} role="cell" style={{ left: stickyLeft(cell.column.columnDef.meta), width: cell.column.getSize() }}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</div>)}
            </div>
          })}
        </div>}
    </div>
  </section>
}

export function unit(base: string, rate: boolean | undefined): string {
  return rate === true ? `${base}/s` : base
}

function Cell({ cell, kind = "text", locale, rate }: { readonly cell: Cell; readonly kind?: EntityColumn["kind"]; readonly locale: Locale; readonly rate?: boolean | undefined }) {
  if (cell === null) return <span className="null-cell">—</span>
  if (kind === "id") return <span className="entity-value id-value">{identifier(cell)}</span>
  if (kind === "timestamp") {
    const timestamp = asNumber(cell)
    return <span className="entity-value">{timestamp === null ? "—" : formatUtc(timestamp)}</span>
  }
  if (kind === "bytes") return <span className="entity-value">{measure(cell, locale, unit(" B", rate))}</span>
  if (kind === "kib") return <span className="entity-value">{measure(cell, locale, unit(" KiB", rate))}</span>
  if (kind === "milliseconds") return <span className="entity-value">{measure(cell, locale, unit(" ms", rate))}</span>
  if (kind === "microseconds") return <span className="entity-value">{measure(cell, locale, unit(" μs", rate))}</span>
  if (kind === "percent") return <span className="entity-value">{measure(cell, locale, unit("%", rate))}</span>
  if (kind === "boolean") return <span className="entity-value">{cell === true ? locale === "ru" ? "да" : "true" : cell === false ? locale === "ru" ? "нет" : "false" : rawText(cell) ?? "—"}</span>
  if (kind === "number") return <span className="entity-value">{measure(cell, locale, unit("", rate))}</span>
  return <span className="entity-value text-value" title={rawText(cell) ?? "—"}>{rawText(cell) ?? "—"}</span>
}

function sortable(cell: Cell, kind: EntityColumn["kind"]): string | number | boolean | null {
  if (kind === "text") return rawText(cell)
  if (kind === "boolean") return typeof cell === "boolean" ? cell : rawText(cell)
  return asNumber(cell) ?? rawText(cell)
}

function sticky(meta: unknown, head: boolean): string {
  const enabled = (meta as { readonly sticky?: boolean } | undefined)?.sticky === true
  return `${head ? "entity-header-cell" : "entity-cell"}${enabled ? " entity-sticky" : ""}`
}

function stickyLeft(meta: unknown): number | undefined {
  const value = meta as { readonly sticky?: boolean; readonly stickyLeft?: number } | undefined
  return value?.sticky === true ? value.stickyLeft ?? 0 : undefined
}

function columnHelp(meta: unknown): { readonly help: string; readonly label: string } | null {
  const value = meta as { readonly help?: string; readonly label?: string } | undefined
  return value?.help === undefined || value.label === undefined ? null : { help: value.help, label: value.label }
}

function defaultKey(row: DataRow): string {
  return `${row.segmentId}:${row.typeId}:${row.ordinal}`
}
