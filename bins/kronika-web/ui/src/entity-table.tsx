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
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { Cell, DataRow, Finding } from "./api"
import { fittedWidth, headerWidths, widestCell } from "./column-size"
import { useDisplayTime } from "./display-time-context"
import { globMatcher } from "./glob"
import { LabelHelp, type Translate } from "./help"
import { rowMatchesLocator } from "./locator"
import { parseSearch, rowMatchesSearch, type SearchSurface } from "./search"
import { IDLE_SEARCH_REQUEST, SearchRequestMessage, type SearchRequestState } from "./search-request"
import { TableFilter } from "./table-filter"
import { asNumber, estimatedRows, humanBytes, humanCores, humanDuration, humanPercent, identifier, measure, rawText, value, type Locale } from "./model"
import { semanticValueTone } from "./value-tone"

// Matches --spacing-row: virtualized offsets and the content-sized height
// are computed from the same number the CSS row height uses.
const ROW_PX = 24
const SKELETON_ROWS = 8

export interface EntityColumn {
  readonly field: string
  readonly physicalField?: string | Readonly<Record<string, string>>
  readonly label: string
  readonly help?: string
  readonly render?: (row: DataRow) => ReactNode
  readonly renderNull?: (row: DataRow) => ReactNode
  readonly filterValue?: (row: DataRow) => string | null
  readonly sortValue?: (row: DataRow) => string | number | boolean | null
  readonly rate?: boolean
  readonly valueScale?: number | null
  readonly kind?: "id" | "number" | "estimated_rows" | "text" | "timestamp" | "bytes" | "kib" | "milliseconds" | "duration" | "microseconds" | "percent" | "cores" | "boolean"
  readonly width?: number
  readonly sticky?: boolean | string
  readonly sortable?: boolean
  readonly available?: ((row: DataRow) => boolean) | undefined
}

export interface TableOrder {
  readonly column: string
  readonly descending: boolean
}

