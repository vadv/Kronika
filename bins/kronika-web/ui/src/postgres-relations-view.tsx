import { Copy, X } from "lucide-react"
import { useEffect, useMemo, useState } from "react"

import { loadSeries, loadSnapshot, type DataRow, type HourData } from "./api"
import { EntityTable, type EntityColumn, type TableOrder } from "./entity-table"
import type { Translate } from "./help"
import { rawText, value, type Locale } from "./model"
import {
  INDEX_LENSES,
  TABLE_LENSES,
  isRelationLens,
  linkedRelation,
  relationDefaultOrder,
  relationDetailTarget,
  relationDrill,
  relationFieldKind,
  relationFields,
  relationHistory,
  relationHistoryField,
  relationRequest,
  relationRowKey,
  type RelationGroup,
  type RelationLens,
  type RelationNavigation,
  type RelationSection,
} from "./postgres-relations"
import { emptyHourStatusKey } from "./refresh"
import { SeriesChart } from "./series-chart"
import { chartFormat, display, tableState } from "./postgres-view"

export interface PostgresRelationsViewProps {
  readonly cursor: number
  readonly data: HourData
  readonly densePageState: "idle" | "loading" | "error"
  readonly filters: Readonly<Record<string, string>>
  readonly hour: number
  readonly lens: RelationLens
  readonly level: RelationGroup
  readonly locale: Locale
  readonly onLens: (lens: RelationLens) => void
  readonly onLoadMore: () => void
  readonly onNavigate: (navigation: RelationNavigation) => void
  readonly onOrder: (order: TableOrder | null) => void
  readonly onPattern: (pattern: string) => void
  readonly onRetry: () => void
  readonly onSelectedKey: (key: string | null) => void
  readonly order?: TableOrder | undefined
  readonly pattern: string
  readonly section: RelationSection
  readonly selectedKey: string | null
  readonly t: Translate
}

export function PostgresRelationsView(props: PostgresRelationsViewProps) {
  const { cursor, data, densePageState, filters, hour, level, locale, onLens, onLoadMore, onNavigate, onOrder, onPattern, onRetry, order, pattern, section, t } = props
  const lens = isRelationLens(section, props.lens) ? props.lens : section === "pg_stat_user_tables" ? "access" : "usage"
  const rows = useMemo(() => relationDataRows(data.sections[section] ?? [], section, level), [data.sections, level, section])
  const columns = useMemo(() => relationColumns(section, lens, level), [lens, level, section])
  const activeOrder = order !== undefined && columns.some(({ field, sortable }) => field === order.column && sortable === true)
    ? order
    : { column: relationDefaultOrder(section, lens), descending: true }
  const metadata = data.snapshotRows.find((stored) => stored.logicalName === section && stored.group === level)
  const selectedKey = props.selectedKey
  const selected = rows.find((row) => relationRowKey(row) === selectedKey) ?? null
  const select = (row: DataRow) => {
    const drill = relationDrill(row)
    if (drill !== null) {
      props.onSelectedKey(null)
      onNavigate(drill)
      return
    }
    const key = relationRowKey(row)
    props.onSelectedKey(key)
  }
  const clearSelection = () => {
    props.onSelectedKey(null)
  }
  const navigate = (next: RelationNavigation) => {
    clearSelection()
    onNavigate(next)
  }
  const hasMore = metadata?.hasMore === true && metadata.nextCursor !== null
  const status = <>{tableState(metadata, rows.length, cursor, pattern, activeOrder, locale, t)}<span>{relationScope(filters, t)}</span>{lens === "low_activity" && <span>{t("pg.relation.activity_note")}</span>}</>
  return <>
    <RelationLevels filters={filters} level={level} onNavigate={navigate} section={section} t={t} />
    <RelationLenses active={lens} onLens={onLens} section={section} t={t} />
    <div className={selected === null ? "pg-entity-layout pg-table-only" : "pg-entity-layout"} data-pg-section={section === "pg_stat_user_tables" ? "tables" : "indexes"}>
      <EntityTable
        columns={columns}
        empty={t(emptyHourStatusKey(hour))}
        label={t(section === "pg_stat_user_tables" ? "pg.section.tables" : "pg.section.indexes")}
        locale={locale}
        onNearEnd={densePageState === "idle" && hasMore ? onLoadMore : undefined}
        onOrder={onOrder}
        onPattern={onPattern}
        onSelect={select}
        order={activeOrder}
        pattern={pattern}
        rowKey={relationRowKey}
        rowLabel={relationRowLabel}
        rows={rows}
        selectedKey={selectedKey}
        serverSorted
        status={status}
        t={t}
        testId={section === "pg_stat_user_tables" ? "pg-tables-table" : "pg-indexes-table"}
      />
      {selected !== null && <RelationDetail hour={hour} key={selectedKey} lens={lens} locale={locale} onClose={clearSelection} onNavigate={navigate} row={selected} t={t} />}
    </div>
    {(densePageState !== "idle" || hasMore) && <div className="lens-tabs" data-testid="table-paging"><button disabled={densePageState === "loading"} onClick={densePageState === "error" ? onRetry : onLoadMore} type="button">{densePageState === "loading" ? "…" : densePageState === "error" ? "↻" : "+"}</button></div>}
  </>
}

