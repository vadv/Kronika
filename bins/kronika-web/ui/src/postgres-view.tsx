import { Activity, BarChart3, Database, KeyRound, LockKeyhole, X } from "lucide-react"
import { useEffect, useMemo, useState, type ReactNode } from "react"

import type { DataRow, Finding, HourData } from "./api"
import { EntityTable, type EntityColumn } from "./entity-table"
import type { Translate } from "./help"
import { fieldNameForLocator } from "./api"
import { LabelHelp } from "./help"
import { asNumber, formatUtc, humanBytes, identifier, measure, rawText, snapshot, value, type Locale } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

export type PostgresSection = "overview" | "activity" | "statements" | "locks" | "databases"

export const ACTIVITY_COLUMNS: readonly EntityColumn[] = [
  pgId("pid", "pg.field.pid", 78, true), pgId("leader_pid", "pg.leader_pid", 105), pgText("backend_type", "pg.backend_type", 150, true), pgText("datname", "pg.datname", 145), pgText("usename", "pg.usename", 130),
  pgText("application_name", "pg.application_name", 180), pgText("client_addr", "pg.client_addr", 150), pgText("state", "pg.state", 110), pgText("wait_event_type", "pg.wait_event_type", 135),
  pgText("wait_event", "pg.wait_event", 155), pgId("query_id", "pg.query_id", 150), pgNumber("backend_xid_age", "pg.backend_xid_age", 145), pgNumber("backend_xmin_age", "pg.backend_xmin_age", 145),
  pgTimestamp("backend_start", "pg.backend_start", 210), pgTimestamp("xact_start", "pg.xact_start", 210), pgTimestamp("query_start", "pg.query_start", 210), pgTimestamp("state_change", "pg.state_change", 210),
  pgText("query", "pg.query", 420),
]

export const STATEMENT_COLUMNS: readonly EntityColumn[] = [
  id("queryid", 155, true), id("dbid", 120), id("userid", 120), boolean("toplevel", 105), text("datname", 145, true), text("usename", 130), text("query", 440),
  number("calls"), legacyMilliseconds("total_time", "total_exec_time", 155), legacyMilliseconds("mean_time", "mean_exec_time", 155), legacyMilliseconds("max_time", "max_exec_time", 145),
  milliseconds("total_exec_time", 155), milliseconds("mean_exec_time", 155), milliseconds("max_exec_time", 145),
  number("rows"), number("shared_blks_hit", 145), number("shared_blks_read", 145), number("shared_blks_written", 155),
  number("temp_blks_read", 145), number("temp_blks_written", 155), bytes("wal_bytes", 145), number("wal_records", 135),
  number("plans"), milliseconds("total_plan_time", 155), timestamp("stats_since", 210),
]

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
  { id: "locks", icon: <LockKeyhole size={13} />, sections: ["pg_locks"] },
  { id: "databases", icon: <Database size={13} />, sections: ["pg_stat_database"] },
]

