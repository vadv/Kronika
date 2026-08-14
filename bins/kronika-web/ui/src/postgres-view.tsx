import { Copy, X } from "lucide-react"
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react"
import { registry } from "kronika:registry"

import type { DataRow, Finding, HourData, SnapshotRows } from "./api"
import { buildMetricSamples } from "./chart"
import { ChartOnly } from "./chart-visibility"
import { contextMatches, contextualRows, type EntityContext } from "./entity-context"
import { createDisplayTimeFormatter, type DisplayTimeFormatter } from "./display-time"
import { useDisplayTime } from "./display-time-context"
import { EntityTable, EstimatedRows, unit, type EntityColumn, type TableOrder } from "./entity-table"
import type { Translate } from "./help"
import { acceptResponse, fieldNameForLocator, loadSeries, loadSnapshot } from "./api"
import { LabelHelp } from "./help"
import { asNumber, compact, humanBytes, humanDuration, humanPercent, identifier, measure, rawText, snapshot, value, type Locale, shownMoment } from "./model"
import { activityDurationHistory, activityDurationSource, activityDurationMs, decorateActivityRow, transactionDurationMs } from "./postgres-activity"
import { decoratePostgresIntervalRow, findingSemanticField, intervalMetric, PG_STAT_STATEMENTS_TYPE_IDS, PG_STORE_PLANS_TYPE_IDS, physicalField, physicalFields, planDefaultOrder, postgresHistory, postgresIdentity, statementDefaultOrder, unique, type PlanLens, type PostgresSemanticField, type StatementLens } from "./postgres-metrics"
import { PostgresRelationsView } from "./postgres-relations-view"
import type { RelationGroup, RelationLens, RelationNavigation } from "./postgres-relations"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

export type PostgresSection = "overview" | "activity" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes"

export const ACTIVITY_DEFAULT_ORDER: TableOrder = { column: "query_duration_ms", descending: true }

const ACTIVITY_PID = pgId("pid", "pg.field.pid", 78, true, false)
const ACTIVITY_BACKEND_TYPE = pgText("backend_type", "pg.backend_type", 150, true)

export const ACTIVITY_COLUMNS: readonly EntityColumn[] = [
  ACTIVITY_PID, pgText("datname", "pg.datname", 145, false, false), pgText("usename", "pg.usename", 130, false, false), pgText("query", "pg.query", 420),
  duration("query_duration_ms", 145), duration("transaction_duration_ms", 155),
  pgText("application_name", "pg.application_name", 180, false, false), pgText("client_addr", "pg.client_addr", 150),
  pgText("state", "pg.state", 140), pgText("wait_event_type", "pg.wait_event_type", 135), pgText("wait_event", "pg.wait_event", 155),
]

export const ACTIVITY_DETAIL_COLUMNS: readonly EntityColumn[] = [
  ...ACTIVITY_COLUMNS, ACTIVITY_BACKEND_TYPE, pgId("leader_pid", "pg.leader_pid", 105), pgId("query_id", "pg.query_id", 150),
  pgNumber("backend_xmin_age", "pg.backend_xmin_age", 145), pgNumber("backend_xid_age", "pg.backend_xid_age", 145),
  duration("backend_age_ms", 145), duration("state_duration_ms", 155),
]

export { activityDurationHistory, activityDurationMs, transactionDurationMs }

export function activityColumns(showSystem: boolean): readonly EntityColumn[] {
  return showSystem ? [ACTIVITY_PID, ACTIVITY_BACKEND_TYPE, ...ACTIVITY_COLUMNS.slice(1)] : ACTIVITY_COLUMNS
}

export const STATEMENT_COLUMNS: readonly EntityColumn[] = [
  rateNumber("calls_per_second", 120),
  rateMilliseconds("execution_ms_per_second", 165),
  { ...milliseconds("mean_exec_ms_per_call", 170), physicalField: physicalFields(PG_STAT_STATEMENTS_TYPE_IDS, "mean_exec_ms_per_call") },
  text("query", 440, true), rateNumber("rows_per_second", 120),
  rateNumber("shared_blks_hit", 145), rateNumber("shared_blks_read", 145), rateNumber("shared_blks_dirtied", 155), rateNumber("shared_blks_written", 155),
  rateNumber("local_blks_hit", 145), rateNumber("local_blks_read", 145), rateNumber("local_blks_dirtied", 155), rateNumber("local_blks_written", 155),
  rateNumber("temp_blks_read", 145), rateNumber("temp_blks_written", 155), rateBytes("wal_bytes", 145), rateNumber("wal_records", 135),
  rateNumber("wal_fpi", 160), rateNumber("wal_buffers_full", 165),
  rateMilliseconds("shared_blk_read_ms_per_second", 165), rateMilliseconds("shared_blk_write_ms_per_second", 170),
  rateMilliseconds("local_blk_read_ms_per_second", 160), rateMilliseconds("local_blk_write_ms_per_second", 165),
  rateMilliseconds("temp_blk_read_ms_per_second", 160), rateMilliseconds("temp_blk_write_ms_per_second", 165),
  rateNumber("plans"), rateMilliseconds("planning_ms_per_second", 165),
  { ...text("datname", 145), help: "pg.field.statement_database.help" }, text("usename", 130), id("queryid", 155),
  boolean("toplevel", 105), timestamp("stats_since", 210),
]

const STATEMENT_DERIVED_COLUMNS: readonly EntityColumn[] = [
  number("rows_per_call", 130), number("blocks_per_call", 140),
  percent("hit_pct", 115), bytes("wal_per_call", 135), percent("plan_time_pct", 130),
  number("cv", 90), milliseconds("min_exec_time_ms", 125), milliseconds("max_exec_time_ms", 125),
  milliseconds("mean_exec_time_ms", 135), milliseconds("stddev_exec_time_ms", 145),
]