export function EntityTable({
  className,
  columns: fields,
  contentSized = false,
  contextLabel,
  empty,
  finding,
  findingField,
  label,
  loading = false,
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
  searchGrouped = false,
  searchRequest = IDLE_SEARCH_REQUEST,
  searchSurface,
  selectedKey,
  status,
  testId,
  t,
}: {
  readonly className?: string | undefined
  readonly columns: readonly EntityColumn[]
  readonly contentSized?: boolean | undefined
  readonly contextLabel?: string | undefined
  readonly empty: string
  readonly finding?: Finding | null | undefined
  readonly findingField?: string | null | undefined
  readonly label: string
  readonly loading?: boolean | undefined
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
  readonly searchGrouped?: boolean | undefined
  readonly searchRequest?: SearchRequestState | undefined
  readonly searchSurface?: SearchSurface | undefined
  readonly selectedKey?: string | null
  readonly status?: ReactNode | undefined
  readonly testId?: string
  readonly t: Translate
}) {
  const [sizing, setSizing] = useState<ColumnSizingState>({})
  const ordering = useMemo<SortingState>(() => order === undefined
    ? []
    : [{ id: order.column, desc: order.descending }], [order])
  const parent = useRef<HTMLDivElement>(null)
  const columns = useMemo<ColumnDef<DataRow>[]>(() => fields.map((field) => ({
    accessorFn: (row) => field.sortValue === undefined ? sortable(value(row, field.field), field.kind) : field.sortValue(row),
    cell: ({ row }) => {
      const stored = value(row.original, field.field)
      if (stored === null && field.renderNull !== undefined) return field.renderNull(row.original)
      return field.render === undefined ? <Cell at={row.original.relation ? row.original.timestamp : null} cell={stored} field={field.field} kind={field.kind} locale={locale} rate={field.rate} t={t} valueScale={field.valueScale} /> : field.render(row.original)
    },
    header: () => t(field.label),
    id: field.field,
    enableSorting: serverSorted === true ? field.sortable === true : field.sortable !== false,
    meta: {
      numeric: NUMERIC_KINDS.has(field.kind ?? "text"),
      sticky: field.sticky,
      help: field.help,
      label: field.label,
    },
    size: field.width ?? 128,
    ...(field.sortValue === undefined ? {} : { sortUndefined: "last" as const }),
  })), [fields, locale, serverSorted, t])
  const data = useMemo(
    () => filterTableRows(rows, fields, pattern, serverSorted === true, searchSurface),
    [fields, pattern, rows, searchSurface, serverSorted],
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
  const pinnedLeft = stickyOffsets(table.getVisibleLeafColumns().map((column) => ({
    id: column.id,
    size: column.getSize(),
    sticky: isSticky(column.columnDef.meta),
  })))
  const head = useRef<HTMLDivElement>(null)
  const [headHeight, setHeadHeight] = useState(28)
  const [horizontalRailHeight, setHorizontalRailHeight] = useState(0)
  const automatic = useRef<ColumnSizingState>({})
  useLayoutEffect(() => {
    const measured = head.current?.getBoundingClientRect().height
    if (measured !== undefined && measured !== headHeight) setHeadHeight(measured)
  }, [fields, headHeight, locale])
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
  const virtual = useVirtualizer({ count: rendered.length, estimateSize: () => ROW_PX, getScrollElement: () => parent.current, overscan: 10 })
  const lastVirtualIndex = virtual.getVirtualItems().at(-1)?.index ?? -1
  useEffect(() => {
    if (onNearEnd !== undefined && rendered.length !== 0 && lastVirtualIndex >= rendered.length - 10) onNearEnd()
  }, [lastVirtualIndex, onNearEnd, rendered.length])
  const locatedIndex = finding === null || finding === undefined
    ? -1
    : rendered.findIndex((row) => rowMatchesLocator(row.original, finding))
  useLayoutEffect(() => {
    // A dense table may have a retained vertical offset when filtering makes
    // it sparse. Horizontal-only mode owns no vertical navigation, so reveal
    // the exact header/row stack before paint.
    if (contentSized && parent.current !== null) parent.current.scrollTop = 0
  }, [contentSized, rendered.length])
  useEffect(() => {
    if (!contentSized && locatedIndex >= 0) virtual.scrollToIndex(locatedIndex, { align: "center" })
  }, [contentSized, finding, locatedIndex, virtual])
  const width = table.getTotalSize()
  // This is real scrollable content, not scroll-padding: at the rightmost
  // position the last header help/grip and cell stop before the owning edge.
  const contentWidth = width + TABLE_END_GUTTER
  useLayoutEffect(() => {
    const root = parent.current
    if (root === null || !contentSized) {
      setHorizontalRailHeight(0)
      return
    }
    const measureRail = () => {
      const measured = root.scrollWidth > root.clientWidth + 1 ? root.offsetHeight - root.clientHeight : 0
      setHorizontalRailHeight((current) => current === measured ? current : measured)
    }
    measureRail()
    const observer = new ResizeObserver(measureRail)
    observer.observe(root)
    return () => observer.disconnect()
  }, [contentSized, rendered.length, width])
  const contentHeight = contentSized ? (rendered.length === 0 ? (loading ? headHeight + SKELETON_ROWS * ROW_PX : 72) : Math.min(310, headHeight + rendered.length * ROW_PX)) + horizontalRailHeight : undefined
  const virtualHeight = contentSized ? rendered.length * ROW_PX : virtual.getTotalSize()
  const searchPending = searchRequest.phase === "pending"
  const searchMessage = searchRequest.phase === "pending" || searchRequest.phase === "error"
    ? <SearchRequestMessage request={searchRequest} t={t} />
    : null
  return <section aria-busy={searchPending} className={`entity-table min-w-0 overflow-hidden bg-s1${contentSized ? "" : " pg-stretch"}${className === undefined ? "" : ` ${className}`}`} data-testid={testId}>
    {(status !== undefined || searchMessage !== null) && onPattern === undefined && contextLabel === undefined && <div className="flex min-h-[26px] min-w-0 items-center gap-x-[14px] overflow-hidden whitespace-nowrap border-b border-line2 bg-[color-mix(in_srgb,var(--color-s2)_82%,transparent)] px-[7px] py-1 text-xs text-fg3 [&_strong]:font-semibold [&_strong]:text-fg2" data-testid="table-status">{searchMessage ?? status}</div>}
    {(onPattern !== undefined || contextLabel !== undefined) && <TableFilter context={contextLabel} grouped={searchGrouped} kept={serverSorted === true ? -1 : data.length} onContextClear={onContextClear} onPattern={onPattern} pattern={pattern} status={searchMessage ?? status} surface={searchSurface ?? "os_process"} t={t} total={rows.length} />}
    <div aria-label={label} className={`entity-scroll relative h-[min(310px,36vh)] min-h-[154px] [scroll-padding-inline-end:8px] focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent [.process-table_&]:h-auto [.process-table_&]:min-h-0 [.process-table_&]:flex-1 [.pg-entity-layout_&]:h-[min(560px,calc(100dvh-475px))] [.pg-entity-layout_&]:min-h-[100px] [.pg-table-shell_.pg-entity-layout_&]:h-auto [.pg-table-shell_.pg-entity-layout_&]:min-h-0 [.pg-table-shell_.pg-entity-layout_&]:flex-1${contentSized ? " !min-h-0 box-content overflow-x-auto overflow-y-hidden" : " overflow-auto"}`} data-scroll-axis={contentSized ? "horizontal" : "both"} ref={parent} role="table" style={contentHeight === undefined ? undefined : { height: contentHeight }} tabIndex={0}>
      <div className="entity-head sticky top-0 z-30 flex h-head min-w-full bg-s2 pr-2 coarse:h-9 [&_[role=columnheader]]:select-none" ref={head} role="row" style={{ width: contentWidth }}>
        {table.getHeaderGroups()[0]?.headers.map((header, index) => {
          const sorted = header.column.getIsSorted()
          return <div className={sticky(header.column.columnDef.meta, true)} key={header.id} role="columnheader" style={{ left: pinnedLeft.get(header.column.id), width: header.getSize() }}>
            <button className="entity-sort flex h-full min-w-0 flex-auto cursor-pointer items-center justify-between border-0 bg-transparent p-0 text-left text-[inherit] disabled:cursor-default [&>span]:overflow-hidden [&>span]:text-ellipsis [&>span]:whitespace-nowrap" disabled={!header.column.getCanSort()} onClick={serverSorted === true
              ? () => onOrder?.(nextServerOrder(order, header.column.id))
              : header.column.getToggleSortingHandler()} type="button">
              <span>{flexRender(header.column.columnDef.header, header.getContext())}</span>
              {sorted !== false && <i className="ml-[5px] not-italic text-accent2">{sorted === "asc" ? "↑" : "↓"}</i>}
            </button>
            {columnHelp(header.column.columnDef.meta) !== null && <LabelHelp helpKey={columnHelp(header.column.columnDef.meta)!.help} iconOnly labelKey={columnHelp(header.column.columnDef.meta)!.label} t={t} />}
            <span className="column-grip absolute bottom-0 right-0 top-0 w-[7px] cursor-col-resize touch-none hover:bg-accent3 hover:opacity-50" onDoubleClick={() => fit(header.column.id, index)} onMouseDown={header.getResizeHandler()} onTouchStart={header.getResizeHandler()} />
          </div>
        })}
      </div>
      {rendered.length === 0
        ? searchPending
          ? <p aria-hidden="true" className="table-empty">{t("filter.searching")}</p>
          : searchRequest.phase === "error"
            ? <div className="table-empty" role="alert">{searchMessage}</div>
            : loading
          // Loading and empty are different truths; never report one as the other.
              ? <div role="status">
                <p className="absolute m-0 h-px w-px overflow-hidden whitespace-nowrap [clip-path:inset(50%)]">{t("table.loading")}</p>
                <div aria-hidden="true" data-testid="table-skeleton">
                  {Array.from({ length: SKELETON_ROWS }, (_, rowIndex) => <div className="flex h-row items-center border-b border-line" key={rowIndex} style={{ width: contentWidth }}>
                    {table.getVisibleLeafColumns().map((column, columnIndex) => <span className="px-[7px]" key={column.id} style={{ width: column.getSize() }}>
                      <span className="block h-2 animate-skeleton rounded-[3px] bg-s3 motion-reduce:animate-none" style={{ animationDelay: `${rowIndex * -90}ms`, width: `${45 + (rowIndex * 7 + columnIndex * 13) % 40}%` }} />
                    </span>)}
                  </div>)}
                </div>
              </div>
              : <p className="table-empty flex items-center gap-2.5">{pattern === "" ? empty : <>{t("filter.none")}{onPattern !== undefined && <button className="cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4" data-testid="table-clear-filter" onClick={() => onPattern("")} type="button">{t("filter.clear")}</button>}</>}</p>
        : <div className="relative" data-testid="virtual-body" style={{ height: virtualHeight, width: contentWidth }}>
          {virtual.getVirtualItems().map((item) => {
            const row = rendered[item.index]
            if (row === undefined) return null
            const key = rowKey(row.original)
            const located = finding !== null && finding !== undefined && rowMatchesLocator(row.original, finding)
            const activeFinding = located ? finding : null
            return <div
              aria-label={rowLabel?.(row.original)}
              aria-selected={selectedKey === key}
              className={`entity-row absolute left-0 top-0 flex min-w-full bg-s1 even:bg-zebra aria-selected:bg-s4 aria-selected:shadow-[inset_2px_0_var(--color-accent)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent [.process-table_&]:cursor-pointer [.process-table_&]:hover:bg-s3 ${onSelect === undefined ? "cursor-default" : "cursor-pointer hover:bg-s3"}${activeFinding === null ? "" : ` ${LOCATOR_ROW[activeFinding.kind]}`}`}
              data-locator-row={located || undefined}
              key={row.id}
              onClick={() => onSelect?.(row.original)}
              onKeyDown={(event) => {
                if (onSelect === undefined || (event.key !== "Enter" && event.key !== " ")) return
                event.preventDefault()
                onSelect(row.original)
              }}
              role="row"
              style={{ height: item.size, paddingRight: TABLE_END_GUTTER, transform: `translateY(${contentSized ? item.index * ROW_PX : item.start}px)`, width: contentWidth }}
              tabIndex={onSelect === undefined ? undefined : 0}
            >
              {row.getVisibleCells().map((cell) => {
                const field = fields.find((candidate) => candidate.field === cell.column.id)
                const exact = activeFinding !== null && field !== undefined && locatorMatchesColumn(field, row.original.typeId, findingField ?? null)
                const stored = field === undefined ? null : value(row.original, field.field)
                const tone = field === undefined ? null : semanticValueTone(field.field, stored, field.rate, row.original)
                const toneText = tone === null || tone === "inactive" ? null : t(`pg.value.${tone}`)
                return <div aria-label={toneText === null || field === undefined ? undefined : `${toneText}: ${cellAriaValue(stored, field, locale, t)}`} className={`${sticky(cell.column.columnDef.meta, false)}${tone === null ? "" : ` value-tone-${tone} ${VALUE_TONE[tone] ?? ""}`}${exact ? ` locator-cell ${LOCATOR_CELL[activeFinding.kind] ?? ""}` : ""}`} data-locator-cell={exact || undefined} data-value-tone={tone ?? undefined} key={cell.id} role="cell" style={{ left: pinnedLeft.get(cell.column.id), width: cell.column.getSize() }}>
                  {toneText !== null && <span aria-hidden="true" className="mr-[5px] h-2.5 flex-none border-l-2 border-current" />}
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </div>
              })}
            </div>
          })}
        </div>}
    </div>
  </section>
}

// A row and a cell mark a finding with their own edge; selection keeps its
// accent stripe alongside.
const LOCATOR_ROW: Readonly<Record<string, string>> = {
  known_bad: "shadow-[inset_3px_0_var(--color-bad)] aria-selected:shadow-[inset_2px_0_var(--color-accent),inset_5px_0_var(--color-bad)]",
  spike: "shadow-[inset_3px_0_var(--color-warn)] aria-selected:shadow-[inset_2px_0_var(--color-accent),inset_5px_0_var(--color-warn)]",
}
const LOCATOR_CELL: Readonly<Record<string, string>> = {
  known_bad: "bg-[color-mix(in_srgb,var(--color-bad)_18%,transparent)] text-fg-hi [&_.entity-value]:text-fg-hi",
  spike: "bg-[color-mix(in_srgb,var(--color-warn)_12%,transparent)] text-fg-hi [&_.entity-value]:text-fg-hi",
}
const TABLE_END_GUTTER = 8
// Both hooks name the same thing: styles.css treats .entity-value and
// .numeric-cell alike, and process rows label their value span the latter.
const VALUE_TONE: Readonly<Record<string, string>> = {
  good: "[&_.entity-value]:text-ok [&_.numeric-cell]:text-ok",
  warning: "[&_.entity-value]:text-warn [&_.numeric-cell]:text-warn",
  critical: "[&_.entity-value]:text-bad [&_.numeric-cell]:text-bad",
  inactive: "[&_.entity-value]:text-fg4 [&_.numeric-cell]:text-fg4",
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
  surface?: SearchSurface,
): DataRow[] {
  if (serverFiltered) return [...rows]
  if (surface !== undefined) {
    const parsed = parseSearch(pattern, surface)
    if (!parsed.ok) return [...rows]
    return rows.filter((row) => rowMatchesSearch(row, parsed.query, surface))
  }
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

const NUMERIC_KINDS = new Set(["number", "estimated_rows", "bytes", "kib", "milliseconds", "duration", "microseconds", "percent", "cores"])

export function unit(base: string, rate: boolean | undefined, perSecond = "/s"): string {
  return rate === true ? `${base}${perSecond}` : base
}

function Cell({ at, cell, field, kind = "text", locale, rate, t, valueScale }: { readonly at: number | null; readonly cell: Cell; readonly field: string; readonly kind?: EntityColumn["kind"]; readonly locale: Locale; readonly rate?: boolean | undefined; readonly t: Translate; readonly valueScale?: number | null | undefined }) {
  const time = useDisplayTime()
  if (cell === null) return <span className="text-fg-null">—</span>
  if (kind === "timestamp") {
    const timestamp = asNumber(cell)
    if (timestamp === null) return "—"
    const exact = time.timestamp(timestamp)
    return <time className="entity-value block w-full overflow-hidden text-ellipsis whitespace-nowrap" title={exact}>{at === null ? exact : humanDuration((at - timestamp) / 1_000, locale)}</time>
  }
  if (kind === "estimated_rows") return <EstimatedRows cell={cell} locale={locale} t={t} />
  const output = cellAriaValue(cell, { field, kind, ...(rate === undefined ? {} : { rate }), ...(valueScale === undefined ? {} : { valueScale }) }, locale, t)
  const className = `entity-value block w-full overflow-hidden text-ellipsis whitespace-nowrap${kind === "id" ? " text-fg2" : kind === "text" ? " text-fg" : ""}`
  return <span className={className} title={kind === "bytes" || kind === "text" ? output : undefined}>{output}</span>
}

export function cellAriaValue(cell: Cell, field: Pick<EntityColumn, "field" | "kind" | "rate" | "valueScale">, locale: Locale, t: Translate): string {
  const per = t("unit.per_second")
  if (cell === null) return "—"
  const number = asNumber(cell)
  const scaled = field.valueScale === null || field.valueScale !== undefined && number === null
    ? null
    : field.valueScale === undefined ? cell : number! * field.valueScale
  if (field.kind === "id") return identifier(cell)
  if (field.kind === "bytes") return humanBytes(scaled, locale, field.field.endsWith("_per_call") || field.field === "blocks_per_call" ? t("unit.per_call") : field.rate === true ? per : "")
  if (field.kind === "kib") return unit(humanBytes(asNumber(cell) === null ? null : asNumber(cell)! * 1024, locale), field.rate, per)
  if (field.kind === "milliseconds" || field.kind === "duration") return humanDuration(cell, locale, "milliseconds", durationSuffix(field, t))
  if (field.kind === "microseconds") return humanDuration(cell, locale, "microseconds", durationSuffix(field, t))
  if (field.kind === "percent") return humanPercent(cell, locale, field.rate === true ? per : "")
  if (field.kind === "cores") return humanCores(cell, locale, field.rate === true ? per : "")
  if (field.kind === "boolean") return cell === true ? locale === "ru" ? "да" : "true" : cell === false ? locale === "ru" ? "нет" : "false" : rawText(cell) ?? "—"
  if (field.kind === "estimated_rows") return estimatedRows(cell, locale, t)?.primary ?? "—"
  if (field.kind === "number") return measure(cell, locale, unit("", field.rate, per))
  return rawText(cell) ?? "—"
}

function durationSuffix(field: Pick<EntityColumn, "field" | "rate">, t: Translate): string {
  if (field.field.endsWith("_per_call")) return t("unit.per_call")
  return field.rate === true || field.field.endsWith("_per_second") ? t("unit.per_second") : ""
}

export function EstimatedRows({ cell, locale, t }: { readonly cell: Cell; readonly locale: Locale; readonly t: Translate }) {
  const rows = estimatedRows(cell, locale, t)
  return rows === null ? "—" : <span aria-label={rows.secondary ?? undefined} className="entity-value block w-full overflow-hidden text-ellipsis whitespace-nowrap" title={rows.secondary ?? undefined}>{rows.primary}</span>
}

function sortable(cell: Cell, kind: EntityColumn["kind"]): string | number | boolean | null {
  if (kind === "text") return rawText(cell)
  if (kind === "boolean") return typeof cell === "boolean" ? cell : rawText(cell)
  return asNumber(cell) ?? rawText(cell)
}

export function sticky(meta: unknown, head: boolean): string {
  const cell = meta as { readonly sticky?: boolean | string; readonly numeric?: boolean } | undefined
  // Shared box first, then the head/body difference. The names stay as hooks
  // for the per-table overrides that have not moved onto markup yet.
  // Shared box first, then the head/body difference, then what the process
  // table asks of both. The names stay as hooks for the tests.
  const box = "flex-none min-w-0 overflow-hidden border-b border-line px-[7px] [.process-table_&]:px-2"
  const pinned = cell?.sticky === true || typeof cell?.sticky === "string"
  const name = typeof cell?.sticky === "string" ? cell.sticky : ""
  return [
    box,
    head
      ? `entity-header-cell flex items-center font-sans text-xs font-medium text-fg3${pinned ? " bg-s2 z-40" : " relative"}`
      : "entity-cell flex h-row items-center font-mono text-xs tabular-nums text-fg2 [.process-table_&]:text-sm",
    cell?.numeric === true ? "align-right" : "",
    pinned ? "entity-sticky sticky left-0 z-[12] bg-inherit" : "",
    name,
    // The pinned command column casts a shadow over the scrolled body; the pid
    // column sits behind it and must not.
    name === "sticky-command" ? "shadow-[5px_0_8px_var(--color-shadow-b)] max-[760px]:static max-[760px]:shadow-none" : "",
  ].filter(Boolean).join(" ")
}

export function stickyOffsets(columns: readonly { readonly id: string; readonly size: number; readonly sticky: boolean }[]): ReadonlyMap<string, number> {
  const offsets = new Map<string, number>()
  let left = 0
  for (const column of columns) {
    if (!column.sticky) continue
    offsets.set(column.id, left)
    left += column.size
  }
  return offsets
}

function isSticky(meta: unknown): boolean {
  const value = meta as { readonly sticky?: boolean | string } | undefined
  return value?.sticky !== undefined && value.sticky !== false
}

function columnHelp(meta: unknown): { readonly help: string; readonly label: string } | null {
  const value = meta as { readonly help?: string; readonly label?: string } | undefined
  return value?.help === undefined || value.label === undefined ? null : { help: value.help, label: value.label }
}

function defaultKey(row: DataRow): string {
  return `${row.segmentId}:${row.typeId}:${row.ordinal}`
}