export function PostgresView({
  cursor,
  data,
  focus,
  focusFinding,
  hour,
  locale,
  onCursor,
  onFinding,
  onSection,
  section,
  t,
}: {
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
  readonly t: Translate
}) {
  const available = (name: string) => (data.sections[name] ?? []).length !== 0
  useEffect(() => {
    const tab = TABS.find((candidate) => candidate.id === section)
    if (tab?.sections === undefined || tab.sections.some(available)) return
    onSection("overview")
  }, [data.sections, onSection, section])
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={data.load} memory={data.memory} onCursor={onCursor} onFinding={onFinding} pressure={data.pressure} t={t} />
    <nav aria-label={t("pg.sections")} className="pg-tabs">
      {TABS.map((tab) => {
        const enabled = tab.sections === undefined || tab.sections.some(available)
        return <button aria-current={section === tab.id ? "page" : undefined} disabled={!enabled} key={tab.id} onClick={() => onSection(tab.id)} title={enabled ? undefined : t("pg.no_section_data")} type="button">{tab.icon}<span>{t(`pg.section.${tab.id}`)}</span></button>
      })}
    </nav>
    {section === "overview" && <Overview cursor={cursor} data={data} hour={hour} locale={locale} t={t} />}
    {section === "activity" && available("pg_stat_activity") && <PgEntityView columns={ACTIVITY_COLUMNS} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_activity" ? focusFinding : null} focus={focus} historyField="backend_xid_age" locale={locale} section="pg_stat_activity" t={t} />}
    {section === "activity" && available("pg_stat_progress_vacuum") && <PgPreview cursor={cursor} data={data} focus={focusFinding?.logicalName === "pg_stat_progress_vacuum" ? focus : null} locale={locale} section="pg_stat_progress_vacuum" t={t} />}
    {section === "statements" && <PgEntityView columns={STATEMENT_COLUMNS} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_statements" ? focusFinding : null} focus={focus} historyField="calls" locale={locale} section="pg_stat_statements" t={t} />}
    {section === "locks" && <PgEntityView columns={LOCK_COLUMNS} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_locks" ? focusFinding : null} focus={focus} historyField={null} locale={locale} section="pg_locks" t={t} />}
    {section === "databases" && <PgEntityView columns={DATABASE_COLUMNS} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_database" ? focusFinding : null} focus={focus} historyField="xact_commit" locale={locale} section="pg_stat_database" t={t} />}
  </>
}

function PgPreview({ cursor, data, focus, locale, section, t }: { readonly cursor: number; readonly data: HourData; readonly focus: DataRow | null; readonly locale: Locale; readonly section: string; readonly t: Translate }) {
  const rows = snapshot(data.sections[section] ?? [], cursor)
  return <section className="pg-preview">
    <h2>{section}</h2>
    <EntityTable columns={columnsFor(rows)} empty={t("table.no_rows")} label={section} locale={locale} rows={rows} selectedKey={focus === null ? null : rowKey(focus)} t={t} />
  </section>
}

function Overview({ cursor, data, hour, locale, t }: { readonly cursor: number; readonly data: HourData; readonly hour: number; readonly locale: Locale; readonly t: Translate }) {
  const activity = snapshot(data.sections.pg_stat_activity ?? [], cursor)
  const databases = snapshot(data.sections.pg_stat_database ?? [], cursor)
  const statements = snapshot(data.sections.pg_stat_statements ?? [], cursor)
  const locks = snapshot(data.sections.pg_locks ?? [], cursor)
  const active = activity.filter((row) => rawText(value(row, "state")) === "active").length
  const waiting = activity.filter((row) => rawText(value(row, "wait_event")) !== null).length
  const databaseCount = postgresDatabaseCount(databases)
  const totals: [string, number][] = []
  if (activity.length !== 0) totals.push(["pg.overview.backends", activity.length], ["pg.overview.active", active], ["pg.overview.waiting", waiting])
  if (databases.length !== 0) totals.push(["pg.overview.databases", databaseCount])
  if (statements.length !== 0) totals.push(["pg.overview.statements", statements.length])
  if (locks.length !== 0) totals.push(["pg.overview.lock_rows", locks.length])
  const history = countHistory(data.sections.pg_stat_activity ?? [])
  const overviewSections = groupSections(data.pgOverview)
  return <section className="pg-overview">
    <div className="overview-metrics">{totals.map(([label, output]) => <article key={label}><span>{t(label)}</span><strong>{measure(output, locale)}</strong></article>)}</div>
    {history.length !== 0 && <SeriesChart hour={hour} label={t("pg.overview.backend_history")} locale={locale} points={history} />}
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
    <dl>{Object.entries(row.values).map(([field, cell]) => <div key={field}><dt>{field}</dt><dd>{overviewValue(cell, field, locale)}</dd></div>)}</dl>
  </section>
}

/** One empty array for the whole module: `?? []` is a new reference every
 *  render, and every memo downstream of it recomputes. */
const NO_ROWS: readonly DataRow[] = []

function PgEntityView({
  columns,
  cursor,
  data,
  focus,
  finding,
  historyField,
  locale,
  section,
  t,
}: {
  readonly columns: readonly EntityColumn[]
  readonly cursor: number
  readonly data: HourData
  readonly focus: DataRow | null
  readonly finding?: Finding | null
  readonly historyField: string | null
  readonly locale: Locale
  readonly section: string
  readonly t: Translate
}) {
  const allRows = data.sections[section] ?? NO_ROWS
  const rows = useMemo(() => snapshot(allRows, cursor), [allRows, cursor])
  const visibleColumns = useMemo(() => columns.filter((column) => allRows.some((row) => Object.hasOwn(row.values, column.field))), [allRows, columns])
  const [selected, setSelected] = useState<DataRow | null>(null)
  useEffect(() => {
    setSelected((current) => selectedEntity(rows, current, focus, section))
  }, [focus, rows, section])
  const selectedKey = selected === null ? null : rowKey(selected)
  const selectedHistoryField = findingHistoryField(visibleColumns, finding, historyField)
  return <div className={selected === null ? "pg-entity-layout pg-table-only" : "pg-entity-layout"} data-pg-section={sectionName(section)} data-testid="pg-entity-layout">
    <EntityTable columns={visibleColumns} empty={t("table.no_rows")} label={t(`pg.section.${sectionName(section)}`)} locale={locale} onSelect={setSelected} rows={rows} selectedKey={selectedKey} t={t} testId={`pg-${sectionName(section)}-table`} />
    {selected !== null && <PgDetail allRows={allRows} columns={visibleColumns} historyField={selectedHistoryField} hour={Math.floor(cursor / 3_600_000_000) * 3_600_000_000} locale={locale} onClose={() => setSelected(null)} row={selected} section={section} t={t} />}
  </div>
}

function PgDetail({ allRows, columns, historyField, hour, locale, onClose, row, section, t }: { readonly allRows: readonly DataRow[]; readonly columns: readonly EntityColumn[]; readonly historyField: string | null; readonly hour: number; readonly locale: Locale; readonly onClose: () => void; readonly row: DataRow; readonly section: string; readonly t: Translate }) {
  const identity = identityFields(section)
  const history = historyField === null ? [] : allRows.filter((candidate) => candidate.typeId === row.typeId && identity.every((field) => rawText(value(candidate, field)) === rawText(value(row, field)))).map((candidate) => ({
    segmentId: candidate.segmentId,
    timestamp: candidate.timestamp,
    value: asNumber(value(candidate, historyField)),
  }))
  const historyColumn = columns.find((column) => column.field === historyField)
  const query = rawText(value(row, "query"))
  const fields = columns.filter((column) => column.field !== "query")
  return <aside className="pg-detail" data-testid="pg-detail">
    <header><div><span>{t(`pg.section.${sectionName(section)}`)}</span><h2>{detailTitle(row, section, t)}</h2></div><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    <dl>{fields.map((column) => <div key={column.field}><dt>{column.help === undefined ? t(column.label) : <LabelHelp helpKey={column.help} labelKey={column.label} t={t} />}</dt><dd>{display(value(row, column.field), column, locale, t)}</dd></div>)}</dl>
    {query !== null && <section className="query-block"><span>{t("pg.query.label")}<button className="copy-raw" onClick={() => void navigator.clipboard?.writeText(query)} type="button">{t("common.raw")}</button></span><pre data-testid="pg-exact-query">{query}</pre></section>}
    {historyField !== null && <SeriesChart hour={hour} label={t(historyColumn?.label ?? historyField)} locale={locale} format={chartFormat(historyColumn?.kind)} points={history} />}
  </aside>
}

function countHistory(rows: readonly DataRow[]): readonly ChartPoint[] {
  const counts = new Map<string, ChartPoint>()
  for (const row of rows) {
    const key = `${row.segmentId}:${row.timestamp}`
    const current = counts.get(key)
    counts.set(key, { segmentId: row.segmentId, timestamp: row.timestamp, value: (current?.value ?? 0) + 1 })
  }
  return [...counts.values()]
}

export function sameEntity(left: DataRow, right: DataRow, section: string): boolean {
  return left.typeId === right.typeId && identityFields(section).every((field) => rawText(value(left, field)) === rawText(value(right, field)))
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
  return rows[0] ?? null
}

export function postgresDatabaseCount(rows: readonly DataRow[]): number {
  return rows.filter((row) => asNumber(value(row, "datid")) !== 0).length
}

function identityFields(section: string): readonly string[] {
  if (section === "pg_stat_activity" || section === "pg_locks") return ["pid"]
  if (section === "pg_stat_statements") return ["queryid", "userid", "dbid", "toplevel"]
  return ["datid"]
}

function detailTitle(row: DataRow, section: string, t: Translate): string {
  if (section === "pg_stat_activity" || section === "pg_locks") return t("pg.detail.pid", { pid: identifier(value(row, "pid")) })
  if (section === "pg_stat_statements") return t("pg.detail.query", { id: identifier(value(row, "queryid")) })
  return rawText(value(row, "datname")) ?? identifier(value(row, "datid"))
}

function display(cell: ReturnType<typeof value>, column: EntityColumn, locale: Locale, t: Translate): ReactNode {
  if (cell === null) return "—"
  if (column.kind === "timestamp") {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : <TimestampValue t={t} timestamp={timestamp} />
  }
  if (column.kind === "id") return rawText(cell) ?? "—"
  if (column.kind === "bytes") return humanBytes(cell, locale)
  if (column.kind === "kib") return humanBytes(asNumber(cell) === null ? null : (asNumber(cell) ?? 0) * 1024, locale)
  if (column.kind === "milliseconds") return measure(cell, locale, " ms")
  if (column.kind === "microseconds") return measure(cell, locale, " μs")
  if (column.kind === "percent") return measure(cell, locale, "%")
  if (column.kind === "boolean" && typeof cell === "boolean") return locale === "ru" ? cell ? "да" : "нет" : String(cell)
  if (typeof cell === "number") return measure(cell, locale)
  return rawText(cell) ?? "—"
}

function TimestampValue({ t, timestamp }: { readonly t: Translate; readonly timestamp: number }) {
  return <span className="timestamp-value"><span>{formatUtc(timestamp)}</span><button aria-label={t("common.raw")} onClick={() => void navigator.clipboard?.writeText(String(timestamp))} type="button">{t("common.raw")}</button></span>
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
  const fields = [...new Set(rows.flatMap((row) => Object.keys(row.values)))]
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
  const column = columns.find((candidate) => candidate.field === field)
  return column === undefined || column.kind === "text" || column.kind === "timestamp" || column.kind === "boolean" ? fallback : column.field
}
function chartFormat(kind: EntityColumn["kind"]): ((value: number, locale: Locale) => string) | undefined {
  if (kind === "bytes") return (value, locale) => humanBytes(value, locale)
  if (kind === "kib") return (value, locale) => humanBytes(value * 1024, locale)
  if (kind === "microseconds") return (value, locale) => measure(value / 1_000, locale, " ms")
  return undefined
}
function sectionName(section: string): PostgresSection {
  if (section === "pg_stat_activity") return "activity"
  if (section === "pg_stat_statements") return "statements"
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
function legacyMilliseconds(field: string, labelField: string, width: number): EntityColumn {
  return { field, label: `pg.field.${labelField}.label`, help: `pg.field.${labelField}.help`, kind: "milliseconds", width }
}
function timestamp(field: string, width = 210): EntityColumn { return pgColumn(field, "timestamp", width) }
function boolean(field: string, width = 125): EntityColumn { return pgColumn(field, "boolean", width) }
function pgText(field: string, key: string, width = 130, sticky = false): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "text", width, sticky } }
function pgNumber(field: string, key: string, width = 125): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "number", width } }
function pgId(field: string, key: string, width = 110, sticky = false): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "id", width, sticky } }
function pgTimestamp(field: string, key: string, width = 210): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "timestamp", width } }
