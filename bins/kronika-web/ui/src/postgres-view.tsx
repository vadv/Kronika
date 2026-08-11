import { Activity, BarChart3, Database, KeyRound, LockKeyhole, X } from "lucide-react"
import { useEffect, useMemo, useState, type ReactNode } from "react"

import type { DataRow, Finding, HourData } from "./api"
import { EntityTable, type EntityColumn } from "./entity-table"
import type { Translate } from "./help"
import { asNumber, formatUtc, identifier, measure, rawText, snapshot, value, type Locale } from "./model"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { Timeline } from "./timeline"

export type PostgresSection = "overview" | "activity" | "statements" | "locks" | "databases"

const ACTIVITY_COLUMNS: readonly EntityColumn[] = [
  id("pid", 78, true), text("backend_type", 150, true), text("datname", 145), text("usename", 130),
  text("application_name", 180), text("client_addr", 150), text("state", 110), text("wait_event_type", 135),
  text("wait_event", 155), id("query_id", 150), number("backend_xid_age", 145), number("backend_xmin_age", 145),
  timestamp("backend_start", 210), timestamp("xact_start", 210), timestamp("query_start", 210), timestamp("state_change", 210),
  text("query", 420),
]

const STATEMENT_COLUMNS: readonly EntityColumn[] = [
  id("queryid", 155, true), text("datname", 145, true), text("usename", 130), text("query", 440),
  number("calls"), milliseconds("total_exec_time", 155), milliseconds("mean_exec_time", 155), milliseconds("max_exec_time", 145),
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

const TABS: readonly { readonly id: PostgresSection; readonly icon: ReactNode; readonly section?: string }[] = [
  { id: "overview", icon: <BarChart3 size={13} /> },
  { id: "activity", icon: <Activity size={13} />, section: "pg_stat_activity" },
  { id: "statements", icon: <KeyRound size={13} />, section: "pg_stat_statements" },
  { id: "locks", icon: <LockKeyhole size={13} />, section: "pg_locks" },
  { id: "databases", icon: <Database size={13} />, section: "pg_stat_database" },
]

export function PostgresView({
  cursor,
  data,
  focus,
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
    if (tab?.section === undefined || available(tab.section)) return
    onSection("overview")
  }, [data.sections, onSection, section])
  return <>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={data.load} memory={data.memory} onCursor={onCursor} onFinding={onFinding} pressure={data.pressure} t={t} />
    <nav aria-label={t("pg.sections")} className="pg-tabs">
      {TABS.map((tab) => {
        const enabled = tab.section === undefined || available(tab.section)
        return <button aria-current={section === tab.id ? "page" : undefined} disabled={!enabled} key={tab.id} onClick={() => onSection(tab.id)} title={enabled ? undefined : t("pg.no_section_data")} type="button">{tab.icon}<span>{t(`pg.section.${tab.id}`)}</span></button>
      })}
    </nav>
    {section === "overview" && <Overview cursor={cursor} data={data} hour={hour} locale={locale} t={t} />}
    {section === "activity" && <PgEntityView columns={ACTIVITY_COLUMNS} cursor={cursor} data={data} focus={focus} historyField="backend_xid_age" locale={locale} section="pg_stat_activity" t={t} />}
    {section === "statements" && <PgEntityView columns={STATEMENT_COLUMNS} cursor={cursor} data={data} focus={focus} historyField="calls" locale={locale} section="pg_stat_statements" t={t} />}
    {section === "locks" && <PgEntityView columns={LOCK_COLUMNS} cursor={cursor} data={data} focus={focus} historyField="backend_xid_age" locale={locale} section="pg_locks" t={t} />}
    {section === "databases" && <PgEntityView columns={DATABASE_COLUMNS} cursor={cursor} data={data} focus={focus} historyField="xact_commit" locale={locale} section="pg_stat_database" t={t} />}
  </>
}

