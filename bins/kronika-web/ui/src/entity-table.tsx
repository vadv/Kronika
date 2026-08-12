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
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { Cell, DataRow, Finding } from "./api"
import { fittedWidth, headerWidths, widestCell } from "./column-size"
import { globMatcher } from "./glob"
import { LabelHelp, type Translate } from "./help"
import { rowMatchesLocator } from "./locator"
import { TableFilter } from "./table-filter"
import { asNumber, formatUtc, humanDuration, identifier, measure, rawText, value, type Locale } from "./model"
import { semanticValueTone } from "./value-tone"

export interface EntityColumn {
  readonly field: string
  readonly physicalField?: string | Readonly<Record<string, string>>
  readonly label: string
  readonly help?: string
  readonly render?: (row: DataRow) => ReactNode
  readonly filterValue?: (row: DataRow) => string | null
  readonly sortValue?: (row: DataRow) => string | number | boolean | null
  readonly rate?: boolean
  readonly kind?: "id" | "number" | "text" | "timestamp" | "bytes" | "kib" | "milliseconds" | "duration" | "microseconds" | "percent" | "boolean"
  readonly width?: number
  readonly sticky?: boolean | string
  readonly sortable?: boolean
}

export interface TableOrder {
  readonly column: string
  readonly descending: boolean
}

export function EntityTable({
  className,
  columns: fields,
  contextLabel,
  empty,
  finding,
  findingField,
  label,
  locale,
  onOrder,
  onPattern,
  onNearEnd,
  onContextClear,
  onSelect,
  order,
  pattern = "",
  serverSorted,
  rowKey = defaultKey,
  rowLabel,
  rows,
  selectedKey,
  status,
  testId,
  t,
}: {
  readonly className?: string | undefined
  readonly columns: readonly EntityColumn[]
  readonly contextLabel?: string | undefined
  readonly empty: string
  readonly finding?: Finding | null | undefined
  readonly findingField?: string | null | undefined
  readonly label: string
  readonly locale: Locale
  readonly onOrder?: ((order: TableOrder | null) => void) | undefined
  readonly onPattern?: ((pattern: string) => void) | undefined
  readonly onNearEnd?: (() => void) | undefined
  readonly onContextClear?: (() => void) | undefined
  readonly onSelect?: (row: DataRow) => void
  readonly pattern?: string | undefined
  readonly order?: TableOrder | undefined
  readonly serverSorted?: boolean | undefined
  readonly rowKey?: (row: DataRow) => string
  readonly rowLabel?: ((row: DataRow) => string) | undefined
  readonly rows: readonly DataRow[]
  readonly selectedKey?: string | null
  readonly status?: ReactNode | undefined
  readonly testId?: string
  readonly t?: Translate
}) {
  const [sizing, setSizing] = useState<ColumnSizingState>({})
  const ordering = useMemo<SortingState>(() => order === undefined
    ? []
    : [{ id: order.column, desc: order.descending }], [order])
  const parent = useRef<HTMLDivElement>(null)
  const columns = useMemo<ColumnDef<DataRow>[]>(() => fields.map((field, index) => ({
    accessorFn: (row) => field.sortValue === undefined ? sortable(value(row, field.field), field.kind) : field.sortValue(row),
    cell: ({ row }) => field.render === undefined ? <Cell cell={value(row.original, field.field)} kind={field.kind} locale={locale} rate={field.rate} t={t} /> : field.render(row.original),
    header: () => t === undefined ? field.label : t(field.label),
    id: field.field,
    enableSorting: serverSorted === true ? field.sortable === true : field.sortable !== false,
    meta: {
      numeric: NUMERIC_KINDS.has(field.kind ?? "text"),
      sticky: field.sticky,
      stickyLeft: fields.slice(0, index).reduce((left, candidate) => left + (candidate.sticky === undefined || candidate.sticky === false ? 0 : candidate.width ?? 128), 0),
      help: field.help,
      label: field.label,
    },
    size: field.width ?? 128,
    ...(field.sortValue === undefined ? {} : { sortUndefined: "last" as const }),
  })), [fields, locale, serverSorted, t])
  const data = useMemo(
    () => filterTableRows(rows, fields, pattern, serverSorted === true),
    [fields, pattern, rows, serverSorted],
  )
  const table = useReactTable({
    columns,
    data,
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => rowKey(row),
    getSortedRowModel: getSortedRowModel(),
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
  const lastVirtualIndex = virtual.getVirtualItems().at(-1)?.index ?? -1
  useEffect(() => {
    if (onNearEnd !== undefined && rendered.length !== 0 && lastVirtualIndex >= rendered.length - 10) onNearEnd()
  }, [lastVirtualIndex, onNearEnd, rendered.length])
  const locatedIndex = finding === null || finding === undefined
    ? -1
    : rendered.findIndex((row) => rowMatchesLocator(row.original, finding))
  useEffect(() => {
    if (locatedIndex >= 0) virtual.scrollToIndex(locatedIndex, { align: "center" })
  }, [finding, locatedIndex, virtual])
  const width = table.getTotalSize()
  return <section className={`entity-table${className === undefined ? "" : ` ${className}`}`} data-testid={testId}>
    {status !== undefined && <div className="table-status" data-testid="table-status">{status}</div>}
    {t !== undefined && (onPattern !== undefined || contextLabel !== undefined) && <TableFilter context={contextLabel} kept={serverSorted === true ? -1 : data.length} onContextClear={onContextClear} onPattern={onPattern} pattern={pattern} t={t} total={rows.length} />}
    <div aria-label={label} className="entity-scroll" ref={parent} role="table">
      <div className="entity-head" ref={head} role="row" style={{ width }}>
        {table.getHeaderGroups()[0]?.headers.map((header, index) => {
          const sorted = header.column.getIsSorted()
          return <div className={sticky(header.column.columnDef.meta, true)} key={header.id} role="columnheader" style={{ left: stickyLeft(header.column.columnDef.meta), width: header.getSize() }}>
            <button className="entity-sort" disabled={!header.column.getCanSort()} onClick={serverSorted === true
              ? () => onOrder?.(nextServerOrder(order, header.column.id))
              : header.column.getToggleSortingHandler()} type="button">
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
              aria-label={rowLabel?.(row.original)}
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
                const stored = field === undefined ? null : value(row.original, field.field)
                const tone = field === undefined ? null : semanticValueTone(field.field, stored, field.rate, row.original)
                const toneText = tone === null || tone === "inactive" || t === undefined ? null : t(`pg.value.${tone}`)
                return <div aria-label={toneText === null ? undefined : `${toneText}: ${rawText(stored) ?? "—"}`} className={`${sticky(cell.column.columnDef.meta, false)}${tone === null ? "" : ` value-tone-${tone}`}${exact ? ` locator-cell locator-${activeFinding.kind}` : ""}`} data-locator-cell={exact || undefined} data-value-tone={tone ?? undefined} key={cell.id} role="cell" style={{ left: stickyLeft(cell.column.columnDef.meta), width: cell.column.getSize() }}>
                  {toneText !== null && <span aria-hidden="true" className="value-tone-mark" />}
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </div>
              })}
            </div>
          })}
        </div>}
    </div>
  </section>
}

