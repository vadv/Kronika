import { registry } from "kronika:registry"
import { Copy, X } from "lucide-react"
import { useEffect, useMemo, useState } from "react"

import { acceptResponse, loadSeries, loadSnapshot, type DataRow, type HourData } from "./api"
import { useDisplayTime } from "./display-time-context"
import { EntityTable, type EntityColumn, type TableOrder } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
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
  relationDisplayFields,
  relationFieldKind,
  relationFields,
  relationHelpKey,
  relationHistory,
  relationHistoryField,
  relationHistoryFields,
  relationRequest,
  relationRowKey,
  type RelationGroup,
  type RelationLens,
  type RelationNavigation,
  type RelationSection,
} from "./postgres-relations"
import { emptyHourStatusKey } from "./refresh"
import { SeriesChart } from "./series-chart"
import { chartFormat, chartPointValue, chartScale, chartUnit, chartableColumn, display, tableState } from "./postgres-view"

export interface PostgresRelationsViewProps {
  readonly cursor: number
  readonly data: HourData
  readonly densePageState: "idle" | "loading" | "error"
  readonly filters: Readonly<Record<string, string>>
  readonly hour: number
  readonly lens: RelationLens
  readonly level: RelationGroup
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
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
  const time = useDisplayTime()
  const { cursor, data, densePageState, filters, hour, level, locale, onCursor, onLens, onLoadMore, onNavigate, onOrder, onPattern, onRetry, order, pattern, section, t } = props
  const lens = isRelationLens(section, props.lens) ? props.lens : section === "pg_stat_user_tables" ? "access" : "usage"
  const rows = useMemo(() => relationDataRows(data.sections[section] ?? [], section, level), [data.sections, level, section])
  const rateFields = data.rateColumns[section] ?? []
  const columns = useMemo(() => relationColumns(section, lens, level, rateFields, t), [lens, level, rateFields, section, t])
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
  const status = <>{tableState(metadata, rows.length, cursor, pattern, activeOrder, locale, t, time)}<span>{relationScope(filters, rows, t)}</span>{lens === "low_activity" && <span>{t("pg.relation.activity_note")}</span>}<span>{t("system.history")}</span></>
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
      {selected !== null && <RelationDetail cursor={cursor} hour={hour} key={selectedKey} lens={lens} locale={locale} onClose={clearSelection} onCursor={onCursor} onNavigate={navigate} rateFields={rateFields} row={selected} t={t} />}
    </div>
    {(densePageState !== "idle" || hasMore) && <div className="lens-tabs" data-testid="table-paging"><button disabled={densePageState === "loading"} onClick={densePageState === "error" ? onRetry : onLoadMore} type="button">{densePageState === "loading" ? "…" : densePageState === "error" ? "↻" : "+"}</button></div>}
  </>
}

export function relationDataRows(rows: readonly DataRow[], section: RelationSection, level: RelationGroup): readonly DataRow[] {
  return rows.filter((row) => row.logicalName === section && row.relation?.group === level)
}

export function relationColumns(section: RelationSection, lens: RelationLens, level: RelationGroup, rateFields: readonly string[] = [], t?: Translate): readonly EntityColumn[] {
  const request = relationRequest(section, lens, level)
  return visibleRelationFields(section, lens, level).map((field, index) => ({
    ...relationColumn(section, field, rateFields),
    ...(t !== undefined && (field === "last_seq_scan" || field === "last_idx_scan") ? { renderNull: (row: DataRow) => value(row, `${field}_never`) === true ? t("common.never") : t("common.unavailable") } : {}),
    sticky: index === 0, sortable: Object.hasOwn(request.order ?? {}, field),
  }))
}

