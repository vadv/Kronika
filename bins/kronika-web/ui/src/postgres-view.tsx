import { Activity, BarChart3, Copy, Database, KeyRound, LockKeyhole, ScrollText, X } from "lucide-react"
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react"
import { registry } from "kronika:registry"

import type { DataRow, Finding, HourData } from "./api"
import { EntityTable, unit, type EntityColumn, type TableOrder } from "./entity-table"
import type { Translate } from "./help"
import { fieldNameForLocator, loadSeries, loadSnapshot } from "./api"
import { LabelHelp } from "./help"
import { asNumber, formatUtc, humanBytes, humanDuration, identifier, measure, rawText, snapshot, value, type Locale, shownMoment } from "./model"
import { decoratePostgresIntervalRow, findingSemanticField, PG_STAT_STATEMENTS_TYPE_IDS, PG_STORE_PLANS_TYPE_IDS, physicalField, physicalFields, postgresHistory, postgresIdentity, type PlanLens, type PostgresSemanticField, type StatementLens } from "./postgres-metrics"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

export type PostgresSection = "overview" | "activity" | "statements" | "plans" | "locks" | "databases"

export const ACTIVITY_DEFAULT_ORDER: TableOrder = { column: "query_duration_ms", descending: true }

const ACTIVITY_PID = pgId("pid", "pg.field.pid", 78, true)
const ACTIVITY_BACKEND_TYPE = pgText("backend_type", "pg.backend_type", 150, true)

export const ACTIVITY_COLUMNS: readonly EntityColumn[] = [
  ACTIVITY_PID, duration("query_duration_ms", 145), duration("transaction_duration_ms", 155), pgText("state", "pg.state", 140),
  pgText("wait_event_type", "pg.wait_event_type", 135), pgText("wait_event", "pg.wait_event", 155), pgText("datname", "pg.datname", 145), pgText("usename", "pg.usename", 130),
  pgText("application_name", "pg.application_name", 180), pgText("client_addr", "pg.client_addr", 150), pgText("query", "pg.query", 420),
]

export const ACTIVITY_DETAIL_COLUMNS: readonly EntityColumn[] = [
  ...ACTIVITY_COLUMNS, ACTIVITY_BACKEND_TYPE, pgId("leader_pid", "pg.leader_pid", 105), pgId("query_id", "pg.query_id", 150),
  pgNumber("backend_xmin_age", "pg.backend_xmin_age", 145), pgNumber("backend_xid_age", "pg.backend_xid_age", 145),
  pgTimestamp("backend_start", "pg.backend_start", 210), pgTimestamp("xact_start", "pg.xact_start", 210), pgTimestamp("query_start", "pg.query_start", 210), pgTimestamp("state_change", "pg.state_change", 210),
]

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
  text("datname", 145), text("usename", 130), id("queryid", 155), id("dbid", 120), id("userid", 120),
  boolean("toplevel", 105), timestamp("stats_since", 210),
]

const STATEMENT_DERIVED_COLUMNS: readonly EntityColumn[] = [
  number("exec_load", 125), number("rows_per_call", 130), number("blocks_per_call", 140),
  percent("hit_pct", 115), bytes("wal_per_call", 135), percent("plan_time_pct", 130),
  number("cv", 90), milliseconds("min_exec_time_ms", 125), milliseconds("max_exec_time_ms", 125),
  milliseconds("mean_exec_time_ms", 135), milliseconds("stddev_exec_time_ms", 145),
]

