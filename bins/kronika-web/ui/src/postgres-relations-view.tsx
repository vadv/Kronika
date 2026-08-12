import { Copy, X } from "lucide-react"
import { useEffect, useMemo, useState, type ReactNode } from "react"

import { loadSeries, loadSnapshot, type DataRow, type HourData, type SnapshotRows } from "./api"
import { EntityTable, type EntityColumn, type TableOrder } from "./entity-table"
import type { Translate } from "./help"
import { formatUtc, humanBytes, measure, rawText, value, type Locale } from "./model"
import {
  INDEX_LENSES,
  TABLE_LENSES,
  isRelationLens,
  linkedRelation,
  relationDefaultOrder,
  relationDetailTarget,
  relationDrill,
  relationFields,
  relationHistory,
  relationHistoryField,
  relationRequest,
  relationRowKey,
  type RelationGroup,
  type RelationLens,
  type RelationNavigation,
  type RelationRow,
  type RelationSection,
} from "./postgres-relations"
import { emptyHourStatusKey } from "./refresh"
import { SeriesChart } from "./series-chart"

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
  readonly onSelectedKey?: ((key: string | null) => void) | undefined
  readonly order?: TableOrder | undefined
  readonly pattern: string
  readonly section: RelationSection
  readonly selectedKey?: string | null | undefined
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
  const [localKey, setLocalKey] = useState<string | null>(null)
  const selectedKey = props.selectedKey === undefined ? localKey : props.selectedKey
  const selected = rows.find((row) => row.relation !== undefined && relationRowKey(row.relation) === selectedKey) ?? null
  const select = (row: DataRow) => {
    const relation = row.relation
    if (relation === undefined) return
    const drill = relationDrill(relation)
    if (drill !== null) {
      setLocalKey(null)
      props.onSelectedKey?.(null)
      onNavigate(drill)
      return
    }
    const key = relationRowKey(relation)
    setLocalKey(key)
    props.onSelectedKey?.(key)
  }
  const clearSelection = () => {
    setLocalKey(null)
    props.onSelectedKey?.(null)
  }
  const navigate = (next: RelationNavigation) => {
    clearSelection()
    onNavigate(next)
  }
  const hasMore = metadata?.hasMore === true && metadata.nextCursor !== null
  const status = relationStatus(metadata, rows.length, cursor, level, filters, pattern, activeOrder, locale, lens, densePageState, t)
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
        rowKey={relationAdapterKey}
        rowLabel={relationRowLabel}
        rows={rows}
        selectedKey={selectedKey}
        serverSorted
        status={status}
        t={t}
        testId={section === "pg_stat_user_tables" ? "pg-tables-table" : "pg-indexes-table"}
      />
      {selected?.relation !== undefined && <RelationDetail hour={hour} key={selectedKey} lens={lens} locale={locale} onClose={clearSelection} onNavigate={navigate} row={selected.relation} t={t} />}
    </div>
    {(densePageState !== "idle" || hasMore) && <div className="lens-tabs" data-testid="table-paging"><button disabled={densePageState === "loading"} onClick={densePageState === "error" ? onRetry : onLoadMore} type="button">{densePageState === "loading" ? "…" : densePageState === "error" ? "↻" : "+"}</button></div>}
  </>
}

export function relationDataRows(rows: readonly DataRow[], section: RelationSection, level: RelationGroup): readonly DataRow[] {
  return rows.filter((row) => row.relation?.logicalName === section && row.relation.group === level)
}