export function relationDetailColumns(section: RelationSection, lens: RelationLens, level: RelationGroup, rateFields: readonly string[] = []): readonly EntityColumn[] {
  const identity = level === "database" ? 2 : level === "schema" ? 3 : section === "pg_stat_user_tables" ? 6 : lens === "state" ? 7 : 9
  return relationFields(section, lens, level).slice(identity).filter((field) => !field.endsWith("_never") && field !== "state_severity").map((field) => relationColumn(section, field, rateFields))
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

function RelationDetail({ cursor, hour, lens, locale, onClose, onCursor, onNavigate, rateFields, row, t }: { readonly cursor: number; readonly hour: number; readonly lens: RelationLens; readonly locale: Locale; readonly onClose: () => void; readonly onCursor: (timestamp: number) => void; readonly onNavigate: (navigation: RelationNavigation) => void; readonly rateFields: readonly string[]; readonly row: DataRow; readonly t: Translate }) {
  const group = row.relation?.group ?? "object"
  const object = group === "object"
  const definitionTarget = useMemo(() => object && row.logicalName === "pg_stat_user_indexes" ? relationDetailTarget(row) : null, [object, row])
  const [exact, setExact] = useState<DataRow | null>()
  const initialHistoryField = relationHistoryField(row.logicalName as RelationSection, lens)
  const allColumns = useMemo(() => relationDetailColumns(row.logicalName as RelationSection, lens, group, rateFields), [group, lens, rateFields, row.logicalName])
  const columns = useMemo(() => allColumns.filter((column) => value(row, column.field) !== null || value(row, `${column.field}_never`) === true), [allColumns, row])
  const physicalFields = useMemo(() => registry.find(({ typeId }) => typeId === row.typeId)?.columns ?? [], [row.typeId])
  const chartColumns = useMemo(() => allColumns.filter((column) => relationChartableColumn(row.logicalName as RelationSection, column, physicalFields, group)), [allColumns, group, physicalFields, row.logicalName])
  const preferredField = chartColumns.some(({ field }) => field === initialHistoryField) ? initialHistoryField : chartColumns[0]?.field ?? null
  const chartFields = chartColumns.map(({ field }) => field).join("\u0000")
  const historyFields = useMemo(() => relationHistoryRequestFields(row.logicalName as RelationSection, group, chartColumns, physicalFields), [chartFields, group, physicalFields, row.logicalName])
  const historyFilters = useMemo(() => relationHistoryFilters(row), [row])
  const [historyField, setHistoryField] = useState(preferredField)
  const [historyRows, setHistoryRows] = useState<readonly DataRow[]>([])
  useEffect(() => {
    setHistoryField((current) => current !== null && chartFields.split("\u0000").includes(current) ? current : preferredField)
  }, [chartFields, preferredField])
  useEffect(() => {
    setExact(definitionTarget === null ? null : undefined)
    const controller = new AbortController()
    if (definitionTarget !== null) {
      acceptResponse(loadSnapshot(row.segmentId, definitionTarget.at, [definitionTarget.request], controller.signal, undefined, definitionTarget.options), controller.signal,
        (data) => setExact(data.sections[row.logicalName]?.[0] ?? null), () => setExact(null))
    }
    return () => controller.abort()
  }, [definitionTarget, row])
  useEffect(() => {
    setHistoryRows([])
    if (historyFields.length === 0) return
    const controller = new AbortController()
    acceptResponse(loadSeries(hour, row.logicalName, historyFilters, historyFields, controller.signal, object ? row.typeId : undefined, row.timestamp, object ? undefined : group), controller.signal, setHistoryRows)
    return () => controller.abort()
  }, [group, historyFields, historyFilters, hour, object, row.logicalName, row.timestamp, row.typeId])
  const definition = exact ? rawText(value(exact, "indexdef")) : null
  const linked = linkedRelation(row)
  const historyColumn = chartColumns.find(({ field }) => field === historyField)
  const history = useMemo(() => historyColumn === undefined ? [] : relationMetricHistory(historyRows, historyColumn, group), [group, historyColumn, historyRows])
  const drill = relationDrill(row)
  return <aside className="pg-detail" data-testid="pg-relation-detail">
    <header><h2>{relationRowLabel(row)}</h2><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    {linked !== null && <div className="lens-tabs"><button data-testid="pg-relation-link" onClick={() => onNavigate(linked)} type="button">{t(row.logicalName === "pg_stat_user_tables" ? "pg.relation.indexes" : "pg.relation.table")}</button></div>}
    {drill !== null && <div className="lens-tabs"><button data-testid="pg-relation-drill" onClick={() => onNavigate(drill)} type="button">{t(row.relation?.group === "database" ? "pg.relation.level.schema" : row.logicalName === "pg_stat_user_tables" ? "pg.section.tables" : "pg.section.indexes")}</button></div>}
    {historyField !== null && historyColumn !== undefined && <section className="process-history pg-metric-history">
      <div aria-label={t("system.history")} className="process-history-selector" role="group">{chartColumns.map((column) => <button aria-pressed={historyField === column.field} data-testid={`pg-relation-chart-${column.field}`} key={column.field} onClick={() => setHistoryField(column.field)} type="button">{t(column.label)}</button>)}</div>
      <SeriesChart cursor={cursor} format={chartFormat(historyColumn.kind)} hour={hour} label={t(historyColumn.label)} locale={locale} onCursor={onCursor} points={history} scale={chartScale(historyColumn)} unit={chartUnit(historyColumn, t("unit.per_second"))} />
    </section>}
    <dl>{columns.map((column) => {
      const label = t(column.label)
      return <div key={column.field}><dt><span>{column.help === undefined ? label : <LabelHelp helpKey={column.help} labelKey={column.label} t={t} />}</span></dt><dd>{scanValue(row, column, locale, t)}</dd></div>
    })}</dl>
    {definitionTarget !== null && <section className="query-block"><span>{t("pg.relation.definition")}{definition !== null && <button aria-label={t("common.raw")} className="copy-raw" onClick={() => void navigator.clipboard?.writeText(definition)} type="button"><Copy aria-hidden="true" size={12} /></button>}</span><pre data-testid="pg-exact-indexdef">{exact === undefined ? t("status.loading") : definition ?? t("common.unavailable")}</pre></section>}
  </aside>
}

export function relationChartableColumn(section: RelationSection, column: EntityColumn, physicalFields: readonly string[], group: RelationGroup = "object"): boolean {
  return chartableColumn(column) && (group !== "object" || relationHistoryFields(section, column.field, physicalFields).length > 0)
}

export function relationHistoryRequestFields(section: RelationSection, group: RelationGroup, columns: readonly EntityColumn[], physicalFields: readonly string[]): readonly string[] {
  return [...new Set(columns.flatMap((column) => group === "object"
    ? relationHistoryFields(section, column.field, physicalFields)
    : [column.field]))]
}

export function relationMetricHistory(rows: readonly DataRow[], column: EntityColumn, group: RelationGroup): ReturnType<typeof relationHistory> {
  const points = group === "object"
    ? relationHistory(rows, column.field)
    : [...rows].sort((left, right) => left.timestamp - right.timestamp).map((row) => ({ segmentId: row.segmentId, timestamp: row.timestamp, value: value(row, column.field) }))
  return points.map((point) => ({ ...point, value: point.value === null ? null : chartPointValue(point.value, column) }))
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
  return relationDisplayFields(section, lens, level).filter((field) => !isRelationId(field)
    && !field.endsWith("_never")
    && field !== "state_severity"
    && !rawRelationField(section, lens, field))
}

function relationColumn(section: RelationSection, field: string, rateFields: readonly string[] = []): EntityColumn {
  const kind = relationFieldKind(field)
  const width = kind === "timestamp" ? 210 : kind === "text" ? field.includes("relname") ? 190 : 145 : kind === "boolean" || kind === "id" ? 115 : kind === "milliseconds" ? 155 : 140
  const copyField = section === "pg_stat_user_tables" && field === "main_fork_bytes" ? "table_data_bytes" : field
  const help = relationHelpKey(section, field)
  return { field, label: `pg.field.${copyField}.label`, ...(help === undefined ? {} : { help }), kind, width, rate: rateFields.includes(field) }
}

function rawRelationField(section: RelationSection, lens: RelationLens, field: string): boolean {
  return lens === "size_buffers" && (field.endsWith("_blks_read") || field.endsWith("_blks_hit")
    || section === "pg_stat_user_tables" && ["toast_bytes", "toast_n_live_tup", "toast_n_dead_tup"].includes(field))
}

function scanValue(row: DataRow, column: EntityColumn, locale: Locale, t: Translate) {
  if (column.kind === "timestamp" && value(row, column.field) === null && value(row, `${column.field}_never`) === true) return t("common.never")
  return display(value(row, column.field), column, locale, t)
}

export function relationHistoryFilters(row: DataRow): Readonly<Record<string, string>> {
  const datid = rawText(row.values.datid ?? null) ?? ""
  if (row.relation?.group === "database") return { datid }
  if (row.relation?.group === "schema") return { datid, schemaname: rawText(row.values.schemaname ?? null) ?? "" }
  const object = row.logicalName === "pg_stat_user_tables" ? "relid" : "indexrelid"
  return { datid, [object]: rawText(row.values[object] ?? null) ?? "" }
}

function relationRowLabel(row: DataRow): string {
  const name = row.logicalName === "pg_stat_user_tables" ? "relname" : "indexrelname"
  return ["datname", "schemaname", name].flatMap((field) => rawText(row.values[field] ?? null) ?? []).join(".") || relationRowKey(row)
}
