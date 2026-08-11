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

import type { Cell, DataRow, Finding } from "./api"
import { fittedWidth, headerWidths, widestCell } from "./column-size"
import { globMatcher } from "./glob"
import { LabelHelp, type Translate } from "./help"
import { rowMatchesLocator } from "./locator"
import { TableFilter } from "./table-filter"
import { asNumber, formatUtc, identifier, measure, rawText, value, type Locale } from "./model"

export interface EntityColumn {
  readonly field: string
  /** The exact physical field represented by this semantic column. */
  readonly physicalField?: string | Readonly<Record<string, string>>
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
  finding,
  findingField,
  label,
  locale,
  onOrder,
  onPattern,
  onSelect,
  order,
  pattern = "",
  serverSorted,
  rowKey = defaultKey,
  rows,
  selectedKey,
  testId,
  t,
}: {
  readonly columns: readonly EntityColumn[]
  readonly empty: string
  readonly finding?: Finding | null | undefined
  readonly findingField?: string | null | undefined
  readonly label: string
  readonly locale: Locale
  /** Set when the server ordered and cut the rows; the header then asks it
   *  for another order rather than reshuffling what arrived. */
  readonly onOrder?: ((order: TableOrder | null) => void) | undefined
  readonly onPattern?: ((pattern: string) => void) | undefined
  readonly onSelect?: (row: DataRow) => void
  readonly pattern?: string | undefined
  readonly order?: TableOrder | undefined
  readonly serverSorted?: boolean | undefined
  readonly rowKey?: (row: DataRow) => string
  readonly rows: readonly DataRow[]
  readonly selectedKey?: string | null
  readonly testId?: string
  readonly t?: Translate
}) {
  const [sizing, setSizing] = useState<ColumnSizingState>({})
  const ordering = useMemo<SortingState>(() => order === undefined ? [] : [{ id: order.column, desc: order.descending }], [order])
  const parent = useRef<HTMLDivElement>(null)
  const columns = useMemo<ColumnDef<DataRow>[]>(() => fields.map((field, index) => ({
    accessorFn: (row) => sortable(value(row, field.field), field.kind),
    cell: ({ row }) => <Cell cell={value(row.original, field.field)} kind={field.kind} locale={locale} rate={field.rate} t={t} />,
    header: () => t === undefined ? field.label : t(field.label),
    id: field.field,
    meta: {
      numeric: NUMERIC_KINDS.has(field.kind ?? "text"),
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
  // Text of a row is what a person searches by: a query, a database, a role,
  // a wait event — every column that is not a number.
  const data = useMemo(() => {
    const match = globMatcher(pattern)
    if (match === null) return [...rows]
    const texts = fields.filter((field) => field.kind === undefined || field.kind === "text" || field.kind === "id")
    return rows.filter((row) => texts.some((field) => {
      const text = rawText(value(row, field.field))
      return text !== null && match(text)
    }))
  }, [fields, pattern, rows])
  const table = useReactTable({
    columns,
    data,
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => rowKey(row),
    getSortedRowModel: getSortedRowModel(),
    // A table the server cut to its top rows cannot be reordered here: sorting
    // the visible two hundred by another column answers a different question
    // than the two hundred largest by that column. Every table reports its
    // order outward all the same, so the address can carry it.
    manualSorting: serverSorted === true,
    onSortingChange: (updater) => {
      const next = typeof updater === "function" ? updater(ordering) : updater
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
  // A header is the contract of its column: the column is widened to hold it
  // before anything else decides the width.
  const head = useRef<HTMLDivElement>(null)
  const automatic = useRef<ColumnSizingState>({})
  useEffect(() => {
    const row = head.current
    if (row === null) return
    const wanted = headerWidths(row)
    setSizing((current) => {
      const next = { ...current }
      fields.forEach((field, index) => {
        const needed = wanted[index]
        // A width the reader chose by dragging or fitting outranks this one.
        const own = current[field.field] === undefined || current[field.field] === automatic.current[field.field]
        if (needed === undefined || !own) return
        if (needed > (field.width ?? 128)) {
          next[field.field] = needed
          automatic.current[field.field] = needed
        } else {
          delete next[field.field]
        }
      })
      return next
    })
  }, [fields, locale])
  const fit = useCallback((id: string, index: number) => {
    const root = parent.current
    if (root === null) return
    setSizing((current) => ({ ...current, [id]: fittedWidth(widestCell(root, index)) }))
  }, [])
  const rendered = table.getRowModel().rows
  const virtual = useVirtualizer({ count: rendered.length, estimateSize: () => 23, getScrollElement: () => parent.current, overscan: 10 })
  const locatedIndex = finding === null || finding === undefined
    ? -1
    : rendered.findIndex((row) => rowMatchesLocator(row.original, finding))
  useEffect(() => {
    if (locatedIndex >= 0) virtual.scrollToIndex(locatedIndex, { align: "center" })
  }, [finding, locatedIndex, virtual])
  const width = table.getTotalSize()
  return <section aria-label={label} className="entity-table" data-testid={testId}>
    {t !== undefined && onPattern !== undefined && <TableFilter kept={data.length} onPattern={onPattern} pattern={pattern} t={t} total={rows.length} />}
    <div className="entity-scroll" ref={parent} role="table">
      <div className="entity-head" ref={head} role="row" style={{ width }}>
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
        ? <p className="table-empty">{pattern !== "" && t !== undefined ? t("filter.none") : empty}</p>
        : <div className="virtual-body" style={{ height: virtual.getTotalSize(), width }}>
          {virtual.getVirtualItems().map((item) => {
            const row = rendered[item.index]
            if (row === undefined) return null
            const key = rowKey(row.original)
            const located = finding !== null && finding !== undefined && rowMatchesLocator(row.original, finding)
            const activeFinding = located ? finding : null
            return <div
              aria-selected={selectedKey === key}
              className={`entity-row${activeFinding === null ? "" : ` locator-row locator-${activeFinding.kind}`}`}
              data-locator-row={located || undefined}
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
              {row.getVisibleCells().map((cell) => {
                const field = fields.find((candidate) => candidate.field === cell.column.id)
                const exact = activeFinding !== null && field !== undefined && locatorMatchesColumn(field, row.original.typeId, findingField ?? null)
                return <div className={`${sticky(cell.column.columnDef.meta, false)}${exact ? ` locator-cell locator-${activeFinding.kind}` : ""}`} data-locator-cell={exact || undefined} key={cell.id} role="cell" style={{ left: stickyLeft(cell.column.columnDef.meta), width: cell.column.getSize() }}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</div>
              })}
            </div>
          })}
        </div>}
    </div>
  </section>
}

export function locatorMatchesColumn(column: EntityColumn, typeId: string, findingField: string | null): boolean {
  const physical = typeof column.physicalField === "string"
    ? column.physicalField
    : column.physicalField?.[typeId] ?? column.field
  return findingField !== null && physical === findingField
}

/** A number is read by comparing digits: numbers line up on the right, names
 *  on the left, header included. */
const NUMERIC_KINDS = new Set(["number", "bytes", "kib", "milliseconds", "microseconds", "percent"])

export function unit(base: string, rate: boolean | undefined, perSecond = "/s"): string {
  return rate === true ? `${base}${perSecond}` : base
}

function Cell({ cell, kind = "text", locale, rate, t }: { readonly cell: Cell; readonly kind?: EntityColumn["kind"]; readonly locale: Locale; readonly rate?: boolean | undefined; readonly t?: Translate | undefined }) {
  const per = t === undefined ? "/s" : t("unit.per_second")
  if (cell === null) return <span className="null-cell">—</span>
  if (kind === "id") return <span className="entity-value id-value">{identifier(cell)}</span>
  if (kind === "timestamp") {
    const timestamp = asNumber(cell)
    return <span className="entity-value">{timestamp === null ? "—" : formatUtc(timestamp)}</span>
  }
  if (kind === "bytes") return <span className="entity-value">{measure(cell, locale, unit(t === undefined ? " B" : t("unit.byte"), rate, per))}</span>
  if (kind === "kib") return <span className="entity-value">{measure(cell, locale, unit(" KiB", rate, per))}</span>
  if (kind === "milliseconds") return <span className="entity-value">{measure(cell, locale, unit(t === undefined ? " ms" : t("unit.ms"), rate, per))}</span>
  if (kind === "microseconds") return <span className="entity-value">{measure(cell, locale, unit(t === undefined ? " μs" : t("unit.us"), rate, per))}</span>
  if (kind === "percent") return <span className="entity-value">{measure(cell, locale, unit("%", rate, per))}</span>
  if (kind === "boolean") return <span className="entity-value">{cell === true ? locale === "ru" ? "да" : "true" : cell === false ? locale === "ru" ? "нет" : "false" : rawText(cell) ?? "—"}</span>
  if (kind === "number") return <span className="entity-value">{measure(cell, locale, unit("", rate, per))}</span>
  return <span className="entity-value text-value" title={rawText(cell) ?? "—"}>{rawText(cell) ?? "—"}</span>
}

function sortable(cell: Cell, kind: EntityColumn["kind"]): string | number | boolean | null {
  if (kind === "text") return rawText(cell)
  if (kind === "boolean") return typeof cell === "boolean" ? cell : rawText(cell)
  return asNumber(cell) ?? rawText(cell)
}

function sticky(meta: unknown, head: boolean): string {
  const cell = meta as { readonly sticky?: boolean; readonly numeric?: boolean } | undefined
  return [
    head ? "entity-header-cell" : "entity-cell",
    cell?.numeric === true ? "align-right" : "",
    cell?.sticky === true ? "entity-sticky" : "",
  ].filter(Boolean).join(" ")
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
