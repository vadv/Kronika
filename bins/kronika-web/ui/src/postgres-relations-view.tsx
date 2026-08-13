import { Copy, X } from "lucide-react"
import { useEffect, useMemo, useState } from "react"

import { loadSeries, loadSnapshot, type DataRow, type HourData } from "./api"
import { EntityTable, type EntityColumn, type TableOrder } from "./entity-table"
import type { Translate } from "./help"
import { rawText, value, type Locale } from "./model"
import {
  INDEX_LENSES,
  TABLE_LENSES,
  isRelationId,
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
  const rateFields = data.rateColumns[section] ?? []
  const columns = useMemo(() => relationColumns(section, lens, level, rateFields), [lens, level, rateFields, section])
  const activeOrder = order !== undefined && columns.some(({ field, sortable }) => field === order.column && sortable === true)
    ? order
    : { column: relationDefaultOrder(section, lens), descending: true }
  const metadata = data.snapshotRows.find((stored) => stored.logicalName === section && stored.group === level)
  const selectedKey = props.selectedKey
  const selected = rows.find((row) => relationRowKey(row) === selectedKey) ?? null
  const select = (row: DataRow) => {
    props.onSelectedKey(relationRowKey(row))
  }
  const clearSelection = () => {
    props.onSelectedKey(null)
  }
  const navigate = (next: RelationNavigation) => {
    clearSelection()
    onNavigate(next)
  }
  const hasMore = metadata?.hasMore === true && metadata.nextCursor !== null
  const status = <>{tableState(metadata, rows.length, cursor, pattern, activeOrder, locale, t)}<span>{relationScope(filters, rows, t)}</span>{lens === "low_activity" && <span>{t("pg.relation.activity_note")}</span>}</>
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
      {selected !== null && <RelationDetail hour={hour} key={selectedKey} lens={lens} locale={locale} onClose={clearSelection} onNavigate={navigate} rateFields={rateFields} row={selected} t={t} />}
    </div>
    {(densePageState !== "idle" || hasMore) && <div className="lens-tabs" data-testid="table-paging"><button disabled={densePageState === "loading"} onClick={densePageState === "error" ? onRetry : onLoadMore} type="button">{densePageState === "loading" ? "…" : densePageState === "error" ? "↻" : "+"}</button></div>}
  </>
}

export function relationDataRows(rows: readonly DataRow[], section: RelationSection, level: RelationGroup): readonly DataRow[] {
  return rows.filter((row) => row.logicalName === section && row.relation?.group === level)
}

export function relationColumns(section: RelationSection, lens: RelationLens, level: RelationGroup, rateFields: readonly string[] = []): readonly EntityColumn[] {
  const request = relationRequest(section, lens, level)
  return visibleRelationFields(section, lens, level).map((field, index) => ({
    ...relationColumn(field, rateFields), sticky: index === 0, sortable: Object.hasOwn(request.order ?? {}, field),
  }))
}

export function relationDetailColumns(section: RelationSection, lens: RelationLens, level: RelationGroup, rateFields: readonly string[] = []): readonly EntityColumn[] {
  return visibleRelationFields(section, lens, level).map((field) => relationColumn(field, rateFields))
}

function RelationLevels({ filters, level, onNavigate, section, t }: { readonly filters: Readonly<Record<string, string>>; readonly level: RelationGroup; readonly onNavigate: (navigation: RelationNavigation) => void; readonly section: RelationSection; readonly t: Translate }) {
  const target = (group: RelationGroup): RelationNavigation => ({ section, group, filters: {}, selectedKey: null })
  return <nav className="lensbar">
    <div className="lens-tabs">{(["object", "schema", "database"] as const).map((stored) => <button aria-pressed={stored === level} key={stored} onClick={() => { if (stored !== level || Object.keys(filters).length !== 0) onNavigate(target(stored)) }} type="button">{stored === "object" ? t(section === "pg_stat_user_tables" ? "pg.section.tables" : "pg.section.indexes") : t(`pg.relation.level.${stored}`)}</button>)}</div>
    {Object.keys(filters).length !== 0 && <button onClick={() => onNavigate(target("object"))}>{t("pg.relation.scope.all")}</button>}
  </nav>
}

function RelationLenses({ active, onLens, section, t }: { readonly active: RelationLens; readonly onLens: (lens: RelationLens) => void; readonly section: RelationSection; readonly t: Translate }) {
  const lenses: readonly RelationLens[] = section === "pg_stat_user_tables" ? TABLE_LENSES : INDEX_LENSES
  return <div className="lensbar pg-lensbar" data-testid="pg-relation-lenses"><span>{t("pg.lens.label")}</span><div aria-label={t("pg.lens.label")} className="lens-tabs" role="group">{lenses.map((lens) => <button aria-pressed={lens === active} key={lens} onClick={() => onLens(lens)} type="button">{t(`pg.lens.${lens}`)}</button>)}</div></div>
}