export function relationColumns(section: RelationSection, lens: RelationLens, level: RelationGroup): readonly EntityColumn[] {
  const request = relationRequest(section, lens, level)
  return relationFields(section, lens, level).map((field, index) => ({
    field,
    label: `pg.field.${field}.label`,
    kind: fieldKind(field),
    width: fieldWidth(field),
    sticky: index === 0,
    rate: rates.has(field),
    sortable: Object.hasOwn(request.order ?? {}, field),
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

function RelationDetail({ hour, lens, locale, onClose, onNavigate, row, t }: { readonly hour: number; readonly lens: RelationLens; readonly locale: Locale; readonly onClose: () => void; readonly onNavigate: (navigation: RelationNavigation) => void; readonly row: RelationRow; readonly t: Translate }) {
  const target = useMemo(() => relationDetailTarget(row), [row])
  const [exact, setExact] = useState<DataRow | null>(null)
  const [exactPending, setExactPending] = useState(true)
  const historyField = relationHistoryField(row.logicalName, lens)
  const [history, setHistory] = useState<ReturnType<typeof relationHistory>>([])
  useEffect(() => {
    setExact(null)
    setExactPending(true)
    const controller = new AbortController()
    void loadSnapshot(target.segmentId, target.at, [target.request], controller.signal, undefined, target.options)
      .then((data) => { if (!controller.signal.aborted) setExact(data.sections[row.logicalName]?.[0] ?? null) })
      .catch(() => {})
      .finally(() => { if (!controller.signal.aborted) setExactPending(false) })
    return () => controller.abort()
  }, [row.logicalName, target])
  useEffect(() => {
    setHistory([])
    const controller = new AbortController()
    void loadSeries(hour, row.logicalName, historyFilters(row), [historyField], controller.signal, undefined, target.at)
      .then((rows) => { if (!controller.signal.aborted) setHistory(relationHistory(rows, historyField)) })
      .catch(() => {})
    return () => controller.abort()
  }, [historyField, hour, row, target.at])
  const shown = exact
  const fields = shown === null ? [] : Object.keys(shown.values)
  const definition = row.logicalName === "pg_stat_user_indexes" && shown !== null ? rawText(value(shown, "indexdef")) : null
  const titleField = row.logicalName === "pg_stat_user_tables" ? "relname" : "indexrelname"
  const linked = linkedRelation(row)
  const historyColumn = relationColumn(historyField)
  return <aside className="pg-detail" data-testid="pg-relation-detail">
    <header><div><span>{t(row.logicalName === "pg_stat_user_tables" ? "pg.section.tables" : "pg.section.indexes")}</span><h2>{rawText(row.values[titleField] ?? null) ?? "—"}</h2></div><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    {linked !== null && <div className="lens-tabs"><button data-testid="pg-relation-link" onClick={() => onNavigate(linked)} type="button">{t(row.logicalName === "pg_stat_user_tables" ? "pg.relation.indexes" : "pg.relation.table")}</button></div>}
    {row.logicalName === "pg_stat_user_indexes" && <section className="query-block"><span>{t("pg.relation.definition")}{definition !== null && <button aria-label={t("common.raw")} className="copy-raw" onClick={() => void navigator.clipboard?.writeText(definition)} type="button"><Copy aria-hidden="true" size={12} /></button>}</span><pre data-testid="pg-exact-indexdef">{exactPending ? t("status.loading") : definition ?? t("common.unavailable")}</pre></section>}
    <dl>{fields.filter((field) => field !== "indexdef").map((field) => {
      const column = relationColumn(field)
      return <div key={field}><dt>{t(column.label)}</dt><dd>{shown === null ? t("common.unavailable") : display(value(shown, field), { ...column, rate: false }, locale, t)}</dd></div>
    })}</dl>
    <SeriesChart cursor={target.at} format={chartFormat(historyColumn.kind)} hour={hour} label={t(historyColumn.label)} locale={locale} points={history} />
  </aside>
}

function relationStatus(metadata: SnapshotRows | undefined, loaded: number, cursor: number, level: RelationGroup, filters: Readonly<Record<string, string>>, pattern: string, order: TableOrder, locale: Locale, lens: RelationLens, pageState: "idle" | "loading" | "error", t: Translate): ReactNode {
  const count = (value: number) => new Intl.NumberFormat(locale).format(value)
  const scope = [
    filters.datid === undefined ? null : t("pg.relation.scope.database", { oid: filters.datid }),
    filters.schemaname === undefined ? null : t("pg.relation.scope.schema", { schema: filters.schemaname }),
    filters.relid === undefined ? null : t("pg.relation.scope.table", { oid: filters.relid }),
    filters.indexrelid === undefined ? null : t("pg.relation.scope.index", { oid: filters.indexrelid }),
  ].filter((label): label is string => label !== null).join(" · ")
  const shown = pageState === "loading"
    ? t("pg.relation.loading")
    : pageState === "error"
      ? t("pg.relation.load_failed")
      : t("pg.table.shown", { returned: count(loaded), eligible: count(metadata?.eligible ?? loaded) })
  return <>
    <span>{t(`pg.relation.level.${level}`)}</span>
    <span>{t("pg.table.cursor", { time: formatUtc(cursor) })}</span>
    <span>{metadata?.from === null || metadata?.from === undefined || metadata.to === null ? t("pg.table.interval_unavailable") : t("pg.table.interval", { from: formatUtc(metadata.from), to: formatUtc(metadata.to) })}</span>
    <span>{scope === "" ? t("pg.relation.scope.all") : scope}</span>
    <span>{pattern.trim() === "" ? t("pg.relation.search.none") : t("pg.relation.search.active", { pattern: pattern.trim() })}</span>
    <span>{t("pg.relation.order", { semantic: t(`pg.field.${order.column}.label`), direction: t(`pg.table.${metadata?.orderDirection ?? (order.descending ? "desc" : "asc")}`) })}</span>
    {lens === "low_activity" && <span>{t("pg.relation.activity_note")}</span>}
    <strong>{shown}</strong>
  </>
}

function relationColumn(field: string): EntityColumn {
  return { field, label: `pg.field.${field}.label`, kind: fieldKind(field), width: fieldWidth(field), rate: rates.has(field) }
}

function fieldKind(field: string): NonNullable<EntityColumn["kind"]> {
  if (["datid", "relid", "indexrelid"].includes(field)) return "id"
  if (texts.has(field)) return "text"
  if (booleans.has(field)) return "boolean"
  if (field.endsWith("_oldest") || field.endsWith("_latest") || timestamps.has(field)) return "timestamp"
  if (field.endsWith("_bytes")) return "bytes"
  if (field.endsWith("_mean_ms") || field.startsWith("total_") && field.endsWith("_time")) return "milliseconds"
  if (field.endsWith("_pct")) return "percent"
  return "number"
}

function fieldWidth(field: string): number {
  const kind = fieldKind(field)
  if (kind === "timestamp") return 210
  if (kind === "text") return field.includes("relname") ? 190 : 145
  if (kind === "boolean" || kind === "id") return 115
  return kind === "milliseconds" ? 155 : 140
}

function display(cell: ReturnType<typeof value>, column: EntityColumn, locale: Locale, t: Translate): string {
  if (cell === null) return "—"
  if (column.kind === "id" || column.kind === "text") return rawText(cell) ?? "—"
  if (column.kind === "timestamp") return formatUtc(Number(rawText(cell)))
  if (column.kind === "boolean" && typeof cell === "boolean") return locale === "ru" ? cell ? "да" : "нет" : String(cell)
  const per = column.rate === true ? t("unit.per_second") : ""
  if (column.kind === "bytes") return humanBytes(cell, locale, per)
  if (column.kind === "milliseconds") return measure(cell, locale, ` ${t("unit.ms")}${per}`)
  if (column.kind === "percent") return measure(cell, locale, "%")
  return measure(cell, locale, per)
}

function chartFormat(kind: EntityColumn["kind"]): ((stored: number, locale: Locale) => string) | undefined {
  return kind === "bytes" ? humanBytes : kind === "milliseconds" ? (stored, locale) => measure(stored, locale, " ms") : undefined
}

function historyFilters(row: RelationRow): Readonly<Record<string, string>> {
  const object = row.logicalName === "pg_stat_user_tables" ? "relid" : "indexrelid"
  return { datid: row.key.datid ?? "", [object]: row.key[object] ?? "" }
}

function relationAdapterKey(row: DataRow): string {
  return row.relation === undefined ? "" : relationRowKey(row.relation)
}

function relationRowLabel(row: DataRow): string {
  const relation = row.relation
  if (relation === undefined) return ""
  const name = relation.logicalName === "pg_stat_user_tables" ? "relname" : "indexrelname"
  return rawText(relation.values[name] ?? relation.values.schemaname ?? relation.values.datname ?? null) ?? relationRowKey(relation)
}

function pick(values: Readonly<Record<string, string>>, ...names: readonly string[]): Readonly<Record<string, string>> {
  return Object.fromEntries(names.flatMap((name) => values[name] === undefined ? [] : [[name, values[name]]]))
}

const texts = new Set(["datname", "schemaname", "relname", "indexrelname", "tablespace", "amname", "indexdef"])
const booleans = new Set(["no_scans", "indisvalid", "indisready", "indisunique", "indisprimary", "indisexclusion"])
const timestamps = new Set(["last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "last_seq_scan", "last_idx_scan", "toast_last_autovacuum"])
const rates = new Set([
  "seq_scan", "seq_tup_read", "idx_scan", "idx_tup_fetch", "idx_tup_read", "n_tup_ins", "n_tup_upd", "n_tup_del", "n_tup_hot_upd", "n_tup_newpage_upd",
  "vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit",
])