export function nextServerOrder(current: TableOrder | undefined, column: string): TableOrder | null {
  if (current?.column !== column) return { column, descending: true }
  if (current.descending) return { column, descending: false }
  return null
}

export function filterTableRows(
  rows: readonly DataRow[],
  fields: readonly EntityColumn[],
  pattern: string,
  serverFiltered: boolean,
): DataRow[] {
  if (serverFiltered) return [...rows]
  const match = globMatcher(pattern)
  if (match === null) return [...rows]
  const searchable = fields.filter((field) => field.filterValue !== undefined || field.kind === undefined || field.kind === "text" || field.kind === "id")
  return rows.filter((row) => searchable.some((field) => {
    const text = field.filterValue === undefined ? rawText(value(row, field.field)) : field.filterValue(row)
    return text !== null && match(text)
  }))
}

export function locatorMatchesColumn(column: EntityColumn, typeId: string, findingField: string | null): boolean {
  const physical = typeof column.physicalField === "string"
    ? column.physicalField
    : column.physicalField?.[typeId] ?? column.field
  return findingField !== null && physical === findingField
}

const NUMERIC_KINDS = new Set(["number", "bytes", "kib", "milliseconds", "duration", "microseconds", "percent"])

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
  if (kind === "duration") return <span className="entity-value">{humanDuration(cell, locale)}</span>
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
  const cell = meta as { readonly sticky?: boolean | string; readonly numeric?: boolean } | undefined
  return [
    head ? "entity-header-cell" : "entity-cell",
    cell?.numeric === true ? "align-right" : "",
    cell?.sticky === true ? "entity-sticky" : typeof cell?.sticky === "string" ? cell.sticky : "",
  ].filter(Boolean).join(" ")
}

function stickyLeft(meta: unknown): number | undefined {
  const value = meta as { readonly sticky?: boolean | string; readonly stickyLeft?: number } | undefined
  return value?.sticky === undefined || value.sticky === false ? undefined : value.stickyLeft ?? 0
}

function columnHelp(meta: unknown): { readonly help: string; readonly label: string } | null {
  const value = meta as { readonly help?: string; readonly label?: string } | undefined
  return value?.help === undefined || value.label === undefined ? null : { help: value.help, label: value.label }
}

function defaultKey(row: DataRow): string {
  return `${row.segmentId}:${row.typeId}:${row.ordinal}`
}