export function relationDataRows(rows: readonly DataRow[], section: RelationSection, level: RelationGroup): readonly DataRow[] {
  return rows.filter((row) => row.logicalName === section && row.relation?.group === level)
}

export function relationColumns(section: RelationSection, lens: RelationLens, level: RelationGroup): readonly EntityColumn[] {
  const request = relationRequest(section, lens, level)
  return relationFields(section, lens, level).map((field, index) => ({
    ...relationColumn(field), sticky: index === 0, sortable: Object.hasOwn(request.order ?? {}, field),
  }))
}

function RelationLevels({ filters, level, onNavigate, section, t }: { readonly filters: Readonly<Record<string, string>>; readonly level: RelationGroup; readonly onNavigate: (navigation: RelationNavigation) => void; readonly section: RelationSection; readonly t: Translate }) {
  const levels: readonly RelationGroup[] = level === "database" ? ["database"] : level === "schema" ? ["database", "schema"] : ["database", "schema", "object"]
  const target = (next: RelationGroup): RelationNavigation => ({
    section,
    group: next,
    filters: next === "database" ? {} : next === "schema" ? pick(filters, "datid") : pick(filters, "datid", "schemaname"),
    selectedKey: null,
  })
  return <nav aria-label={t("pg.relation.up")} className="lensbar" data-testid="pg-relation-levels"><div className="lens-tabs">{levels.map((stored) => <button aria-pressed={stored === level} key={stored} onClick={() => { if (stored !== level) onNavigate(target(stored)) }} type="button">{t(`pg.relation.level.${stored}`)}</button>)}</div></nav>
}

function RelationLenses({ active, onLens, section, t }: { readonly active: RelationLens; readonly onLens: (lens: RelationLens) => void; readonly section: RelationSection; readonly t: Translate }) {
  const lenses: readonly RelationLens[] = section === "pg_stat_user_tables" ? TABLE_LENSES : INDEX_LENSES
  return <div className="lensbar pg-lensbar" data-testid="pg-relation-lenses"><span>{t("pg.lens.label")}</span><div aria-label={t("pg.lens.label")} className="lens-tabs" role="group">{lenses.map((lens) => <button aria-pressed={lens === active} key={lens} onClick={() => onLens(lens)} type="button">{t(`pg.lens.${lens}`)}</button>)}</div></div>
}