export const STATEMENT_LENSES: Readonly<Record<StatementLens, readonly string[]>> = {
  load: ["query", "datname", "usename", "queryid", "toplevel", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"],
  per_call: ["query", "datname", "usename", "queryid", "toplevel", "mean_exec_ms_per_call", "rows_per_call", "blocks_per_call", "calls_per_second"],
  io: ["query", "datname", "usename", "queryid", "toplevel", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "shared_blks_written", "local_blks_read", "temp_blks_read", "temp_blks_written"],
  resources: ["query", "datname", "usename", "queryid", "toplevel", "wal_bytes", "wal_per_call", "temp_blks_written", "planning_ms_per_second", "plan_time_pct", "calls_per_second", "execution_ms_per_second"],
  stability: ["query", "datname", "usename", "queryid", "toplevel", "cv", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second"],
}

export const PLAN_COLUMNS: readonly EntityColumn[] = [
  rateNumber("calls_per_second", 120),
  rateMilliseconds("execution_ms_per_second", 165),
  { ...milliseconds("mean_exec_ms_per_call", 170), physicalField: physicalFields(PG_STORE_PLANS_TYPE_IDS, "mean_exec_ms_per_call") },
  text("plan", 440, true), rateNumber("rows_per_second", 120),
  id("planid", 155), id("queryid", 155),
  rateNumber("shared_blks_hit", 145), rateNumber("shared_blks_read", 145), rateNumber("shared_blks_dirtied", 155), rateNumber("shared_blks_written", 155),
  rateNumber("local_blks_hit", 145), rateNumber("local_blks_read", 145), rateNumber("local_blks_dirtied", 155), rateNumber("local_blks_written", 155),
  rateNumber("temp_blks_read", 145), rateNumber("temp_blks_written", 155),
  rateMilliseconds("shared_blk_read_ms_per_second", 165), rateMilliseconds("shared_blk_write_ms_per_second", 170),
  rateMilliseconds("local_blk_read_ms_per_second", 160), rateMilliseconds("local_blk_write_ms_per_second", 165),
  rateMilliseconds("temp_blk_read_ms_per_second", 160), rateMilliseconds("temp_blk_write_ms_per_second", 165),
  rateMilliseconds("planning_ms_per_second", 165), rateNumber("slow_log_calls", 145),
  { ...text("datname", 145), help: "pg.field.plan_database.help" }, text("usename", 130),
  text("cmd_type", 125), id("queryid_stat_statements", 220),
]

const PLAN_DERIVED_COLUMNS: readonly EntityColumn[] = [
  number("rows_per_call", 130), number("blocks_per_call", 140), percent("hit_pct", 115),
  percent("plan_time_pct", 130), milliseconds("min_exec_time_ms", 125), milliseconds("max_exec_time_ms", 125),
  milliseconds("mean_exec_time_ms", 135), milliseconds("stddev_exec_time_ms", 145),
  timestamp("first_call", 190), timestamp("last_call", 190),
]

export const PLAN_LENSES: Readonly<Record<PlanLens, readonly string[]>> = {
  load: ["plan", "datname", "usename", "queryid", "planid", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"],
  timing: ["plan", "datname", "usename", "queryid", "planid", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second", "first_call", "last_call"],
  io: ["plan", "datname", "usename", "queryid", "planid", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "local_blks_read", "temp_blks_read"],
  identity: ["plan", "datname", "usename", "queryid", "planid", "cmd_type", "queryid_stat_statements", "calls_per_second"],
}

const STATEMENT_ALL_COLUMNS = [...STATEMENT_COLUMNS, ...STATEMENT_DERIVED_COLUMNS]
const PLAN_ALL_COLUMNS = [...PLAN_COLUMNS, ...PLAN_DERIVED_COLUMNS]

export function statementColumns(lens: StatementLens): readonly EntityColumn[] {
  return columnsInOrder(STATEMENT_ALL_COLUMNS, STATEMENT_LENSES[lens])
}

export function planColumns(lens: PlanLens): readonly EntityColumn[] {
  return columnsInOrder(PLAN_ALL_COLUMNS, PLAN_LENSES[lens])
}

function columnsInOrder(columns: readonly EntityColumn[], fields: readonly string[]): readonly EntityColumn[] {
  const byField = new Map(columns.map((column) => [column.field, column]))
  return fields.flatMap((field) => {
    const column = byField.get(field)
    return column === undefined ? [] : [{ ...column, sticky: field === "query" || field === "plan" }]
  })
}

export const LOCK_COLUMNS: readonly EntityColumn[] = [
  id("pid", 78, true, false), pgText("datname", "pg.datname", 145, true, false), pgText("usename", "pg.usename", 130, false, false), pgText("query", "pg.query", 420), pgText("application_name", "pg.application_name", 180, false, false),
  text("lock_target", 260), text("lock_relname", 180), text("lock_locktype", 145), text("lock_mode", 180), text("blocked_by", 150),
  pgText("state", "pg.state", 110), pgText("wait_event_type", "pg.wait_event_type", 135), pgText("wait_event", "pg.wait_event", 155), timestamp("waitstart", 210),
]

export const DATABASE_COLUMNS: readonly EntityColumn[] = [
  text("datname", 170, true, false), number("numbackends", 135), number("xact_commit", 145), number("xact_rollback", 145), number("sessions", 125),
  number("tup_returned", 145), number("tup_fetched", 145), number("tup_inserted", 145), number("tup_updated", 145), number("tup_deleted", 145),
  number("blks_read", 140), number("blks_hit", 140), milliseconds("blk_read_time", 150), milliseconds("blk_write_time", 155),
  number("temp_files", 125), bytes("temp_bytes", 145), number("conflicts", 125), number("deadlocks", 125), number("frozen_xid_age", 155),
]

const TABS: readonly { readonly id: PostgresSection; readonly sections?: readonly string[] }[] = [
  { id: "overview" },
  { id: "activity", sections: ["pg_stat_activity", "pg_stat_progress_vacuum"] },
  { id: "statements", sections: ["pg_stat_statements"] },
  { id: "plans", sections: ["pg_store_plans", "pg_store_plans_info"] },
  { id: "locks", sections: ["pg_locks"] },
  { id: "databases", sections: ["pg_stat_database"] },
  { id: "tables", sections: ["pg_stat_user_tables"] },
  { id: "indexes", sections: ["pg_stat_user_indexes"] },
]

export function PostgresView({
  context,
  densePageState,
  onLoadMore,
  onRetry,
  onPattern,
  pattern,
  onOrder,
  order,
  cursor,
  data,
  focus,
  focusFinding,
  hour,
  locale,
  onCursor,
  onContextClear,
  onFinding,
  onPlanLens,
  onRelationLens,
  onRelationNavigate,
  onRelationSelectedKey,
  onSection,
  onStatementLens,
  planLens,
  relationFilters,
  relationLens,
  relationLevel,
  relationSelectedKey,
  section,
  statementLens,
  t,
}: {
  readonly context: EntityContext | null
  readonly densePageState: "idle" | "loading" | "error"
  readonly onLoadMore: () => void
  readonly onRetry: () => void
  readonly onOrder: (order: TableOrder | null) => void
  readonly onPattern: (pattern: string) => void
  readonly order: TableOrder | undefined
  readonly pattern: string
  readonly cursor: number
  readonly data: HourData
  readonly focus: DataRow | null
  readonly focusFinding: Finding | null
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onContextClear: () => void
  readonly onFinding: (finding: Finding) => void
  readonly onSection: (section: PostgresSection) => void
  readonly onRelationLens: (lens: RelationLens) => void
  readonly onRelationNavigate: (navigation: RelationNavigation) => void
  readonly onRelationSelectedKey: (key: string | null) => void
  readonly section: PostgresSection
  readonly statementLens: StatementLens
  readonly planLens: PlanLens
  readonly onStatementLens: (lens: StatementLens) => void
  readonly onPlanLens: (lens: PlanLens) => void
  readonly relationFilters: Readonly<Record<string, string>>
  readonly relationLens: RelationLens
  readonly relationLevel: RelationGroup
  readonly relationSelectedKey: string | null
  readonly t: Translate
}) {
  const available = (name: string) => data.availableSections.includes(name)
  useEffect(() => {
    const tab = TABS.find((candidate) => candidate.id === section)
    if (tab === undefined || tab.id === "plans" || tab.id === "tables" || tab.id === "indexes" || tab.sections === undefined || tab.sections.some(available)) return
    onSection("overview")
  }, [data.availableSections, onSection, section])
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  return <>
    <ChartOnly><Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={onCursor} onFinding={onFinding} primaryLane={section === "statements" || section === "plans" ? "pg_running" : section === "activity" || section === "locks" ? "pg_waiting" : "health"} shownAt={shownAt} t={t} /></ChartOnly>
    <nav aria-label={t("pg.sections")} className="pg-tabs">
      {TABS.map((tab) => {
        const enabled = tab.id === "plans" || tab.id === "tables" || tab.id === "indexes" || tab.sections === undefined || tab.sections.some(available)
        return <button aria-current={section === tab.id ? "page" : undefined} disabled={!enabled} key={tab.id} onClick={() => { if (section !== tab.id) onOrder(null); onSection(tab.id) }} title={enabled ? undefined : t("pg.no_section_data")} type="button"><span>{t(`pg.section.${tab.id}`)}</span></button>
      })}
    </nav>
    {section === "overview" && <Overview cursor={cursor} data={data} hour={hour} locale={locale} onCursor={onCursor} t={t} />}
    {section === "activity" && available("pg_stat_activity") && <ActivityView context={context} onContextClear={onContextClear} onCursor={onCursor} onOrder={onOrder} order={order} onPattern={onPattern} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_activity" ? focusFinding : null} focus={focus} locale={locale} t={t} />}
    {section === "activity" && available("pg_stat_progress_vacuum") && <PgPreview cursor={cursor} data={data} focus={focusFinding?.logicalName === "pg_stat_progress_vacuum" ? focus : null} hour={hour} locale={locale} onCursor={onCursor} section="pg_stat_progress_vacuum" t={t} />}
    {section === "statements" && <><PostgresLensBar active={statementLens} choices={["load", "per_call", "io", "resources", "stability"]} onChange={onStatementLens} prefix="statement" t={t} /><PgEntityView columns={statementColumns(statementLens)} context={context} defaultOrder={{ column: statementDefaultOrder(statementLens), descending: true }} densePageState={densePageState} onContextClear={onContextClear} onCursor={onCursor} onLoadMore={onLoadMore} onRetry={onRetry} onOrder={onOrder} onPattern={onPattern} pattern={pattern} order={order} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_statements" ? focusFinding : null} focus={focus} historyField={statementLens === "stability" ? "cv" : "mean_exec_ms_per_call"} locale={locale} section="pg_stat_statements" t={t} /></>}
    {section === "plans" && available("pg_store_plans_info") && <PlanInfo cursor={cursor} data={data} hour={hour} locale={locale} onCursor={onCursor} t={t} />}
    {section === "plans" && <PostgresLensBar active={planLens} choices={["load", "timing", "io", "identity"]} onChange={onPlanLens} prefix="plan" t={t} />}
    {section === "plans" && available("pg_store_plans") && <PgEntityView columns={planColumns(planLens)} context={context} defaultOrder={{ column: planDefaultOrder(planLens), descending: true }} densePageState={densePageState} onContextClear={onContextClear} onCursor={onCursor} onLoadMore={onLoadMore} onRetry={onRetry} onOrder={onOrder} onPattern={onPattern} pattern={pattern} order={order} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_store_plans" ? focusFinding : null} focus={focus} historyField="mean_exec_ms_per_call" locale={locale} section="pg_store_plans" t={t} />}
    {section === "plans" && !available("pg_store_plans") && <p className="pg-empty" data-testid="pg-plans-empty">{t("pg.plans.empty")}</p>}
    {section === "locks" && <PgEntityView columns={LOCK_COLUMNS} context={context} onContextClear={onContextClear} onCursor={onCursor} onOrder={onOrder} order={order} onPattern={onPattern} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_locks" ? focusFinding : null} focus={focus} historyField={null} locale={locale} section="pg_locks" t={t} />}
    {section === "databases" && <PgEntityView columns={DATABASE_COLUMNS} context={context} onContextClear={onContextClear} onCursor={onCursor} onOrder={onOrder} order={order} onPattern={onPattern} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_database" ? focusFinding : null} focus={focus} historyField="xact_commit" locale={locale} section="pg_stat_database" t={t} />}
    {section === "tables" && <PostgresRelationsView cursor={cursor} data={data} densePageState={densePageState} filters={relationFilters} hour={hour} lens={relationLens} level={relationLevel} locale={locale} onCursor={onCursor} onLens={onRelationLens} onLoadMore={onLoadMore} onNavigate={onRelationNavigate} onOrder={onOrder} onPattern={onPattern} onRetry={onRetry} onSelectedKey={onRelationSelectedKey} order={order} pattern={pattern} section="pg_stat_user_tables" selectedKey={relationSelectedKey} t={t} />}
    {section === "indexes" && <PostgresRelationsView cursor={cursor} data={data} densePageState={densePageState} filters={relationFilters} hour={hour} lens={relationLens} level={relationLevel} locale={locale} onCursor={onCursor} onLens={onRelationLens} onLoadMore={onLoadMore} onNavigate={onRelationNavigate} onOrder={onOrder} onPattern={onPattern} onRetry={onRetry} onSelectedKey={onRelationSelectedKey} order={order} pattern={pattern} section="pg_stat_user_indexes" selectedKey={relationSelectedKey} t={t} />}
  </>
}

export interface ActivityVisibility {
  readonly showIdle: boolean
  readonly showSystem: boolean
}

export function isSystemActivity(row: DataRow): boolean {
  const backendType = rawText(value(row, "backend_type"))
  return backendType !== null && backendType !== "" && backendType !== "client backend"
}

export function isIdleActivity(row: DataRow): boolean {
  return rawText(value(row, "state")) === "idle"
}

export function overviewBackendCounts(rows: readonly DataRow[]): { readonly active: number; readonly idle: number; readonly total: number } {
  const clients = rows.filter((row) => !isSystemActivity(row))
  const active = clients.filter((row) => rawText(value(row, "state")) === "active").length
  return { active, idle: clients.length - active, total: clients.length }
}

export function visibleActivityRows(
  rows: readonly DataRow[],
  visibility: ActivityVisibility,
  focus: DataRow | null = null,
): readonly DataRow[] {
  const focusKey = focus === null ? null : rowKey(focus)
  return rows
    .filter((row) => rowKey(row) === focusKey
      || (visibility.showSystem || !isSystemActivity(row))
        && (visibility.showIdle || !isIdleActivity(row)))
    .map(decorateActivityRow)
    .sort((left, right) => {
      const leftDuration = asNumber(value(left, "query_duration_ms"))
      const rightDuration = asNumber(value(right, "query_duration_ms"))
      if (leftDuration === null && rightDuration === null) {
        return (asNumber(value(right, "transaction_duration_ms")) ?? -1) - (asNumber(value(left, "transaction_duration_ms")) ?? -1)
      }
      if (leftDuration === null) return 1
      if (rightDuration === null) return -1
      return rightDuration - leftDuration
    })
}

function ActivityView({ context, cursor, data, finding, focus, locale, onContextClear, onCursor, onOrder, onPattern, order, pattern, t }: {
  readonly context: EntityContext | null
  readonly cursor: number
  readonly data: HourData
  readonly finding: Finding | null
  readonly focus: DataRow | null
  readonly locale: Locale
  readonly onContextClear: () => void
  readonly onCursor: (timestamp: number) => void
  readonly onOrder: (order: TableOrder | null) => void
  readonly onPattern: (pattern: string) => void
  readonly order: TableOrder | undefined
  readonly pattern: string
  readonly t: Translate
}) {
  const [showSystem, setShowSystem] = useState(false)
  const [showIdle, setShowIdle] = useState(false)
  const columns = activityColumns(showSystem)
  const activityOrder = order !== undefined && columns.some(({ field }) => field === order.column) ? order : undefined
  const transformRows = useCallback(
    (rows: readonly DataRow[]) => visibleActivityRows(rows, { showIdle, showSystem }, focus),
    [focus, showIdle, showSystem],
  )
  return <>
    <div className="lensbar pg-lensbar" role="group" aria-label={t("pg.section.activity")}>
      <div className="lens-tabs">
        <button aria-pressed={showSystem} data-testid="activity-filter-system" onClick={() => setShowSystem((shown) => !shown)} type="button">{t("pg.activity.system")}</button>
        <button aria-pressed={showIdle} data-testid="activity-filter-idle" onClick={() => setShowIdle((shown) => !shown)} type="button">{t("pg.activity.idle")}</button>
      </div>
    </div>
    <PgEntityView columns={columns} context={context} cursor={cursor} data={data} defaultOrder={ACTIVITY_DEFAULT_ORDER} detailColumns={ACTIVITY_DETAIL_COLUMNS} finding={finding} focus={focus} historyField={null} locale={locale} onContextClear={onContextClear} onCursor={onCursor} onOrder={onOrder} onPattern={onPattern} order={activityOrder} pattern={pattern} section="pg_stat_activity" t={t} transformRows={transformRows} />
  </>
}

function PostgresLensBar<L extends string>({ active, choices, onChange, prefix, t }: { readonly active: L; readonly choices: readonly L[]; readonly onChange: (lens: L) => void; readonly prefix: "statement" | "plan"; readonly t: Translate }) {
  return <div className="lensbar pg-lensbar"><span>{t("pg.lens.label")}</span><div className="lens-tabs" role="group" aria-label={t("pg.lens.label")}>{choices.map((choice) => <button aria-pressed={active === choice} data-testid={`${prefix}-lens-${choice}`} key={choice} onClick={() => onChange(choice)} type="button">{t(`pg.lens.${choice}`)}</button>)}</div><div className="value-tone-legend" aria-label={t("pg.value.legend")}><i className="tone-good" />{t("pg.value.good")}<i className="tone-warning" />{t("pg.value.warning")}<i className="tone-critical" />{t("pg.value.critical")}</div></div>
}

function PgPreview({ columns: prescribedColumns, cursor, data, focus, hour, locale, onCursor, overview = false, section, t }: { readonly columns?: readonly EntityColumn[] | undefined; readonly cursor: number; readonly data: HourData; readonly focus: DataRow | null; readonly hour: number; readonly locale: Locale; readonly onCursor: (timestamp: number) => void; readonly overview?: boolean | undefined; readonly section: string; readonly t: Translate }) {
  const allRows = data.sections[section] ?? NO_ROWS
  const rows = snapshot(allRows, cursor)
  const rates = data.rateColumns[section] ?? NO_RATES
  const columns = useMemo(() => (prescribedColumns ?? (section === "pg_stat_progress_vacuum" ? progressVacuumColumns(rows, rates) : columnsFor(rows))).filter((column) => rows.some((row) => Object.hasOwn(row.values, column.field))).map((column) => ({
    ...column,
    ...(overview && prescribedColumns === undefined ? { help: `${overviewFieldKey(column.field)}.help`, label: overviewFieldKey(column.field) } : {}),
    ...(rates.includes(column.field) ? { rate: true } : {}),
  })), [overview, prescribedColumns, rates, rows, section])
  const [selected, setSelected] = useState<DataRow | null>(null)
  useEffect(() => setSelected((current) => selectedEntity(rows, current, section)), [rows, section])
  const selectedKey = selected === null ? null : rowKey(selected)
  const initialHistory = columns.find(chartableColumn)?.field ?? null
  return <section className="pg-preview">
    <h2>{overview ? t(overviewSectionKey(section)) : section}</h2>
    <div className={selected === null ? "pg-entity-layout pg-table-only" : "pg-entity-layout"}>
      <EntityTable columns={columns} empty={t("table.no_rows")} label={section} locale={locale} onSelect={setSelected} rows={rows} selectedKey={selectedKey ?? (focus === null ? null : rowKey(focus))} status={initialHistory === null ? undefined : <span>{t("system.history")}</span>} t={t} />
      {selected !== null && <PgDetail allRows={allRows} columns={columns} cursor={cursor} historyField={initialHistory} hour={hour} locale={locale} onClose={() => setSelected(null)} onCursor={onCursor} overview={overview} row={selected} section={section} t={t} />}
    </div>
  </section>
}

const PLAN_DEALLOC_COLUMN = rateNumber("dealloc")

function PlanInfo({ cursor, data, hour, locale, onCursor, t }: { readonly cursor: number; readonly data: HourData; readonly hour: number; readonly locale: Locale; readonly onCursor: (timestamp: number) => void; readonly t: Translate }) {
  const row = snapshot(data.sections.pg_store_plans_info ?? [], cursor)[0] ?? null
  const history = usePgMetricHistory(hour, row, "dealloc", PLAN_DEALLOC_COLUMN)
  if (row === null) return null
  const dealloc = value(row, "dealloc")
  const reset = value(row, "stats_reset")
  return <section className="pg-overview-section" data-testid="pg-plans-info">
    <h2>pg_store_plans_info</h2>
    <dl>
      <div><dt>{t("pg.field.dealloc.label")}</dt><dd>{dealloc === null ? "—" : measure(dealloc, locale, t("unit.per_second"))}</dd></div>
      <div><dt>{t("pg.field.stats_reset.label")}</dt><dd>{display(reset, timestamp("stats_reset"), locale, t)}</dd></div>
    </dl>
    <ChartOnly><SeriesChart cursor={cursor} helpKey="pg.field.dealloc.help" hour={hour} labelKey="pg.field.dealloc.label" locale={locale} onCursor={onCursor} points={history ?? []} scale="nonnegative" t={t} unit={t("unit.per_second")} /></ChartOnly>
  </section>
}

function Overview({ cursor, data, hour, locale, onCursor, t }: { readonly cursor: number; readonly data: HourData; readonly hour: number; readonly locale: Locale; readonly onCursor: (timestamp: number) => void; readonly t: Translate }) {
  const activity = snapshot(data.sections.pg_stat_activity ?? [], cursor)
  const databases = snapshot(data.sections.pg_stat_database ?? [], cursor)
  const locks = snapshot(data.sections.pg_locks ?? [], cursor)
  const backends = overviewBackendCounts(activity)
  const databaseCount = postgresDatabaseCount(databases)
  const totals: [string, number][] = []
  if (activity.length !== 0) totals.push(["pg.overview.backends", backends.total], ["pg.overview.active", backends.active], ["pg.overview.idle", backends.idle])
  if (databases.length !== 0) totals.push(["pg.overview.databases", databaseCount])
  if (locks.length !== 0) totals.push(["pg.overview.lock_rows", locks.length])
  const walStorage = snapshot(data.sections.pg_wal_storage ?? [], cursor)[0]
  const overviewSections = groupSections(data.pgOverview.filter(({ logicalName }) => logicalName !== "pg_wal_storage"))
  return <section className="pg-overview">
    <div className="overview-metrics">{totals.map(([label, output]) => <article key={label}><span>{t(label)}</span><strong>{measure(output, locale)}</strong></article>)}</div>
    <OverviewActivityHistory cursor={cursor} data={data} hour={hour} locale={locale} onCursor={onCursor} t={t} />
    {walStorage !== undefined && <WalStorage cursor={cursor} hour={hour} locale={locale} onCursor={onCursor} row={walStorage} t={t} />}
    {overviewSections.map(([logicalName, allRows]) => {
      const rows = snapshot(allRows, cursor)
      if (rows.length === 0) return null
      if (OVERVIEW_SINGLETONS.has(logicalName)) return <OverviewMetrics cursor={cursor} hour={hour} key={logicalName} locale={locale} logicalName={logicalName} onCursor={onCursor} row={rows[0]!} t={t} />
      return <PgPreview cursor={cursor} data={data} focus={null} hour={hour} key={logicalName} locale={locale} onCursor={onCursor} overview section={logicalName} t={t} />
    })}
    {databases.length !== 0 && <PgPreview columns={DATABASE_COLUMNS.slice(0, 9)} cursor={cursor} data={data} focus={null} hour={hour} locale={locale} onCursor={onCursor} overview section="pg_stat_database" t={t} />}
  </section>
}

const OVERVIEW_SINGLETONS = new Set(["pg_stat_bgwriter", "pg_stat_checkpointer", "pg_stat_statements_info"])

function OverviewActivityHistory({ cursor, data, hour, locale, onCursor, t }: { readonly cursor: number; readonly data: HourData; readonly hour: number; readonly locale: Locale; readonly onCursor: (timestamp: number) => void; readonly t: Translate }) {
  const running = data.lanePoints.filter(({ lane }) => lane === "pg_running")
  const waiting = data.lanePoints.filter(({ lane }) => lane === "pg_waiting")
  if (running.length === 0 && waiting.length === 0) return null
  return <ChartOnly><section className="pg-overview-section" data-testid="pg-overview-activity-history">
    <h2>{t("pg.overview.activity_history")}</h2>
    <SeriesChart cursor={cursor} helpKey="lane.pg_running.help" hour={hour} labelKey="pg.overview.running" locale={locale} onCursor={onCursor} points={running} scale="nonnegative" second={waiting} secondHelpKey="lane.pg_waiting.help" secondLabelKey="pg.overview.waiting" t={t} unit="count" />
  </section></ChartOnly>
}

export function walStoragePoints(rows: readonly DataRow[]): readonly ChartPoint[] {
  return buildMetricSamples(rows, (row) => Object.hasOwn(row.values, "wal_files_bytes")
    ? asNumber(value(row, "wal_files_bytes"))
    : undefined)
}

function WalStorage({ cursor, hour, locale, onCursor, row, t }: { readonly cursor: number; readonly hour: number; readonly locale: Locale; readonly onCursor: (timestamp: number) => void; readonly row: DataRow; readonly t: Translate }) {
  const [history, setHistory] = useState<readonly ChartPoint[]>(() => walStoragePoints([row]))
  useEffect(() => {
    const controller = new AbortController()
    acceptResponse(loadSeries(hour, "pg_wal_storage", {}, ["wal_files_bytes"], controller.signal, row.typeId), controller.signal, (rows) => {
      const points = walStoragePoints(rows)
      setHistory(points.length === 0 ? walStoragePoints([row]) : points)
    })
    return () => controller.abort()
  }, [hour, row])
  return <ChartOnly><section className="pg-overview-section" data-testid="pg-wal-storage">
    <h2><LabelHelp helpKey="pg.wal_storage.help" labelKey="pg.wal_storage.label" t={t} /></h2>
    <SeriesChart cursor={cursor} format={humanBytes} helpKey="pg.wal_storage.help" hour={hour} labelKey="pg.wal_storage.history" locale={locale} onCursor={onCursor} points={history} scale="nonnegative" t={t} unit="B" />
  </section></ChartOnly>
}

function OverviewMetrics({ cursor, hour, locale, logicalName, onCursor, row, t }: { readonly cursor: number; readonly hour: number; readonly locale: Locale; readonly logicalName: string; readonly onCursor: (timestamp: number) => void; readonly row: DataRow; readonly t: Translate }) {
  const time = useDisplayTime()
  const chartColumns = useMemo(() => overviewChartColumns(row), [row])
  const preferredField = chartColumns[0]?.field ?? null
  const chartFields = chartColumns.map(({ field }) => field).join("\u0000")
  const [metricField, setMetricField] = useState<string | null>(preferredField)
  useEffect(() => {
    setMetricField((current) => current !== null && chartFields.split("\u0000").includes(current) ? current : preferredField)
  }, [chartFields, preferredField])
  const selectedColumn = chartColumns.find(({ field }) => field === metricField)
  const history = usePgMetricHistory(hour, row, metricField, selectedColumn)
  return <section className="pg-overview-section">
    <h2>{t(overviewSectionKey(logicalName))}</h2>
    <ChartOnly>{selectedColumn !== undefined && <section className="process-history pg-metric-history">
      <div aria-label={t("system.history")} className="process-history-selector" role="group">{chartColumns.map((column) => <button aria-pressed={metricField === column.field} data-testid={`pg-overview-chart-${column.field}`} key={column.field} onClick={() => setMetricField(column.field)} type="button">{t(overviewFieldKey(column.field))}</button>)}</div>
      <SeriesChart cursor={cursor} format={chartFormat(selectedColumn.kind)} helpKey={selectedColumn.help ?? "chart.metric.help"} hour={hour} labelKey={overviewFieldKey(selectedColumn.field)} locale={locale} onCursor={onCursor} points={history ?? []} scale={chartScale(selectedColumn)} t={t} unit={chartUnit(selectedColumn, t("unit.per_second"))} />
    </section>}</ChartOnly>
    <dl>{registryCardFields(row).map(([field, cell]) => <div key={field}><dt><span>{t(overviewFieldKey(field))}</span></dt><dd>{overviewValue(cell, field, locale, time)}</dd></div>)}</dl>
  </section>
}

export function overviewChartColumns(row: DataRow): readonly EntityColumn[] {
  const visible = new Set(registryCardFields(row).map(([field]) => field))
  const metadata = registry.find((layout) => layout.typeId === row.typeId)?.columnMetadata ?? []
  return columnsFor([row]).flatMap((column) => {
    const semantic = metadata.find(({ name }) => name === column.field)
    return visible.has(column.field) && chartableColumn(column)
      && semantic?.class !== "label" && semantic?.class !== "timestamp"
      ? [{ ...column, help: `${overviewFieldKey(column.field)}.help`, ...(semantic?.class === "cumulative" ? { rate: true } : {}) }]
      : []
  })
}

export function postgresMetricHistory(rows: readonly DataRow[], column: EntityColumn, cumulative: boolean, resetField?: string): readonly ChartPoint[] {
  const owned = rows.filter((row) => Object.hasOwn(row.values, column.field))
    .slice().sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  if (!cumulative) return buildMetricSamples(owned, (row) => chartPointValue(value(row, column.field), column))
  return owned.map((row, index) => {
    const earlier = owned[index - 1]
    const reset = resetField !== undefined && earlier !== undefined
      && rawText(value(earlier, resetField)) !== rawText(value(row, resetField))
    const stored = earlier === undefined || earlier.typeId !== row.typeId || reset ? null : intervalMetric(earlier, row, column.field)
    return { segmentId: row.segmentId, timestamp: row.timestamp, value: chartPointValue(stored, column) }
  })
}

function usePgMetricHistory(hour: number, row: DataRow | null, field: string | null, column: EntityColumn | undefined): readonly ChartPoint[] | null {
  const [loaded, setLoaded] = useState<{ readonly field: string; readonly points: readonly ChartPoint[] } | null>(null)
  useEffect(() => {
    setLoaded(null)
    if (row === null || field === null || column === undefined) return
    const controller = new AbortController()
    const metadata = registry.find((layout) => layout.typeId === row.typeId)?.columnMetadata
      ?.find(({ name }) => name === field)
    const cumulative = metadata?.class === "cumulative" || (metadata === undefined && column.rate === true)
    const resetField = cumulative && registry.find((layout) => layout.typeId === row.typeId)?.columns.includes("stats_reset") === true ? "stats_reset" : undefined
    const fields = resetField === undefined ? [field] : [field, resetField]
    acceptResponse(loadSeries(hour, row.logicalName, {}, fields, controller.signal, row.typeId), controller.signal, (rows) => {
      setLoaded({ field, points: postgresMetricHistory(rows, column, cumulative, resetField) })
    })
    return () => controller.abort()
  }, [column, field, hour, row?.logicalName, row?.timestamp, row?.typeId])
  return loaded?.field === field ? loaded.points : null
}

const NO_ROWS: readonly DataRow[] = []
const NO_RATES: readonly string[] = []

function PgEntityView({
  context,
  onOrder,
  order,
  columns,
  detailColumns,
  cursor,
  data,
  focus,
  finding,
  historyField,
  locale,
  onPattern,
  pattern,
  section,
  t,
  defaultOrder,
  densePageState,
  onLoadMore,
  onContextClear,
  onCursor,
  onRetry,
  transformRows,
}: {
  readonly context?: EntityContext | null | undefined
  readonly columns: readonly EntityColumn[]
  readonly detailColumns?: readonly EntityColumn[] | undefined
  readonly cursor: number
  readonly data: HourData
  readonly focus: DataRow | null
  readonly finding?: Finding | null
  readonly historyField: string | null
  readonly locale: Locale
  readonly section: string
  readonly onOrder?: ((order: TableOrder | null) => void) | undefined
  readonly onPattern?: ((pattern: string) => void) | undefined
  readonly pattern?: string | undefined
  readonly order?: TableOrder | undefined
  readonly t: Translate
  readonly defaultOrder?: TableOrder | undefined
  readonly densePageState?: "idle" | "loading" | "error" | undefined
  readonly onLoadMore?: (() => void) | undefined
  readonly onContextClear?: (() => void) | undefined
  readonly onCursor: (timestamp: number) => void
  readonly onRetry?: (() => void) | undefined
  readonly transformRows?: ((rows: readonly DataRow[]) => readonly DataRow[]) | undefined
}) {
  const time = useDisplayTime()
  const allRows = data.sections[section] ?? NO_ROWS
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  const activeOrder = useMemo(() => order !== undefined && columns.some((column) => column.field === order.column && column.sortable === true)
    ? order
    : defaultOrder, [columns, defaultOrder, order])
  const ranked = useMemo(() => dense
    ? allRows.map(decoratePostgresIntervalRow)
    : snapshot(allRows, cursor), [allRows, cursor, dense])
  const activeContext = context?.logicalName === section ? context : null
  const exactFocus = useMemo(() => focus === null || focus.logicalName !== section || (dense && pattern?.trim() !== "")
    ? null : dense ? decoratePostgresIntervalRow(focus) : focus, [dense, focus, pattern, section])
  const rows = useMemo(() => {
    const contextual = contextualRows(ranked, activeContext, exactFocus)
    return transformRows === undefined ? contextual : transformRows(contextual)
  }, [activeContext, exactFocus, ranked, transformRows])
  const rates = data.rateColumns[section] ?? NO_RATES
  const visibleColumns = useMemo(
    () => visibleEntityColumns(columns, rows, rates),
    [columns, rates, rows],
  )
  const visibleDetailColumns = useMemo(
    () => visibleEntityColumns(detailColumns ?? columns, rows, rates),
    [columns, detailColumns, rates, rows],
  )
  const [selected, setSelected] = useState<DataRow | null>(null)
  useEffect(() => {
    setSelected((current) => selectedEntity(rows, current, section))
  }, [rows, section])
  const selectedKey = selected === null ? null : rowKey(selected)
  const selectedHistoryField = findingHistoryField(visibleColumns, finding, historyField)
  const metadata = data.snapshotRows.find((meta) => meta.logicalName === section)
  const focusPreview = exactFocus !== null && activeContext !== null
    && (densePageState === "loading" || !ranked.some((row) => contextMatches(row, activeContext)))
    ? densePageState === "loading" ? "loading" : "outside"
    : null
  const snapshotStatus = dense
    ? tableState(metadata, ranked.length, cursor, pattern, activeOrder, locale, t, time, focusPreview)
    : undefined
  const canLoadMore = metadata?.hasMore === true && metadata.nextCursor !== null
  const paging = dense && (densePageState !== "idle" || canLoadMore)
    ? <button disabled={densePageState === "loading"} onClick={densePageState === "error" ? onRetry : onLoadMore} type="button">
      {densePageState === "loading" ? "…" : densePageState === "error" ? "↻" : "+"}
    </button>
    : undefined
  const status = historyField === null ? snapshotStatus : <>{snapshotStatus}<span>{t("system.history")}</span></>
  return <div className={selected === null ? "pg-entity-layout pg-table-only" : "pg-entity-layout"} data-pg-section={sectionName(section)} data-testid="pg-entity-layout">
    <div className="pg-entity-main">
      <EntityTable columns={visibleColumns} contextLabel={activeContext?.label} empty={t("table.no_rows")} finding={finding} findingField={finding === null || finding === undefined ? null : fieldNameForLocator(finding)} label={t(`pg.section.${sectionName(section)}`)} locale={locale} onContextClear={activeContext === null ? undefined : onContextClear} onNearEnd={densePageState === "idle" && canLoadMore ? onLoadMore : undefined} onOrder={onOrder} onPattern={onPattern} onSelect={setSelected} order={activeOrder} pattern={pattern} serverSorted={dense} rows={rows} selectedKey={selectedKey} status={status} t={t} testId={`pg-${sectionName(section)}-table`} />
      {paging !== undefined && <div className="lens-tabs" data-testid="table-paging">{paging}</div>}
    </div>
    {selected !== null && <PgDetail allRows={allRows} columns={visibleDetailColumns} cursor={cursor} historyField={selectedHistoryField} hour={Math.floor(cursor / 3_600_000_000) * 3_600_000_000} locale={locale} onClose={() => setSelected(null)} onCursor={onCursor} row={selected} section={section} t={t} />}
  </div>
}

export function tableState(
  metadata: SnapshotRows | undefined,
  rowCount: number,
  cursor: number,
  pattern: string | undefined,
  order: TableOrder | undefined,
  locale: Locale,
  t: Translate,
  time: Pick<DisplayTimeFormatter, "timestamp"> = createDisplayTimeFormatter(locale, "browser"),
  focusPreview: "loading" | "outside" | null = null,
): ReactNode {
  const count = (value: number) => new Intl.NumberFormat(locale).format(value)
  const shown = t("pg.table.shown", { "returned": count(rowCount), "eligible": count(metadata?.eligible ?? rowCount) })
  const semanticOrder = order?.column ?? null
  const serverOrder = metadata?.orderBy.join(", ") ?? null
  const interval = metadata === undefined || metadata.from === null || metadata.to === null
    ? t("pg.table.interval_unavailable")
    : t("pg.table.interval", { from: time.timestamp(metadata.from), to: time.timestamp(metadata.to) })
  return <>
    <span>{t("pg.table.cursor", { time: time.timestamp(cursor) })}</span>
    {focusPreview !== null && <span>{t(`pg.table.focus_${focusPreview}`)}</span>}
    {focusPreview === null && <>
      <span>{interval}</span>
      <span>{pattern?.trim() ? t("pg.table.filter", { pattern: pattern.trim() }) : t("pg.table.no_filter")}</span>
      <span>{semanticOrder === null || serverOrder === null
        ? t("pg.table.order_default")
        : t("pg.table.order", { semantic: t(`pg.field.${semanticOrder}.label`), physical: serverOrder, direction: t(`pg.table.${metadata?.orderDirection ?? (order?.descending === false ? "asc" : "desc")}`) })}</span>
    </>}
    <strong>{focusPreview === null ? shown : t("pg.table.focus_exact")}</strong>
  </>
}

function visibleEntityColumns(columns: readonly EntityColumn[], rows: readonly DataRow[], rates: readonly string[]): readonly EntityColumn[] {
  return columns.filter((column) => rows.some((row) => Object.hasOwn(row.values, column.field)))
    .map((column) => column.rate === true || rates.includes(column.field) ? { ...column, rate: true } : column)
}

function PgDetail({ allRows, columns, cursor, historyField, hour, locale, onClose, onCursor, overview = false, row, section, t }: { readonly allRows: readonly DataRow[]; readonly columns: readonly EntityColumn[]; readonly cursor: number; readonly historyField: string | null; readonly hour: number; readonly locale: Locale; readonly onClose: () => void; readonly onCursor: (timestamp: number) => void; readonly overview?: boolean | undefined; readonly row: DataRow; readonly section: string; readonly t: Translate }) {
  const entityRows = useMemo(() => allRows.filter((candidate) => sameEntity(candidate, row, section)), [allRows, row, section])
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  const chartColumns = useMemo(() => columns.filter((column) => chartableColumn(column)
    && (dense ? denseHistoryFields(row.typeId, column.field).length !== 0 : chartColumnAvailable(section, entityRows, column))), [columns, dense, entityRows, row.typeId, section])
  const preferredField = chartColumns.some(({ field }) => field === historyField) ? historyField : chartColumns[0]?.field ?? null
  const chartFields = chartColumns.map(({ field }) => field).join("\u0000")
  const [metricField, setMetricField] = useState(preferredField)
  useEffect(() => {
    setMetricField((current) => current !== null && chartFields.split("\u0000").includes(current) ? current : preferredField)
  }, [chartFields, preferredField])
  const activeMetricField = chartColumns.some(({ field }) => field === metricField) ? metricField : preferredField
  const historyColumn = chartColumns.find((column) => column.field === activeMetricField)
  const loadedHistory = useMemo(() => activeMetricField === null ? [] : buildMetricSamples(
    entityRows,
    (candidate) => Object.hasOwn(candidate.values, activeMetricField) ? chartPointValue(value(candidate, activeMetricField), historyColumn) : undefined,
  ), [activeMetricField, entityRows, historyColumn])
  const exactHistory = usePostgresMetricHistory(row, section, historyColumn, hour)
  const history = exactHistory ?? loadedHistory
  const textField = section === "pg_store_plans" ? "plan" : "query"
  const exactText = useWholeText(row, section, textField)?.trim() || null
  const fields = columns.filter((column) => column.field !== textField)
  return <aside className="pg-detail" data-testid="pg-detail">
    <header><div><span>{overview ? t(overviewSectionKey(section)) : section === "pg_stat_progress_vacuum" ? section : t(`pg.section.${sectionName(section)}`)}</span><h2>{detailTitle(row, section, t)}</h2></div><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    <ChartOnly>{activeMetricField !== null && historyColumn !== undefined && <section className="process-history pg-metric-history">
      <div aria-label={t("system.history")} className="process-history-selector" role="group">{chartColumns.map((column) => <button aria-pressed={activeMetricField === column.field} data-testid={`pg-chart-${column.field}`} key={column.field} onClick={() => setMetricField(column.field)} type="button">{t(column.label)}</button>)}</div>
      <SeriesChart cursor={cursor} helpKey={historyColumn.help ?? "chart.metric.help"} hour={hour} labelKey={historyColumn.label} locale={locale} format={chartFormat(historyColumn.kind)} onCursor={onCursor} points={history} scale={chartScale(historyColumn)} t={t} unit={chartUnit(historyColumn, t("unit.per_second"))} />
    </section>}</ChartOnly>
    {exactText !== null && <section className="query-block"><span>{t(section === "pg_store_plans" ? "pg.plan.label" : "pg.query.label")}<button aria-label={t("common.raw")} className="copy-raw" onClick={() => void navigator.clipboard?.writeText(exactText)} type="button"><Copy aria-hidden="true" size={12} /></button></span><pre data-testid={section === "pg_store_plans" ? "pg-exact-plan" : "pg-exact-query"}>{exactText}</pre></section>}
    <dl>{fields.filter((column) => told(value(row, column.field))).map((column) => <div key={column.field}><dt><span>{column.help === undefined ? t(column.label) : <LabelHelp helpKey={column.help} labelKey={column.label} t={t} />}</span></dt><dd>{display(value(row, column.field), column, locale, t)}</dd></div>)}</dl>
  </aside>
}

export function chartColumnAvailable(section: string, rows: readonly DataRow[], column: EntityColumn): boolean {
  if (rows.some((row) => Object.hasOwn(row.values, column.field))) return true
  if (section !== "pg_stat_activity") return false
  const source = activityDurationSource(column.field)
  return source !== null && rows.some((row) => Object.hasOwn(row.values, source))
}

function usePostgresMetricHistory(row: DataRow, section: string, column: EntityColumn | undefined, hour: number): readonly ChartPoint[] | null {
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  const [history, setHistory] = useState<readonly ChartPoint[] | null>(dense ? [] : null)
  useEffect(() => {
    if (column === undefined) {
      setHistory(dense ? [] : null)
      return
    }
    const activity = section === "pg_stat_activity"
    const durationField = !dense && activity ? activityDurationSource(column.field) : null
    const field = dense ? null : typeof column.physicalField === "string"
      ? column.physicalField
      : column.physicalField?.[row.typeId] ?? column.field
    const resetField = !dense && column.rate === true && registry.find((layout) => layout.typeId === row.typeId)?.columns.includes("stats_reset") === true
      ? "stats_reset" : undefined
    const metricFields = dense ? denseHistoryFields(row.typeId, column.field)
      : durationField === null ? uniqueText([field!, resetField ?? null]) : durationField === "query_start" ? ["state", durationField] : [durationField]
    const fields = activity ? ["pid", ...metricFields] : metricFields
    if (fields.length === 0) {
      setHistory([])
      return
    }
    const identities = identityFields(section, row.typeId).map((name) => [name, rawText(value(row, name))] as const)
    if (identities.some(([, stored]) => stored === null)) {
      setHistory(null)
      return
    }
    setHistory([])
    const controller = new AbortController()
    const filters = Object.fromEntries(identities as readonly (readonly [string, string])[])
    const request = activity
      ? loadSeries(hour, section, filters, fields, controller.signal)
      : loadSeries(hour, section, filters, fields, controller.signal, row.typeId)
    acceptResponse(request, controller.signal, (rows) => {
      const entityHistory = rows.filter((candidate) => !activity || sameEntity(candidate, row, section))
      setHistory(dense
        ? denseMetricHistory(entityHistory, row.typeId, column)
        : durationField === null
          ? postgresMetricHistory(entityHistory, { ...column, field: field! }, column.rate === true, resetField)
          : activityDurationHistory(entityHistory, column.field as Parameters<typeof activityDurationHistory>[1]))
    })
    return () => controller.abort()
  }, [column, dense, hour, row, section])
  return history
}

function historyFields(typeId: string, semantic: PostgresSemanticField): readonly string[] {
  if (semantic === "mean_exec_ms_per_call") {
    return uniqueText([
      physicalField(typeId, "calls_per_second"),
      physicalField(typeId, "execution_ms_per_second"),
    ])
  }
  return uniqueText([physicalField(typeId, semantic)])
}

const DENSE_DERIVED_FIELDS = new Set([
  "rows_per_call", "blocks_per_call", "hit_pct", "wal_per_call", "plan_time_pct", "cv",
  "min_exec_time_ms", "max_exec_time_ms", "mean_exec_time_ms", "stddev_exec_time_ms",
])

export function denseHistoryFields(typeId: string, field: string): readonly string[] {
  const available = new Set(registry.find((layout) => layout.typeId === typeId)?.columns ?? [])
  const present = (fields: readonly (string | null)[]) => uniqueText(fields.filter((name): name is string => name !== null && available.has(name)))
  const semantic = (...fields: readonly PostgresSemanticField[]) => present(fields.map((name) => physicalField(typeId, name)))
  if (isSemanticField(field)) return historyFields(typeId, field)
  if (field === "rows_per_call") return semantic("rows_per_second", "calls_per_second")
  if (field === "blocks_per_call") return present(["shared_blks_hit", "shared_blks_read", "local_blks_hit", "local_blks_read", ...semantic("calls_per_second")])
  if (field === "hit_pct") return present(["shared_blks_hit", "shared_blks_read"])
  if (field === "wal_per_call") return present(["wal_bytes", ...semantic("calls_per_second")])
  if (field === "plan_time_pct") return semantic("planning_ms_per_second", "execution_ms_per_second")
  if (DENSE_DERIVED_FIELDS.has(field)) return present(denseExecutionFields(typeId, field === "cv" ? ["mean", "stddev"] : [field.split("_")[0]! as "min" | "max" | "mean" | "stddev"]))
  return available.has(field) ? [field] : []
}

export function denseMetricHistory(rows: readonly DataRow[], typeId: string, column: EntityColumn): readonly ChartPoint[] {
  if (isSemanticField(column.field)) {
    const semantic = column.field
    return buildMetricSamples(postgresHistory(rows), (point) => Object.hasOwn(point, semantic) ? point[semantic] : undefined)
  }
  const ordered = rows.filter((row) => row.typeId === typeId).slice()
    .sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  const dependencies = denseHistoryFields(typeId, column.field)
  if (!DENSE_DERIVED_FIELDS.has(column.field)) {
    return postgresMetricHistory(ordered, column, column.rate === true)
  }
  return ordered.flatMap((row, index) => {
    const stored = denseDerivedValue(ordered, index, typeId, column.field, dependencies)
    return stored === undefined ? [] : [{ segmentId: row.segmentId, timestamp: row.timestamp, value: stored }]
  })
}

function denseDerivedValue(rows: readonly DataRow[], index: number, typeId: string, field: string, dependencies: readonly string[]): number | null | undefined {
  const row = rows[index]!
  const rate = (name: string): number | null | undefined => {
    if (!Object.hasOwn(row.values, name)) return undefined
    let earlier: DataRow | undefined
    for (let before = index - 1; before >= 0; before -= 1) {
      if (Object.hasOwn(rows[before]!.values, name)) { earlier = rows[before]; break }
    }
    return earlier === undefined || earlier.typeId !== row.typeId ? null : intervalMetric(earlier, row, name)
  }
  const gauge = (name: string): number | null | undefined => Object.hasOwn(row.values, name) ? asNumber(value(row, name)) : undefined
  const quotient = (left: number | null | undefined, right: number | null | undefined): number | null | undefined => left === undefined || right === undefined
    ? undefined : left === null || right === null || right <= 0 ? null : left / right
  const calls = physicalField(typeId, "calls_per_second")
  if (field === "rows_per_call") return quotient(rate(dependencies.find((name) => name !== calls) ?? ""), calls === null ? undefined : rate(calls))
  if (field === "blocks_per_call") {
    if (calls === null) return undefined
    const blocks = dependencies.filter((name) => name !== calls).map(rate)
    if (blocks.some((stored) => stored === undefined)) return undefined
    if (blocks.some((stored) => stored === null)) return null
    return quotient(blocks.reduce<number>((total, stored) => total + (stored ?? 0), 0), rate(calls))
  }
  if (field === "hit_pct") {
    const hit = rate("shared_blks_hit")
    const read = rate("shared_blks_read")
    if (hit === undefined || read === undefined) return undefined
    if (hit === null || read === null || hit + read <= 0) return null
    return 100 * hit / (hit + read)
  }
  if (field === "wal_per_call") return quotient(rate("wal_bytes"), calls === null ? undefined : rate(calls))
  if (field === "plan_time_pct") {
    const planning = physicalField(typeId, "planning_ms_per_second")
    const execution = physicalField(typeId, "execution_ms_per_second")
    const plan = planning === null ? undefined : rate(planning)
    const run = execution === null ? undefined : rate(execution)
    if (plan === undefined || run === undefined) return undefined
    if (plan === null || run === null || plan + run <= 0) return null
    return 100 * plan / (plan + run)
  }
  if (field === "cv") return quotient(gauge(dependencies[1] ?? ""), gauge(dependencies[0] ?? ""))
  return gauge(dependencies[0] ?? "")
}

function denseExecutionFields(typeId: string, names: readonly ("min" | "max" | "mean" | "stddev")[]): readonly string[] {
  const old = typeId === "1002001" || PG_STORE_PLANS_TYPE_IDS.includes(typeId as typeof PG_STORE_PLANS_TYPE_IDS[number])
  return names.map((name) => old ? `${name}_time` : `${name}_exec_time`)
}

function isSemanticField(field: string): field is PostgresSemanticField {
  return [
    "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second",
    "planning_ms_per_second", "shared_blk_read_ms_per_second", "shared_blk_write_ms_per_second",
    "local_blk_read_ms_per_second", "local_blk_write_ms_per_second",
    "temp_blk_read_ms_per_second", "temp_blk_write_ms_per_second",
  ].includes(field)
}

function uniqueText(values: readonly (string | null)[]): readonly string[] {
  return unique(values.filter((field): field is string => field !== null))
}

function useWholeText(row: DataRow, section: string, field: string): string | null {
  const shown = rawText(value(row, field))
  const [whole, setWhole] = useState<string | null>(null)
  useEffect(() => {
    setWhole(null)
    if (shown === null) return
    const controller = new AbortController()
    const filters = Object.fromEntries(identityFields(section, row.typeId)
      .map((name) => [name, rawText(value(row, name)) ?? ""]))
    acceptResponse(loadSnapshot(
      row.segmentId,
      row.timestamp,
      [{ section, fields: [field], typeId: row.typeId }],
      controller.signal,
      undefined,
      { filters, typeId: row.typeId, fullText: true },
    ), controller.signal, (data) => {
      const text = rawText(value(data.sections[section]?.[0] ?? row, field))
      if (text !== null) setWhole(text)
    })
    return () => controller.abort()
  }, [field, row, section, shown])
  return whole ?? shown
}

function told(cell: ReturnType<typeof value>): boolean {
  if (cell === null) return false
  return rawText(cell)?.trim() !== ""
}

export function sameEntity(left: DataRow, right: DataRow, section: string): boolean {
  if (section === "pg_stat_activity") {
    const pid = rawText(value(left, "pid"))
    return pid !== null && pid === rawText(value(right, "pid"))
  }
  const fields = identityFields(section, left.typeId)
  return left.typeId === right.typeId
    && fields.every((field) => rawText(value(left, field)) === rawText(value(right, field)))
}

export function selectedEntity(rows: readonly DataRow[], current: DataRow | null, section: string): DataRow | null {
  if (current !== null) {
    const advanced = rows.find((row) => sameEntity(row, current, section))
    if (advanced !== undefined) return advanced
  }
  return null
}

export function postgresDatabaseCount(rows: readonly DataRow[]): number {
  return rows.filter((row) => asNumber(value(row, "datid")) !== 0).length
}

function identityFields(section: string, typeId?: string): readonly string[] {
  if (section === "pg_stat_activity" || section === "pg_stat_progress_vacuum" || section === "pg_locks") return ["pid"]
  if (section === "pg_stat_io") return ["backend_type", "object", "context"]
  if (section === "pg_prepared_xacts") return ["datname"]
  if ((section === "pg_stat_statements" || section === "pg_store_plans") && typeId !== undefined) return postgresIdentity(typeId)
  return registry.find((layout) => layout.typeId === typeId)?.identity ?? (section === "pg_stat_database" ? ["datid"] : [])
}

function overviewSectionKey(section: string): string { return section === "pg_stat_database" ? "pg.section.databases" : `pg.overview.section.${section}` }
function overviewFieldKey(field: string): string { return `pg.overview.field.${field}` }

function detailTitle(row: DataRow, section: string, t: Translate): string {
  if (section === "pg_stat_activity" || section === "pg_stat_progress_vacuum" || section === "pg_locks") return t("pg.detail.pid", { pid: identifier(value(row, "pid")) })
  if (section === "pg_stat_statements") return t("pg.detail.query", { id: identifier(value(row, "queryid")) })
  if (section === "pg_store_plans") return t("pg.detail.plan", { id: identifier(value(row, "planid")) })
  if (section === "pg_stat_io") return ["backend_type", "object", "context"].flatMap((field) => rawText(value(row, field)) ?? []).join(" · ")
  if (section === "pg_prepared_xacts") return rawText(value(row, "datname")) ?? t("common.unavailable")
  return rawText(value(row, "datname")) ?? identifier(value(row, "datid"))
}

export function display(cell: ReturnType<typeof value>, column: EntityColumn, locale: Locale, t: Translate): ReactNode {
  if (cell === null) return "—"
  if (column.kind === "estimated_rows") return <EstimatedRows cell={cell} locale={locale} t={t} />
  if (column.field === "xid_age" || column.field === "mxid_age") return <span title={String(cell)}>{compact(asNumber(cell)!, locale)}</span>
  if (column.kind === "timestamp") {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : <TimestampValue t={t} timestamp={timestamp} />
  }
  if (column.kind === "id") return rawText(cell) ?? "—"
  const per = t("unit.per_second")
  if (column.kind === "bytes") {
    const output = unit(humanBytes(cell, locale), column.rate, per)
    return <span title={output}>{output}</span>
  }
  if (column.kind === "kib") return unit(humanBytes(asNumber(cell) === null ? null : (asNumber(cell) ?? 0) * 1024, locale), column.rate, per)
  if (column.kind === "milliseconds") return measure(cell, locale, unit(t("unit.ms"), column.rate, per))
  if (column.kind === "duration") return humanDuration(cell, locale)
  if (column.kind === "microseconds") return measure(cell, locale, unit(t("unit.us"), column.rate, per))
  if (column.kind === "percent") return humanPercent(cell, locale, column.rate === true ? per : "")
  if (column.kind === "boolean" && typeof cell === "boolean") return locale === "ru" ? cell ? "да" : "нет" : String(cell)
  if (typeof cell === "number") return measure(cell, locale, unit("", column.rate, per))
  return rawText(cell) ?? "—"
}

function TimestampValue({ t, timestamp }: { readonly t: Translate; readonly timestamp: number }) {
  const time = useDisplayTime()
  return <span className="timestamp-value"><span>{time.timestamp(timestamp)}</span><button aria-label={t("common.raw")} onClick={() => void navigator.clipboard?.writeText(String(timestamp))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
}

function groupSections(rows: readonly DataRow[]): readonly [string, readonly DataRow[]][] {
  const grouped = new Map<string, DataRow[]>()
  for (const row of rows) {
    const stored = grouped.get(row.logicalName) ?? []
    stored.push(row)
    grouped.set(row.logicalName, stored)
  }
  return [...grouped.entries()]
}

export function columnsFor(rows: readonly DataRow[]): readonly EntityColumn[] {
  const fields = [...new Set(rows.flatMap((row) => Object.keys(row.values)))].filter((field) => !isInternalField(field))
  return fields.map((field, index) => {
    const sample = rows.find((row) => value(row, field) !== null)
    const cell = sample === undefined ? null : value(sample, field)
    const kind: EntityColumn["kind"] = field === "pid" || field.endsWith("id") || field.endsWith("_id")
      ? "id"
      : isTimestampField(field)
        ? "timestamp"
        : field.endsWith("_time")
          ? "milliseconds"
          : field.endsWith("_us")
            ? "microseconds"
            : field.endsWith("_bytes")
              ? "bytes"
        : typeof cell === "number" ? "number" : typeof cell === "boolean" ? "boolean" : "text"
    return { field, label: field, kind, sticky: index === 0, width: kind === "text" ? 190 : kind === "timestamp" ? 210 : 135 }
  })
}

export const PROGRESS_VACUUM_FIELDS = [
  "pid", "datname", "is_autovacuum", "heap_blks_scanned", "heap_blks_total", "heap_blks_vacuumed",
  "index_vacuum_count", "indexes_processed", "indexes_total", "num_dead_tuples", "max_dead_tuples",
  "num_dead_item_ids", "dead_tuple_bytes", "max_dead_tuple_bytes", "delay_time", "phase",
] as const

export function progressVacuumColumns(rows: readonly DataRow[], rates: readonly string[]): readonly EntityColumn[] {
  const available = new Map(columnsFor(rows).map((column) => [column.field, column]))
  return PROGRESS_VACUUM_FIELDS.flatMap((field) => {
    const column = available.get(field)
    if (column === undefined) return []
    return [{
      ...column,
      ...(field === "pid" ? {} : { help: `pg.vacuum.${field}.help` }),
      label: `pg.vacuum.${field}.label`,
      rate: rates.includes(field),
      sticky: field === "pid",
    }]
  })
}

const REGISTRY_IDENTITIES = new Map(registry.map((layout) => [layout.typeId, new Set(layout.identity)]))
const INTERNAL_FIELDS = new Set(["ts", "ordinal", "segment_id", "type_id", "row_ordinal", "field_ordinal"])

export function registryCardFields(row: DataRow): readonly (readonly [string, ReturnType<typeof value>])[] {
  const identity = REGISTRY_IDENTITIES.get(row.typeId)
  return Object.entries(row.values).filter(([field]) => !isInternalField(field)
    && identity?.has(field) !== true
    && !isTimestampField(field))
}

function isInternalField(field: string): boolean {
  return INTERNAL_FIELDS.has(field)
}

export function overviewValue(cell: ReturnType<typeof value>, field: string, locale: Locale, time: Pick<DisplayTimeFormatter, "timestamp"> = createDisplayTimeFormatter(locale, "browser")): string {
  if (cell === null) return "—"
  if (isTimestampField(field)) {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : time.timestamp(timestamp)
  }
  if (field === "pid" || field.endsWith("id") || field.endsWith("_id")) return rawText(cell) ?? "—"
  if (field.endsWith("_time")) return measure(cell, locale, " ms")
  if (field.endsWith("_us")) return measure(cell, locale, " μs")
  if (field.endsWith("_bytes")) return humanBytes(cell, locale)
  if (typeof cell === "boolean") return locale === "ru" ? cell ? "да" : "нет" : String(cell)
  if (typeof cell === "number") return measure(cell, locale)
  return rawText(cell) ?? "—"
}

function rowKey(row: DataRow): string { return `${row.segmentId}:${row.typeId}:${row.ordinal}` }
export function isTimestampField(field: string): boolean {
  return field.endsWith("_start") || field === "state_change" || field === "waitstart" || field === "stats_reset" || field === "stats_since"
}
function findingHistoryField(columns: readonly EntityColumn[], finding: Finding | null | undefined, fallback: string | null): string | null {
  const field = finding === null || finding === undefined ? null : fieldNameForLocator(finding)
  const semantic = finding === null || finding === undefined || field === null ? null : findingSemanticField(finding.typeId, field)
  const column = columns.find((candidate) => candidate.field === (semantic ?? field))
  return column === undefined || column.kind === "text" || column.kind === "timestamp" || column.kind === "boolean" ? fallback : column.field
}
const CHARTABLE_KINDS = new Set<EntityColumn["kind"]>(["number", "estimated_rows", "bytes", "kib", "milliseconds", "duration", "microseconds", "percent"])
export function chartableColumn(column: EntityColumn): boolean {
  return CHARTABLE_KINDS.has(column.kind)
}
export function chartScale(column: EntityColumn): "percent" | "nonnegative" {
  return column.kind === "percent" ? "percent" : "nonnegative"
}
export function chartUnit(column: EntityColumn, perSecond = "/s"): string {
  const per = column.rate === true ? perSecond : ""
  if (column.kind === "percent") return `%${per}`
  if (column.kind === "bytes" || column.kind === "kib") return `B${per}`
  if (column.kind === "milliseconds" || column.kind === "duration" || column.kind === "microseconds") return `ms${per}`
  return column.rate === true ? perSecond : "count"
}
export function chartPointValue(cell: ReturnType<typeof value>, column: EntityColumn | undefined): number | null {
  const number = asNumber(cell)
  if (number === null) return null
  if (column?.kind === "kib") return number * 1024
  if (column?.kind === "microseconds") return number / 1_000
  return number
}
export function chartFormat(kind: EntityColumn["kind"]): ((value: number, locale: Locale) => string) | undefined {
  if (kind === "percent") return humanPercent
  if (kind === "bytes") return (value, locale) => humanBytes(value, locale)
  if (kind === "kib") return (value, locale) => humanBytes(value, locale)
  if (kind === "microseconds") return (value, locale) => measure(value, locale, " ms")
  if (kind === "duration") return humanDuration
  return undefined
}
function sectionName(section: string): PostgresSection {
  if (section === "pg_stat_activity") return "activity"
  if (section === "pg_stat_statements") return "statements"
  if (section === "pg_store_plans") return "plans"
  if (section === "pg_locks") return "locks"
  return "databases"
}
function pgColumn(field: string, kind: NonNullable<EntityColumn["kind"]>, width: number, sticky = false, withHelp = true): EntityColumn {
  return { field, label: `pg.field.${field}.label`, ...(withHelp ? { help: `pg.field.${field}.help` } : {}), kind, width, sticky }
}
function text(field: string, width = 130, sticky = false, withHelp = true): EntityColumn { return pgColumn(field, "text", width, sticky, withHelp) }
function number(field: string, width = 125): EntityColumn { return { ...pgColumn(field, "number", width), sortable: true } }
function id(field: string, width = 110, sticky = false, withHelp = true): EntityColumn { return pgColumn(field, "id", width, sticky, withHelp) }
function bytes(field: string, width = 140): EntityColumn { return { ...pgColumn(field, "bytes", width), sortable: true } }
function milliseconds(field: string, width = 145): EntityColumn { return { ...pgColumn(field, "milliseconds", width), sortable: true } }
function duration(field: string, width = 145): EntityColumn { return pgColumn(field, "duration", width) }
function percent(field: string, width = 125): EntityColumn { return { ...pgColumn(field, "percent", width), sortable: true } }
function rateNumber(field: string, width = 125): EntityColumn { return { ...number(field, width), rate: true } }
function rateBytes(field: string, width = 140): EntityColumn { return { ...bytes(field, width), rate: true } }
function rateMilliseconds(field: string, width = 145): EntityColumn { return { ...milliseconds(field, width), rate: true } }
function timestamp(field: string, width = 210): EntityColumn { return { ...pgColumn(field, "timestamp", width), sortable: true } }
function boolean(field: string, width = 125): EntityColumn { return pgColumn(field, "boolean", width) }
function pgText(field: string, key: string, width = 130, sticky = false, withHelp = true): EntityColumn { return { field, label: `${key}.label`, ...(withHelp ? { help: `${key}.help` } : {}), kind: "text", width, sticky } }
function pgNumber(field: string, key: string, width = 125): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "number", width } }
function pgId(field: string, key: string, width = 110, sticky = false, withHelp = true): EntityColumn { return { field, label: `${key}.label`, ...(withHelp ? { help: `${key}.help` } : {}), kind: "id", width, sticky } }
function pgTimestamp(field: string, key: string, width = 210): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "timestamp", width } }