function Overview({ cursor, data, hour, locale, t }: { readonly cursor: number; readonly data: HourData; readonly hour: number; readonly locale: Locale; readonly t: Translate }) {
  const activity = snapshot(data.sections.pg_stat_activity ?? [], cursor)
  const databases = snapshot(data.sections.pg_stat_database ?? [], cursor)
  const statements = snapshot(data.sections.pg_stat_statements ?? [], cursor)
  const locks = snapshot(data.sections.pg_locks ?? [], cursor)
  const active = activity.filter((row) => rawText(value(row, "state")) === "active").length
  const waiting = activity.filter((row) => rawText(value(row, "wait_event")) !== null).length
  const totals = [
    ["pg.overview.backends", activity.length], ["pg.overview.active", active], ["pg.overview.waiting", waiting],
    ["pg.overview.databases", databases.length], ["pg.overview.statements", statements.length], ["pg.overview.lock_rows", locks.length],
  ] as const
  const history = countHistory(data.sections.pg_stat_activity ?? [])
  const overviewSections = groupSections(data.pgOverview)
  return <section className="pg-overview">
    <div className="overview-metrics">{totals.map(([label, output]) => <article key={label}><span>{t(label)}</span><strong>{measure(output, locale)}</strong></article>)}</div>
    <SeriesChart hour={hour} label={t("pg.overview.backend_history")} locale={locale} points={history} unit="" />
    {overviewSections.map(([logicalName, allRows]) => {
      const rows = snapshot(allRows, cursor)
      if (rows.length === 0) return null
      if (rows.length === 1) return <OverviewMetrics key={logicalName} locale={locale} logicalName={logicalName} row={rows[0]!} />
      return <section className="pg-preview" key={logicalName}>
        <h2>{logicalName}</h2>
        <EntityTable columns={columnsFor(rows)} empty={t("table.no_rows")} label={logicalName} locale={locale} rows={rows} />
      </section>
    })}
    {databases.length !== 0 && <section className="pg-preview"><h2>{t("pg.section.databases")}</h2><EntityTable columns={DATABASE_COLUMNS.slice(0, 9)} empty={t("table.no_rows")} label={t("pg.section.databases")} locale={locale} rows={databases} /></section>}
  </section>
}

function OverviewMetrics({ locale, logicalName, row }: { readonly locale: Locale; readonly logicalName: string; readonly row: DataRow }) {
  return <section className="pg-overview-section">
    <h2>{logicalName}</h2>
    <dl>{Object.entries(row.values).map(([field, cell]) => <div key={field}><dt>{field}</dt><dd>{overviewValue(cell, field, locale)}</dd></div>)}</dl>
  </section>
}