function RelationDetail({ hour, lens, locale, onClose, onNavigate, row, t }: { readonly hour: number; readonly lens: RelationLens; readonly locale: Locale; readonly onClose: () => void; readonly onNavigate: (navigation: RelationNavigation) => void; readonly row: DataRow; readonly t: Translate }) {
  const target = useMemo(() => relationDetailTarget(row), [row])
  const [exact, setExact] = useState<DataRow | null>()
  const historyField = relationHistoryField(row.logicalName as RelationSection, lens)
  const [history, setHistory] = useState<ReturnType<typeof relationHistory>>([])
  useEffect(() => {
    setExact(undefined)
    setHistory([])
    const controller = new AbortController()
    void loadSnapshot(row.segmentId, target.at, [target.request], controller.signal, undefined, target.options)
      .then((data) => { if (!controller.signal.aborted) setExact(data.sections[row.logicalName]?.[0] ?? null) })
      .catch(() => { if (!controller.signal.aborted) setExact(null) })
    void loadSeries(hour, row.logicalName, historyFilters(row), [historyField], controller.signal, undefined, target.at)
      .then((rows) => { if (!controller.signal.aborted) setHistory(relationHistory(rows, historyField)) })
      .catch(() => {})
    return () => controller.abort()
  }, [historyField, hour, row, target])
  const fields = exact === undefined || exact === null ? [] : Object.keys(exact.values)
  const definition = row.logicalName === "pg_stat_user_indexes" && exact !== undefined && exact !== null ? rawText(value(exact, "indexdef")) : null
  const titleField = row.logicalName === "pg_stat_user_tables" ? "relname" : "indexrelname"
  const linked = linkedRelation(row)
  const historyColumn = relationColumn(historyField)
  return <aside className="pg-detail" data-testid="pg-relation-detail">
    <header><div><span>{t(row.logicalName === "pg_stat_user_tables" ? "pg.section.tables" : "pg.section.indexes")}</span><h2>{rawText(row.values[titleField] ?? null) ?? "—"}</h2></div><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    {linked !== null && <div className="lens-tabs"><button data-testid="pg-relation-link" onClick={() => onNavigate(linked)} type="button">{t(row.logicalName === "pg_stat_user_tables" ? "pg.relation.indexes" : "pg.relation.table")}</button></div>}
    {row.logicalName === "pg_stat_user_indexes" && <section className="query-block"><span>{t("pg.relation.definition")}{definition !== null && <button aria-label={t("common.raw")} className="copy-raw" onClick={() => void navigator.clipboard?.writeText(definition)} type="button"><Copy aria-hidden="true" size={12} /></button>}</span><pre data-testid="pg-exact-indexdef">{exact === undefined ? t("status.loading") : definition ?? t("common.unavailable")}</pre></section>}
    <dl>{fields.filter((field) => field !== "indexdef").map((field) => {
      const column = relationColumn(field)
      return <div key={field}><dt>{t(column.label)}</dt><dd>{display(value(exact!, field), { ...column, rate: false }, locale, t)}</dd></div>
    })}</dl>
    <SeriesChart cursor={target.at} format={chartFormat(historyColumn.kind)} hour={hour} label={t(historyColumn.label)} locale={locale} points={history} />
  </aside>
}

function relationScope(filters: Readonly<Record<string, string>>, t: Translate): string {
  const scope = [
    filters.datid === undefined ? null : t("pg.relation.scope.database", { oid: filters.datid }),
    filters.schemaname === undefined ? null : t("pg.relation.scope.schema", { schema: filters.schemaname }),
    filters.relid === undefined ? null : t("pg.relation.scope.table", { oid: filters.relid }),
    filters.indexrelid === undefined ? null : t("pg.relation.scope.index", { oid: filters.indexrelid }),
  ].filter((label): label is string => label !== null).join(" · ")
  return scope === "" ? t("pg.relation.scope.all") : scope
}

function relationColumn(field: string): EntityColumn {
  const kind = relationFieldKind(field)
  const width = kind === "timestamp" ? 210 : kind === "text" ? field.includes("relname") ? 190 : 145 : kind === "boolean" || kind === "id" ? 115 : kind === "milliseconds" ? 155 : 140
  return { field, label: `pg.field.${field}.label`, kind, width, rate: relationRate(field) }
}

function historyFilters(row: DataRow): Readonly<Record<string, string>> {
  const object = row.logicalName === "pg_stat_user_tables" ? "relid" : "indexrelid"
  return { datid: rawText(row.values.datid ?? null) ?? "", [object]: rawText(row.values[object] ?? null) ?? "" }
}

function relationRowLabel(row: DataRow): string {
  const name = row.logicalName === "pg_stat_user_tables" ? "relname" : "indexrelname"
  return rawText(row.values[name] ?? row.values.schemaname ?? row.values.datname ?? null) ?? relationRowKey(row)
}

function pick(values: Readonly<Record<string, string>>, ...names: readonly string[]): Readonly<Record<string, string>> {
  return Object.fromEntries(names.flatMap((name) => values[name] === undefined ? [] : [[name, values[name]]]))
}

function relationRate(field: string): boolean {
  return !field.startsWith("last_") && !field.endsWith("_pct") && !field.endsWith("_bytes")
    && (field.includes("scan") || field.includes("tup_") || field.includes("blks_") || field.endsWith("_count"))
}