function RelationDetail({ hour, lens, locale, onClose, onNavigate, rateFields, row, t }: { readonly hour: number; readonly lens: RelationLens; readonly locale: Locale; readonly onClose: () => void; readonly onNavigate: (navigation: RelationNavigation) => void; readonly rateFields: readonly string[]; readonly row: DataRow; readonly t: Translate }) {
  const object = row.relation?.group === "object"
  const definitionTarget = useMemo(() => object && row.logicalName === "pg_stat_user_indexes" ? relationDetailTarget(row) : null, [object, row])
  const [exact, setExact] = useState<DataRow | null>()
  const historyField = relationHistoryField(row.logicalName as RelationSection, lens)
  const [history, setHistory] = useState<ReturnType<typeof relationHistory>>([])
  useEffect(() => {
    setExact(definitionTarget === null ? null : undefined)
    setHistory([])
    const controller = new AbortController()
    if (definitionTarget !== null) {
      void loadSnapshot(row.segmentId, definitionTarget.at, [definitionTarget.request], controller.signal, undefined, definitionTarget.options)
        .then((data) => { if (!controller.signal.aborted) setExact(data.sections[row.logicalName]?.[0] ?? null) })
        .catch(() => { if (!controller.signal.aborted) setExact(null) })
    }
    if (object) {
      void loadSeries(hour, row.logicalName, historyFilters(row), [historyField], controller.signal, undefined, row.timestamp)
        .then((rows) => { if (!controller.signal.aborted) setHistory(relationHistory(rows, historyField)) })
        .catch(() => {})
    }
    return () => controller.abort()
  }, [definitionTarget, historyField, hour, object, row])
  const columns = relationDetailColumns(row.logicalName as RelationSection, lens, row.relation!.group, rateFields)
  const definition = object && row.logicalName === "pg_stat_user_indexes" && exact !== undefined && exact !== null ? rawText(value(exact, "indexdef")) : null
  const titleField = row.relation?.group === "database"
    ? "datname"
    : row.relation?.group === "schema" ? "schemaname" : row.logicalName === "pg_stat_user_tables" ? "relname" : "indexrelname"
  const linked = linkedRelation(row)
  const historyColumn = relationColumn(historyField, rateFields)
  const drill = relationDrill(row)
  return <aside className="pg-detail" data-testid="pg-relation-detail">
    <header><div><span>{t(row.logicalName === "pg_stat_user_tables" ? "pg.section.tables" : "pg.section.indexes")}</span><h2>{rawText(row.values[titleField] ?? null) ?? "—"}</h2></div><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    {linked !== null && <div className="lens-tabs"><button data-testid="pg-relation-link" onClick={() => onNavigate(linked)} type="button">{t(row.logicalName === "pg_stat_user_tables" ? "pg.relation.indexes" : "pg.relation.table")}</button></div>}
    {drill !== null && <div className="lens-tabs"><button data-testid="pg-relation-drill" onClick={() => onNavigate(drill)} type="button">{t(row.relation?.group === "database" ? "pg.relation.level.schema" : row.logicalName === "pg_stat_user_tables" ? "pg.section.tables" : "pg.section.indexes")}</button></div>}
    {object && row.logicalName === "pg_stat_user_indexes" && <section className="query-block"><span>{t("pg.relation.definition")}{definition !== null && <button aria-label={t("common.raw")} className="copy-raw" onClick={() => void navigator.clipboard?.writeText(definition)} type="button"><Copy aria-hidden="true" size={12} /></button>}</span><pre data-testid="pg-exact-indexdef">{exact === undefined ? t("status.loading") : definition ?? t("common.unavailable")}</pre></section>}
    <dl>{columns.map((column) => <div key={column.field}><dt>{t(column.label)}</dt><dd>{display(value(row, column.field), column, locale, t)}</dd></div>)}</dl>
    {object && <SeriesChart cursor={row.timestamp} format={chartFormat(historyColumn.kind)} hour={hour} label={t(historyColumn.label)} locale={locale} points={history} />}
  </aside>
}

function relationScope(filters: Readonly<Record<string, string>>, rows: readonly DataRow[], t: Translate): string {
  const values = rows[0]?.values
  const scope = [
    filters.datid === undefined ? null : rawText(values?.datname ?? null),
    filters.schemaname ?? null,
    filters.relid === undefined ? null : rawText(values?.relname ?? null),
    filters.indexrelid === undefined ? null : rawText(values?.indexrelname ?? null),
  ].filter((name): name is string => name !== null && name !== "").join(" · ")
  return scope !== "" || Object.keys(filters).length !== 0 ? scope : t("pg.relation.scope.all")
}

function visibleRelationFields(section: RelationSection, lens: RelationLens, level: RelationGroup): readonly string[] {
  return relationFields(section, lens, level).filter((field) => !isRelationId(field))
}

function relationColumn(field: string, rateFields: readonly string[] = []): EntityColumn {
  const kind = relationFieldKind(field)
  const width = kind === "timestamp" ? 210 : kind === "text" ? field.includes("relname") ? 190 : 145 : kind === "boolean" || kind === "id" ? 115 : kind === "milliseconds" ? 155 : 140
  return { field, label: `pg.field.${field}.label`, kind, width, rate: rateFields.includes(field) }
}

function historyFilters(row: DataRow): Readonly<Record<string, string>> {
  const object = row.logicalName === "pg_stat_user_tables" ? "relid" : "indexrelid"
  return { datid: rawText(row.values.datid ?? null) ?? "", [object]: rawText(row.values[object] ?? null) ?? "" }
}

function relationRowLabel(row: DataRow): string {
  const name = row.logicalName === "pg_stat_user_tables" ? "relname" : "indexrelname"
  return rawText(row.values[name] ?? row.values.schemaname ?? row.values.datname ?? null) ?? relationRowKey(row)
}