function PgEntityView({
  columns,
  cursor,
  data,
  focus,
  historyField,
  locale,
  section,
  t,
}: {
  readonly columns: readonly EntityColumn[]
  readonly cursor: number
  readonly data: HourData
  readonly focus: DataRow | null
  readonly historyField: string
  readonly locale: Locale
  readonly section: string
  readonly t: Translate
}) {
  const allRows = data.sections[section] ?? []
  const rows = useMemo(() => snapshot(allRows, cursor), [allRows, cursor])
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [selected, setSelected] = useState<DataRow | null>(null)
  useEffect(() => {
    if (focus === null || !allRows.some((row) => rowKey(row) === rowKey(focus))) return
    setSelected(focus)
    setSelectedKey(rowKey(focus))
  }, [allRows, focus])
  useEffect(() => {
    if (selected !== null && rows.some((row) => sameEntity(row, selected, section))) {
      const current = rows.find((row) => sameEntity(row, selected, section)) ?? selected
      setSelected(current)
      setSelectedKey(rowKey(current))
      return
    }
    const first = rows[0] ?? null
    setSelected(first)
    setSelectedKey(first === null ? null : rowKey(first))
  // Intentional: cursor/rows advance the selected entity to its current row.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, section])
  return <div className={selected === null ? "pg-entity-layout pg-table-only" : "pg-entity-layout"}>
    <EntityTable columns={columns} empty={t("table.no_rows")} label={t(`pg.section.${sectionName(section)}`)} locale={locale} onSelect={(row) => { setSelected(row); setSelectedKey(rowKey(row)) }} rows={rows} selectedKey={selectedKey} testId={`pg-${sectionName(section)}-table`} />
    {selected !== null && <PgDetail allRows={allRows} historyField={historyField} hour={Math.floor(cursor / 3_600_000_000) * 3_600_000_000} locale={locale} onClose={() => { setSelected(null); setSelectedKey(null) }} row={selected} section={section} t={t} />}
  </div>
}

function PgDetail({ allRows, historyField, hour, locale, onClose, row, section, t }: { readonly allRows: readonly DataRow[]; readonly historyField: string; readonly hour: number; readonly locale: Locale; readonly onClose: () => void; readonly row: DataRow; readonly section: string; readonly t: Translate }) {
  const identity = identityFields(section)
  const history = allRows.filter((candidate) => identity.every((field) => rawText(value(candidate, field)) === rawText(value(row, field)))).map((candidate) => ({
    segmentId: candidate.segmentId,
    timestamp: candidate.timestamp,
    value: asNumber(value(candidate, historyField)),
  }))
  const query = rawText(value(row, "query"))
  const fields = detailFields(section).filter((field) => field !== "query")
  return <aside className="pg-detail">
    <header><div><span>{t(`pg.section.${sectionName(section)}`)}</span><h2>{detailTitle(row, section, t)}</h2></div><button aria-label={t("common.close")} onClick={onClose} type="button"><X size={14} /></button></header>
    <dl>{fields.map((field) => <div key={field}><dt>{field}</dt><dd>{display(value(row, field), field, locale, t)}</dd></div>)}</dl>
    {query !== null && <section className="query-block"><span>{t("pg.query.label")}<button className="copy-raw" onClick={() => void navigator.clipboard?.writeText(query)} type="button">{t("common.raw")}</button></span><pre data-testid="pg-exact-query">{query}</pre></section>}
    <SeriesChart hour={hour} label={historyField} locale={locale} points={history} unit="" />
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

function sameEntity(left: DataRow, right: DataRow, section: string): boolean {
  return identityFields(section).every((field) => rawText(value(left, field)) === rawText(value(right, field)))
}

function identityFields(section: string): readonly string[] {
  if (section === "pg_stat_activity" || section === "pg_locks") return ["pid"]
  if (section === "pg_stat_statements") return ["queryid", "userid", "dbid"]
  return ["datid"]
}

function detailFields(section: string): readonly string[] {
  if (section === "pg_stat_activity") return ACTIVITY_COLUMNS.map((column) => column.field)
  if (section === "pg_stat_statements") return STATEMENT_COLUMNS.map((column) => column.field)
  if (section === "pg_locks") return LOCK_COLUMNS.map((column) => column.field)
  return DATABASE_COLUMNS.map((column) => column.field)
}

function detailTitle(row: DataRow, section: string, t: Translate): string {
  if (section === "pg_stat_activity" || section === "pg_locks") return t("pg.detail.pid", { pid: identifier(value(row, "pid")) })
  if (section === "pg_stat_statements") return t("pg.detail.query", { id: identifier(value(row, "queryid")) })
  return rawText(value(row, "datname")) ?? identifier(value(row, "datid"))
}

function display(cell: ReturnType<typeof value>, field: string, locale: Locale, t: Translate): ReactNode {
  if (cell === null) return "—"
  if (field.endsWith("_start") || field === "state_change" || field === "waitstart" || field === "stats_since") {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : <TimestampValue t={t} timestamp={timestamp} />
  }
  if (field === "pid" || field.endsWith("id") || field.endsWith("_id") || field === "blocked_by") return rawText(cell) ?? "—"
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

function columnsFor(rows: readonly DataRow[]): readonly EntityColumn[] {
  const fields = [...new Set(rows.flatMap((row) => Object.keys(row.values)))]
  return fields.map((field, index) => {
    const sample = rows.find((row) => value(row, field) !== null)
    const cell = sample === undefined ? null : value(sample, field)
    const kind: EntityColumn["kind"] = field === "pid" || field.endsWith("id") || field.endsWith("_id")
      ? "id"
      : field.endsWith("_start") || field.endsWith("_time") || field === "stats_reset"
        ? "timestamp"
        : typeof cell === "number" ? "number" : typeof cell === "boolean" ? "boolean" : "text"
    return { field, label: field, kind, sticky: index === 0, width: kind === "text" ? 190 : kind === "timestamp" ? 210 : 135 }
  })
}

function overviewValue(cell: ReturnType<typeof value>, field: string, locale: Locale): string {
  if (cell === null) return "—"
  if (field.endsWith("_time") || field.endsWith("_start") || field === "stats_reset") {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : formatUtc(timestamp)
  }
  if (field === "pid" || field.endsWith("id") || field.endsWith("_id")) return rawText(cell) ?? "—"
  if (typeof cell === "number") return measure(cell, locale)
  return rawText(cell) ?? "—"
}

function rowKey(row: DataRow): string { return `${row.segmentId}:${row.typeId}:${row.ordinal}` }
function sectionName(section: string): PostgresSection {
  if (section === "pg_stat_activity") return "activity"
  if (section === "pg_stat_statements") return "statements"
  if (section === "pg_locks") return "locks"
  return "databases"
}
function text(field: string, width = 130, sticky = false): EntityColumn { return { field, label: field, kind: "text", width, sticky } }
function number(field: string, width = 125): EntityColumn { return { field, label: field, kind: "number", width } }
function id(field: string, width = 110, sticky = false): EntityColumn { return { field, label: field, kind: "id", width, sticky } }
function bytes(field: string, width = 140): EntityColumn { return { field, label: field, kind: "bytes", width } }
function milliseconds(field: string, width = 145): EntityColumn { return { field, label: field, kind: "milliseconds", width } }
function timestamp(field: string, width = 210): EntityColumn { return { field, label: field, kind: "timestamp", width } }