const STATEMENT_LENSES: Readonly<Record<StatementLens, readonly string[]>> = {
  load: ["query", "calls_per_second", "execution_ms_per_second", "exec_load", "mean_exec_ms_per_call", "rows_per_second", "datname", "usename", "queryid", "toplevel"],
  per_call: ["query", "mean_exec_ms_per_call", "rows_per_call", "blocks_per_call", "calls_per_second", "datname", "usename", "queryid", "toplevel"],
  io: ["query", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "shared_blks_written", "local_blks_read", "temp_blks_read", "temp_blks_written", "datname", "queryid"],
  resources: ["query", "temp_blks_written", "wal_bytes", "wal_per_call", "planning_ms_per_second", "plan_time_pct", "calls_per_second", "execution_ms_per_second", "datname", "queryid"],
  stability: ["query", "cv", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second", "datname", "queryid"],
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
  text("datname", 145), text("usename", 130), id("dbid", 120), id("userid", 120),
  text("cmd_type", 125), text("relids", 190), id("queryid_stat_statements", 220),
]

const PLAN_DERIVED_COLUMNS: readonly EntityColumn[] = [
  number("exec_load", 125), number("rows_per_call", 130), number("blocks_per_call", 140), percent("hit_pct", 115),
  percent("plan_time_pct", 130), milliseconds("min_exec_time_ms", 125), milliseconds("max_exec_time_ms", 125),
  milliseconds("mean_exec_time_ms", 135), milliseconds("stddev_exec_time_ms", 145),
  timestamp("first_call", 190), timestamp("last_call", 190),
]

const PLAN_LENSES: Readonly<Record<PlanLens, readonly string[]>> = {
  load: ["plan", "calls_per_second", "execution_ms_per_second", "exec_load", "mean_exec_ms_per_call", "rows_per_second", "datname", "usename", "queryid", "planid"],
  timing: ["plan", "queryid", "planid", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second", "first_call", "last_call", "datname"],
  io: ["plan", "queryid", "planid", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "local_blks_read", "temp_blks_read", "datname"],
  identity: ["plan", "queryid", "planid", "dbid", "userid", "datname", "usename", "cmd_type", "relids", "queryid_stat_statements"],
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

const LOCK_COLUMNS: readonly EntityColumn[] = [
  id("pid", 78, true), text("datname", 145, true), text("usename", 130), text("application_name", 180),
  text("state", 110), text("wait_event_type", 135), text("wait_event", 155), text("blocked_by", 150),
  text("lock_locktype", 145), text("lock_mode", 180), text("lock_target", 260), text("lock_relname", 180),
  id("lock_relation", 135), id("lock_transactionid", 160), timestamp("waitstart", 210), text("query", 420),
]

const DATABASE_COLUMNS: readonly EntityColumn[] = [
  id("datid", 105, true), text("datname", 170, true), number("numbackends", 135), number("xact_commit", 145),
  number("xact_rollback", 145), number("blks_hit", 140), number("blks_read", 140), number("tup_returned", 145),
  number("tup_fetched", 145), number("tup_inserted", 145), number("tup_updated", 145), number("tup_deleted", 145),
  number("deadlocks", 125), number("conflicts", 125), number("temp_files", 125), bytes("temp_bytes", 145),
  milliseconds("blk_read_time", 150), milliseconds("blk_write_time", 155), number("sessions", 125), number("frozen_xid_age", 155),
]

const TABS: readonly { readonly id: PostgresSection; readonly icon: ReactNode; readonly sections?: readonly string[] }[] = [
  { id: "overview", icon: <BarChart3 size={13} /> },
  { id: "activity", icon: <Activity size={13} />, sections: ["pg_stat_activity", "pg_stat_progress_vacuum"] },
  { id: "statements", icon: <KeyRound size={13} />, sections: ["pg_stat_statements"] },
  { id: "plans", icon: <ScrollText size={13} />, sections: ["pg_store_plans", "pg_store_plans_info"] },
  { id: "locks", icon: <LockKeyhole size={13} />, sections: ["pg_locks"] },
  { id: "databases", icon: <Database size={13} />, sections: ["pg_stat_database"] },
]

export function PostgresView({
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
  onFinding,
  onPlanLens,
  onSection,
  onStatementLens,
  planLens,
  section,
  statementLens,
  t,
}: {
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
  readonly onFinding: (finding: Finding) => void
  readonly onSection: (section: PostgresSection) => void
  readonly section: PostgresSection
  readonly statementLens: StatementLens
  readonly planLens: PlanLens
  readonly onStatementLens: (lens: StatementLens) => void
  readonly onPlanLens: (lens: PlanLens) => void
  readonly t: Translate
}) {
  const available = (name: string) => data.availableSections.includes(name)
  useEffect(() => {
    const tab = TABS.find((candidate) => candidate.id === section)
    if (tab === undefined || tab.id === "plans" || tab.sections === undefined || tab.sections.some(available)) return
    onSection("overview")
  }, [data.availableSections, onSection, section])
  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={onCursor} onFinding={onFinding} primaryLane={section === "statements" || section === "plans" ? "pg_running" : section === "activity" || section === "locks" ? "pg_waiting" : "health"} shownAt={shownAt} t={t} />
    <nav aria-label={t("pg.sections")} className="pg-tabs">
      {TABS.map((tab) => {
        const enabled = tab.id === "plans" || tab.sections === undefined || tab.sections.some(available)
        return <button aria-current={section === tab.id ? "page" : undefined} disabled={!enabled} key={tab.id} onClick={() => { if (section !== tab.id) onOrder(null); onSection(tab.id) }} title={enabled ? undefined : t("pg.no_section_data")} type="button">{tab.icon}<span>{t(`pg.section.${tab.id}`)}</span></button>
      })}
    </nav>
    {section === "overview" && <Overview cursor={cursor} data={data} locale={locale} t={t} />}
    {section === "activity" && available("pg_stat_activity") && <ActivityView onOrder={onOrder} order={order} onPattern={onPattern} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_activity" ? focusFinding : null} focus={focus} locale={locale} t={t} />}
    {section === "activity" && available("pg_stat_progress_vacuum") && <PgPreview cursor={cursor} data={data} focus={focusFinding?.logicalName === "pg_stat_progress_vacuum" ? focus : null} locale={locale} section="pg_stat_progress_vacuum" t={t} />}
    {section === "statements" && <><PostgresLensBar active={statementLens} choices={["load", "per_call", "io", "resources", "stability"]} onChange={onStatementLens} prefix="statement" t={t} /><PgEntityView columns={statementColumns(statementLens)} onOrder={onOrder} onPattern={onPattern} pattern={pattern} order={order} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_statements" ? focusFinding : null} focus={focus} historyField={statementLens === "stability" ? "cv" : "mean_exec_ms_per_call"} locale={locale} section="pg_stat_statements" t={t} /></>}
    {section === "plans" && available("pg_store_plans_info") && <PlanInfo cursor={cursor} data={data} locale={locale} t={t} />}
    {section === "plans" && <PostgresLensBar active={planLens} choices={["load", "timing", "io", "identity"]} onChange={onPlanLens} prefix="plan" t={t} />}
    {section === "plans" && available("pg_store_plans") && <PgEntityView columns={planColumns(planLens)} onOrder={onOrder} onPattern={onPattern} pattern={pattern} order={order} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_store_plans" ? focusFinding : null} focus={focus} historyField="mean_exec_ms_per_call" locale={locale} section="pg_store_plans" t={t} />}
    {section === "plans" && !available("pg_store_plans") && <p className="pg-empty" data-testid="pg-plans-empty">{t("pg.plans.empty")}</p>}
    {section === "locks" && <PgEntityView columns={LOCK_COLUMNS} onOrder={onOrder} order={order} onPattern={onPattern} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_locks" ? focusFinding : null} focus={focus} historyField={null} locale={locale} section="pg_locks" t={t} />}
    {section === "databases" && <PgEntityView columns={DATABASE_COLUMNS} onOrder={onOrder} order={order} onPattern={onPattern} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_database" ? focusFinding : null} focus={focus} historyField="xact_commit" locale={locale} section="pg_stat_database" t={t} />}
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

export function activityDurationMs(row: DataRow): number | null {
  if (rawText(value(row, "state")) !== "active") return null
  return elapsedSince(row, "query_start")
}

export function transactionDurationMs(row: DataRow): number | null {
  return elapsedSince(row, "xact_start")
}

function elapsedSince(row: DataRow, field: string): number | null {
  const started = asNumber(value(row, field))
  return started === null || started <= 0 || started > row.timestamp ? null : (row.timestamp - started) / 1_000
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
    .map((row) => ({ ...row, values: { ...row.values, query_duration_ms: activityDurationMs(row), transaction_duration_ms: transactionDurationMs(row) } }))
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

function ActivityView({ cursor, data, finding, focus, locale, onOrder, onPattern, order, pattern, t }: {
  readonly cursor: number
  readonly data: HourData
  readonly finding: Finding | null
  readonly focus: DataRow | null
  readonly locale: Locale
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
    <PgEntityView columns={columns} cursor={cursor} data={data} defaultOrder={ACTIVITY_DEFAULT_ORDER} detailColumns={ACTIVITY_DETAIL_COLUMNS} finding={finding} focus={focus} historyField={null} locale={locale} onOrder={onOrder} onPattern={onPattern} order={activityOrder} pattern={pattern} section="pg_stat_activity" t={t} transformRows={transformRows} />
  </>
}

function PostgresLensBar<L extends string>({ active, choices, onChange, prefix, t }: { readonly active: L; readonly choices: readonly L[]; readonly onChange: (lens: L) => void; readonly prefix: "statement" | "plan"; readonly t: Translate }) {
  return <div className="lensbar pg-lensbar"><span>{t("pg.lens.label")}</span><div className="lens-tabs" role="group" aria-label={t("pg.lens.label")}>{choices.map((choice) => <button aria-pressed={active === choice} data-testid={`${prefix}-lens-${choice}`} key={choice} onClick={() => onChange(choice)} type="button">{t(`pg.lens.${choice}`)}</button>)}</div><div className="value-tone-legend" aria-label={t("pg.value.legend")}><i className="tone-good" />{t("pg.value.good")}<i className="tone-warning" />{t("pg.value.warning")}<i className="tone-critical" />{t("pg.value.critical")}</div></div>
}

function PgPreview({ cursor, data, focus, locale, section, t }: { readonly cursor: number; readonly data: HourData; readonly focus: DataRow | null; readonly locale: Locale; readonly section: string; readonly t: Translate }) {
  const rows = snapshot(data.sections[section] ?? [], cursor)
  return <section className="pg-preview">
    <h2>{section}</h2>
    <EntityTable columns={columnsFor(rows)} empty={t("table.no_rows")} label={section} locale={locale} rows={rows} selectedKey={focus === null ? null : rowKey(focus)} t={t} />
  </section>
}

function PlanInfo({ cursor, data, locale, t }: { readonly cursor: number; readonly data: HourData; readonly locale: Locale; readonly t: Translate }) {
  const row = snapshot(data.sections.pg_store_plans_info ?? [], cursor)[0]
  if (row === undefined) return null
  const dealloc = value(row, "dealloc")
  const reset = value(row, "stats_reset")
  return <section className="pg-overview-section" data-testid="pg-plans-info">
    <h2>pg_store_plans_info</h2>
    <dl>
      <div><dt>{t("pg.field.dealloc.label")}</dt><dd>{dealloc === null ? "—" : measure(dealloc, locale, t("unit.per_second"))}</dd></div>
      <div><dt>{t("pg.field.stats_reset.label")}</dt><dd>{display(reset, timestamp("stats_reset"), locale, t)}</dd></div>
    </dl>
  </section>
}

function Overview({ cursor, data, locale, t }: { readonly cursor: number; readonly data: HourData; readonly locale: Locale; readonly t: Translate }) {
  const activity = snapshot(data.sections.pg_stat_activity ?? [], cursor)
  const databases = snapshot(data.sections.pg_stat_database ?? [], cursor)
  const locks = snapshot(data.sections.pg_locks ?? [], cursor)
  const backends = overviewBackendCounts(activity)
  const databaseCount = postgresDatabaseCount(databases)
  const totals: [string, number][] = []
  if (activity.length !== 0) totals.push(["pg.overview.backends", backends.total], ["pg.overview.active", backends.active], ["pg.overview.idle", backends.idle])
  if (databases.length !== 0) totals.push(["pg.overview.databases", databaseCount])
  if (locks.length !== 0) totals.push(["pg.overview.lock_rows", locks.length])
  const overviewSections = groupSections(data.pgOverview)
  return <section className="pg-overview">
    <div className="overview-metrics">{totals.map(([label, output]) => <article key={label}><span>{t(label)}</span><strong>{measure(output, locale)}</strong></article>)}</div>
    {overviewSections.map(([logicalName, allRows]) => {
      const rows = snapshot(allRows, cursor)
      if (rows.length === 0) return null
      if (rows.length === 1) return <OverviewMetrics key={logicalName} locale={locale} logicalName={logicalName} row={rows[0]!} />
      return <section className="pg-preview" key={logicalName}>
        <h2>{logicalName}</h2>
        <EntityTable columns={columnsFor(rows)} empty={t("table.no_rows")} label={logicalName} locale={locale} rows={rows} t={t} />
      </section>
    })}
    {databases.length !== 0 && <section className="pg-preview"><h2>{t("pg.section.databases")}</h2><EntityTable columns={DATABASE_COLUMNS.slice(0, 9)} empty={t("table.no_rows")} label={t("pg.section.databases")} locale={locale} rows={databases} t={t} /></section>}
  </section>
}

function OverviewMetrics({ locale, logicalName, row }: { readonly locale: Locale; readonly logicalName: string; readonly row: DataRow }) {
  return <section className="pg-overview-section">
    <h2>{logicalName}</h2>
    <dl>{registryCardFields(row).map(([field, cell]) => <div key={field}><dt>{field}</dt><dd>{overviewValue(cell, field, locale)}</dd></div>)}</dl>
  </section>
}

const NO_ROWS: readonly DataRow[] = []
const NO_RATES: readonly string[] = []

function PgEntityView({
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
  transformRows,
}: {
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
  readonly transformRows?: ((rows: readonly DataRow[]) => readonly DataRow[]) | undefined
}) {
  const allRows = data.sections[section] ?? NO_ROWS
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  const rows = useMemo(() => {
    const current = snapshot(allRows, cursor)
    const focused = focus !== null && focus.logicalName === section && !current.some((row) => rowKey(row) === rowKey(focus))
      ? [...current, focus]
      : current
    const transformed = transformRows === undefined ? focused : transformRows(focused)
    return dense ? transformed.map(decoratePostgresIntervalRow) : transformed
  }, [allRows, cursor, dense, focus, section, transformRows])
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
    setSelected((current) => selectedEntity(rows, current, focus, section))
  }, [focus, rows, section])
  const selectedKey = selected === null ? null : rowKey(selected)
  const selectedHistoryField = findingHistoryField(visibleColumns, finding, historyField)
  return <div className={selected === null ? "pg-entity-layout pg-table-only" : "pg-entity-layout"} data-pg-section={sectionName(section)} data-testid="pg-entity-layout">
    <EntityTable columns={visibleColumns} empty={t("table.no_rows")} finding={finding} findingField={finding === null || finding === undefined ? null : fieldNameForLocator(finding)} label={t(`pg.section.${sectionName(section)}`)} locale={locale} onOrder={onOrder} onPattern={onPattern} onSelect={setSelected} order={order ?? defaultOrder} pattern={pattern} serverSorted={dense} rows={rows} selectedKey={selectedKey} t={t} testId={`pg-${sectionName(section)}-table`} />
    {selected !== null && <PgDetail allRows={allRows} columns={visibleDetailColumns} historyField={selectedHistoryField} hour={Math.floor(cursor / 3_600_000_000) * 3_600_000_000} locale={locale} onClose={() => setSelected(null)} row={selected} section={section} t={t} />}
  </div>
}

function visibleEntityColumns(columns: readonly EntityColumn[], rows: readonly DataRow[], rates: readonly string[]): readonly EntityColumn[] {
  return columns.filter((column) => rows.some((row) => Object.hasOwn(row.values, column.field)))
    .map((column) => column.rate === true || rates.includes(column.field) ? { ...column, rate: true } : column)
}

function PgDetail({ allRows, columns, historyField, hour, locale, onClose, row, section, t }: { readonly allRows: readonly DataRow[]; readonly columns: readonly EntityColumn[]; readonly historyField: string | null; readonly hour: number; readonly locale: Locale; readonly onClose: () => void; readonly row: DataRow; readonly section: string; readonly t: Translate }) {
  const identity = identityFields(section, row.typeId)
  const loadedHistory = historyField === null ? [] : allRows.filter((candidate) => candidate.typeId === row.typeId && identity.every((field) => rawText(value(candidate, field)) === rawText(value(row, field)))).map((candidate) => ({
    segmentId: candidate.segmentId,
    timestamp: candidate.timestamp,
    value: asNumber(value(candidate, historyField)),
  }))
  const exactHistory = usePostgresMetricHistory(row, section, historyField, hour)
  const history = exactHistory ?? loadedHistory
  const historyColumn = columns.find((column) => column.field === historyField)
  const textField = section === "pg_store_plans" ? "plan" : "query"
  const exactText = useWholeText(row, section, textField)?.trim() || null
  const fields = columns.filter((column) => column.field !== textField)
  return <aside className="pg-detail" data-testid="pg-detail">
    <header><div><span>{t(`pg.section.${sectionName(section)}`)}</span><h2>{detailTitle(row, section, t)}</h2></div><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    {exactText !== null && <section className="query-block"><span>{t(section === "pg_store_plans" ? "pg.plan.label" : "pg.query.label")}<button aria-label={t("common.raw")} className="copy-raw" onClick={() => void navigator.clipboard?.writeText(exactText)} type="button"><Copy aria-hidden="true" size={12} /></button></span><pre data-testid={section === "pg_store_plans" ? "pg-exact-plan" : "pg-exact-query"}>{exactText}</pre></section>}
    <dl>{fields.filter((column) => told(value(row, column.field))).map((column) => <div key={column.field}><dt>{column.help === undefined ? t(column.label) : <LabelHelp helpKey={column.help} labelKey={column.label} t={t} />}</dt><dd>{display(value(row, column.field), column, locale, t)}</dd></div>)}</dl>
    {historyField !== null && history.some((point) => point.value !== null) && <SeriesChart hour={hour} label={t(historyColumn?.label ?? historyField)} locale={locale} format={chartFormat(historyColumn?.kind)} points={history} />}
  </aside>
}

function usePostgresMetricHistory(row: DataRow, section: string, semantic: string | null, hour: number): readonly ChartPoint[] | null {
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  const [history, setHistory] = useState<readonly ChartPoint[] | null>(dense ? [] : null)
  useEffect(() => {
    if (!dense || semantic === null || !isSemanticField(semantic)) {
      setHistory(dense ? [] : null)
      return
    }
    const fields = historyFields(row.typeId, semantic)
    if (fields.length === 0) {
      setHistory([])
      return
    }
    setHistory([])
    const controller = new AbortController()
    const filters = Object.fromEntries(postgresIdentity(row.typeId).map((name) => [name, rawText(value(row, name)) ?? ""]))
    void loadSeries(hour, section, filters, fields, controller.signal, row.typeId)
      .then((rows) => setHistory(postgresHistory(rows).map((point) => ({
        segmentId: point.segmentId,
        timestamp: point.timestamp,
        value: point[semantic],
      }))))
      .catch(() => {})
    return () => controller.abort()
  }, [dense, hour, row, section, semantic])
  return dense ? history ?? [] : null
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

function isSemanticField(field: string): field is PostgresSemanticField {
  return [
    "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second",
    "planning_ms_per_second", "shared_blk_read_ms_per_second", "shared_blk_write_ms_per_second",
    "local_blk_read_ms_per_second", "local_blk_write_ms_per_second",
    "temp_blk_read_ms_per_second", "temp_blk_write_ms_per_second",
  ].includes(field)
}

function uniqueText(values: readonly (string | null)[]): readonly string[] {
  return [...new Set(values.filter((field): field is string => field !== null))]
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
    void loadSnapshot(
      row.segmentId,
      row.timestamp,
      [{ section, fields: [field], typeId: row.typeId }],
      controller.signal,
      undefined,
      { filters, typeId: row.typeId, fullText: true },
    )
      .then((data) => {
        const text = rawText(value(data.sections[section]?.[0] ?? row, field))
        if (text !== null) setWhole(text)
      })
      .catch(() => {})
    return () => controller.abort()
  }, [field, row, section, shown])
  return whole ?? shown
}

function told(cell: ReturnType<typeof value>): boolean {
  if (cell === null) return false
  return rawText(cell)?.trim() !== ""
}

export function sameEntity(left: DataRow, right: DataRow, section: string): boolean {
  return left.typeId === right.typeId && identityFields(section, left.typeId).every((field) => rawText(value(left, field)) === rawText(value(right, field)))
}

export function selectedEntity(rows: readonly DataRow[], current: DataRow | null, focus: DataRow | null, section: string): DataRow | null {
  if (focus !== null) {
    const exact = rows.find((row) => rowKey(row) === rowKey(focus))
    if (exact !== undefined) return exact
  }
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
  if (section === "pg_stat_activity" || section === "pg_locks") return ["pid"]
  if ((section === "pg_stat_statements" || section === "pg_store_plans") && typeId !== undefined) return postgresIdentity(typeId)
  return ["datid"]
}

function detailTitle(row: DataRow, section: string, t: Translate): string {
  if (section === "pg_stat_activity" || section === "pg_locks") return t("pg.detail.pid", { pid: identifier(value(row, "pid")) })
  if (section === "pg_stat_statements") return t("pg.detail.query", { id: identifier(value(row, "queryid")) })
  if (section === "pg_store_plans") return t("pg.detail.plan", { id: identifier(value(row, "planid")) })
  return rawText(value(row, "datname")) ?? identifier(value(row, "datid"))
}

function display(cell: ReturnType<typeof value>, column: EntityColumn, locale: Locale, t: Translate): ReactNode {
  if (cell === null) return "—"
  if (column.kind === "timestamp") {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : <TimestampValue t={t} timestamp={timestamp} />
  }
  if (column.kind === "id") return rawText(cell) ?? "—"
  const per = t("unit.per_second")
  if (column.kind === "bytes") return unit(humanBytes(cell, locale), column.rate, per)
  if (column.kind === "kib") return unit(humanBytes(asNumber(cell) === null ? null : (asNumber(cell) ?? 0) * 1024, locale), column.rate, per)
  if (column.kind === "milliseconds") return measure(cell, locale, unit(t("unit.ms"), column.rate, per))
  if (column.kind === "duration") return humanDuration(cell, locale)
  if (column.kind === "microseconds") return measure(cell, locale, unit(t("unit.us"), column.rate, per))
  if (column.kind === "percent") return measure(cell, locale, unit("%", column.rate, per))
  if (column.kind === "boolean" && typeof cell === "boolean") return locale === "ru" ? cell ? "да" : "нет" : String(cell)
  if (typeof cell === "number") return measure(cell, locale, unit("", column.rate, per))
  return rawText(cell) ?? "—"
}

function TimestampValue({ t, timestamp }: { readonly t: Translate; readonly timestamp: number }) {
  return <span className="timestamp-value"><span>{formatUtc(timestamp)}</span><button aria-label={t("common.raw")} onClick={() => void navigator.clipboard?.writeText(String(timestamp))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
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

export function overviewValue(cell: ReturnType<typeof value>, field: string, locale: Locale): string {
  if (cell === null) return "—"
  if (isTimestampField(field)) {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : formatUtc(timestamp)
  }
  if (field === "pid" || field.endsWith("id") || field.endsWith("_id")) return rawText(cell) ?? "—"
  if (field.endsWith("_time")) return measure(cell, locale, " ms")
  if (field.endsWith("_us")) return measure(cell, locale, " μs")
  if (field.endsWith("_bytes")) return measure(cell, locale, " B")
  if (typeof cell === "boolean") return locale === "ru" ? cell ? "да" : "нет" : String(cell)
  if (typeof cell === "number") return measure(cell, locale)
  return rawText(cell) ?? "—"
}

function rowKey(row: DataRow): string { return `${row.segmentId}:${row.typeId}:${row.ordinal}` }
export function isTimestampField(field: string): boolean {
  return field.endsWith("_start") || field === "state_change" || field === "waitstart" || field === "stats_reset" || field === "stats_since"
    || field === "last_archived_time" || field === "last_failed_time"
}
function findingHistoryField(columns: readonly EntityColumn[], finding: Finding | null | undefined, fallback: string | null): string | null {
  const field = finding === null || finding === undefined ? null : fieldNameForLocator(finding)
  const semantic = finding === null || finding === undefined || field === null ? null : findingSemanticField(finding.typeId, field)
  const column = columns.find((candidate) => candidate.field === (semantic ?? field))
  return column === undefined || column.kind === "text" || column.kind === "timestamp" || column.kind === "boolean" ? fallback : column.field
}
function chartFormat(kind: EntityColumn["kind"]): ((value: number, locale: Locale) => string) | undefined {
  if (kind === "bytes") return (value, locale) => humanBytes(value, locale)
  if (kind === "kib") return (value, locale) => humanBytes(value * 1024, locale)
  if (kind === "microseconds") return (value, locale) => measure(value / 1_000, locale, " ms")
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
function pgColumn(field: string, kind: NonNullable<EntityColumn["kind"]>, width: number, sticky = false): EntityColumn {
  return { field, label: `pg.field.${field}.label`, help: `pg.field.${field}.help`, kind, width, sticky }
}
function text(field: string, width = 130, sticky = false): EntityColumn { return pgColumn(field, "text", width, sticky) }
function number(field: string, width = 125): EntityColumn { return pgColumn(field, "number", width) }
function id(field: string, width = 110, sticky = false): EntityColumn { return pgColumn(field, "id", width, sticky) }
function bytes(field: string, width = 140): EntityColumn { return pgColumn(field, "bytes", width) }
function milliseconds(field: string, width = 145): EntityColumn { return pgColumn(field, "milliseconds", width) }
function duration(field: string, width = 145): EntityColumn { return pgColumn(field, "duration", width) }
function percent(field: string, width = 125): EntityColumn { return pgColumn(field, "percent", width) }
function rateNumber(field: string, width = 125): EntityColumn { return { ...number(field, width), rate: true, sortable: true } }
function rateBytes(field: string, width = 140): EntityColumn { return { ...bytes(field, width), rate: true, sortable: true } }
function rateMilliseconds(field: string, width = 145): EntityColumn { return { ...milliseconds(field, width), rate: true, sortable: true } }
function timestamp(field: string, width = 210): EntityColumn { return pgColumn(field, "timestamp", width) }
function boolean(field: string, width = 125): EntityColumn { return pgColumn(field, "boolean", width) }
function pgText(field: string, key: string, width = 130, sticky = false): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "text", width, sticky } }
function pgNumber(field: string, key: string, width = 125): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "number", width } }
function pgId(field: string, key: string, width = 110, sticky = false): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "id", width, sticky } }
function pgTimestamp(field: string, key: string, width = 210): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "timestamp", width } }
