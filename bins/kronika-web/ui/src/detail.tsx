import { Copy, X } from "lucide-react"
import { useMemo, type ReactNode } from "react"

import type { Cell, DataRow } from "./api"
import { LabelHelp, type Translate } from "./help"
import {
  asNumber,
  formatUtc,
  humanBytes,
  identifier,
  measure,
  millisecondsPerSecond,
  processCommand,
  rawText,
  stateText,
  value,
  type Lens,
  type Locale,
} from "./model"
import { CellValue, formatCell, LENS_FIELDS, type Field } from "./process-table"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { TimeTicks } from "./time-ticks"

interface HistoryField {
  readonly field: string
  readonly key: string
  readonly kind: Field["kind"]
  readonly counter?: true
}

export interface ProcessHistorySeries extends HistoryField {
  readonly points: readonly ChartPoint[]
}

const PROCESS_HISTORY: Readonly<Record<Lens, readonly HistoryField[]>> = {
  generic: [{ field: "num_threads", key: "col.threads", kind: "number" }],
  cpu: [
    { counter: true, field: "utime", key: "col.utime", kind: "cores" },
    { counter: true, field: "stime", key: "col.stime", kind: "cores" },
    { counter: true, field: "rundelay_ns", key: "col.rundelay", kind: "ns" },
    { counter: true, field: "blkdelay_ticks", key: "col.blkdelay", kind: "rate" },
    { counter: true, field: "nvcsw", key: "col.nvcsw", kind: "rate" },
    { counter: true, field: "nivcsw", key: "col.nivcsw", kind: "rate" },
    { counter: true, field: "minflt", key: "col.minflt", kind: "rate" },
    { counter: true, field: "majflt", key: "col.majflt", kind: "rate" },
  ],
  memory: [
    { field: "rmem_kb", key: "col.rmem", kind: "kib" },
    { field: "vmem_kb", key: "col.vmem", kind: "kib" },
    { field: "vswap_kb", key: "col.vswap", kind: "kib" },
    { counter: true, field: "minflt", key: "col.minflt", kind: "rate" },
    { counter: true, field: "majflt", key: "col.majflt", kind: "rate" },
  ],
  disk: [
    { counter: true, field: "read_bytes", key: "col.read_bytes", kind: "bytes" },
    { counter: true, field: "write_bytes", key: "col.write_bytes", kind: "bytes" },
    { counter: true, field: "cancelled_write_bytes", key: "col.cancelled_write", kind: "bytes" },
    { counter: true, field: "syscr", key: "col.syscr", kind: "rate" },
    { counter: true, field: "syscw", key: "col.syscw", kind: "rate" },
    { counter: true, field: "rchar", key: "col.rchar", kind: "bytes" },
    { counter: true, field: "wchar", key: "col.wchar", kind: "bytes" },
    { counter: true, field: "blkdelay_ticks", key: "col.blkdelay", kind: "rate" },
  ],
}

export const PROCESS_HISTORY_FIELDS: readonly string[] = [
  "pid", "starttime",
  ...new Set(Object.values(PROCESS_HISTORY).flatMap((lens) => lens.map((field) => field.field))),
]

const ACTIVITY_FIELDS = [
  ["leader_pid", "pg.leader_pid", "id"], ["backend_type", "pg.backend_type", "text"], ["datname", "pg.datname", "text"],
  ["usename", "pg.usename", "text"],
  ["application_name", "pg.application_name", "text"], ["client_addr", "pg.client_addr", "text"],
  ["state", "pg.state", "text"],
  ["wait_event_type", "pg.wait_event_type", "text"], ["wait_event", "pg.wait_event", "text"],
  ["backend_start", "pg.backend_start", "time"],
  ["xact_start", "pg.xact_start", "time"], ["query_start", "pg.query_start", "time"],
  ["state_change", "pg.state_change", "time"],
  ["query_id", "pg.query_id", "id"], ["backend_xid_age", "pg.backend_xid_age", "number"],
  ["backend_xmin_age", "pg.backend_xmin_age", "number"],
] as const

export function DetailDock({
  activity,
  cursor,
  activityTime,
  hour,
  lens,
  locale,
  onClose,
  process,
  processHistory,
  t,
  ticksPerSecond,
}: {
  readonly activity: DataRow | null
  readonly activityTime: number | null
  readonly cursor: number
  readonly hour: number
  readonly lens: Lens
  readonly locale: Locale
  readonly onClose: () => void
  readonly process: DataRow
  readonly processHistory: readonly DataRow[]
  readonly ticksPerSecond: number | null
  readonly t: Translate
}) {
  const commandCell = value(process, "cmdline")
  const pid = identifier(value(process, "pid"))
  const commandPath = rawText(commandCell)?.trim() ? `/proc/${pid}/cmdline` : `/proc/${pid}/comm`
  const history = useMemo(
    () => processLensHistory(processHistory, lens),
    [lens, processHistory],
  )
  return (
    <aside
      aria-label={t("detail.process.title")}
      className="detail-dock"
      data-testid={activity === null ? "process-dock" : "pg-linked-dock"}
    >
      <div className="panel-head detail-head">
        <div>
          <LabelHelp helpKey="detail.process.help" labelKey="detail.process.title" t={t} />
          <p className="detail-identity">PID {pid}</p>
        </div>
        <button aria-label={t("common.close")} className="icon-button dock-close" onClick={onClose} type="button"><X aria-hidden="true" size={15} /></button>
      </div>
      <section className="command-block" title={commandPath}>
        <code data-testid="process-cmdline">{processCommand(process)}</code>
        <button aria-label={t("common.raw")} className="copy-raw" onClick={() => void navigator.clipboard?.writeText(processCommand(process))} type="button"><Copy aria-hidden="true" size={12} /></button>
      </section>
      <dl className="detail-list">
        <DetailField help="col.pid.help" label="col.pid.label" t={t} value={identifier(value(process, "pid"))} />
        {LENS_FIELDS[lens].filter((field) => field.id !== "command" && field.id !== "pid" && field.field !== undefined && value(process, field.field) !== null).map((field) => <DetailField help={field.help} key={field.id} label={field.label} t={t} value={<CellValue field={field} linked={false} locale={locale} row={process} t={t} ticksPerSecond={ticksPerSecond} />} />)}
      </dl>
      <section aria-label={t(`lens.${lens}`)} className="process-history" data-testid="process-history">
        {history.filter((series) => series.points.some((point) => point.value !== null)).map((series) => (
          <SeriesChart
            cursor={cursor}
            hour={hour}
            key={series.field}
            label={t(`${series.key}.label`)}
            locale={locale}
            format={(reading, place) => formatCell(series.kind, reading, place, t, ticksPerSecond)}
            points={series.points}
          />
        ))}
        <div className="process-history-ticks"><TimeTicks className="mini-time-ticks" hour={hour} ticks={4} /></div>
      </section>

      {activity !== null && <section className="pg-section">
        <div className="pg-title">
          <h3>{t("detail.pg_pid", { pid: identifier(value(activity, "pid")) })}</h3>
        </div>
        <dl className="detail-list">
          <DetailField help="detail.pg_snapshot.help" label="detail.pg_snapshot.label" t={t} value={activityTime === null ? "—" : <Timestamp raw={activityTime} t={t} />} />
          {ACTIVITY_FIELDS.map(([field, key, kind]) => <DetailField help={`${key}.help`} key={field} label={`${key}.label`} t={t} value={formatActivity(value(activity, field), kind, locale, t)} />)}
        </dl>
        <section className="query-block">
          <LabelHelp helpKey="pg.query.help" labelKey="pg.query.label" t={t} />
          <pre data-testid="pg-exact-query">{rawText(value(activity, "query")) ?? "—"}</pre>
        </section>
      </section>}
    </aside>
  )
}

function DetailField({ help, label, t, value: output }: { readonly help: string; readonly label: string; readonly t: Translate; readonly value: ReactNode }) {
  return <div><dt><LabelHelp helpKey={help} labelKey={label} t={t} /></dt><dd>{output}</dd></div>
}

function Timestamp({ cell, raw, t }: { readonly cell?: Cell; readonly raw?: number; readonly t: Translate }) {
  const timestamp = raw ?? asNumber(cell ?? null)
  if (timestamp === null || timestamp === undefined) return <>—</>
  return <span className="timestamp-value"><span>{formatUtc(timestamp)}</span><button aria-label={t("common.raw")} onClick={() => void navigator.clipboard?.writeText(String(timestamp))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
}

function formatActivity(cell: Cell, kind: string, locale: Locale, t: Translate): ReactNode {
  if (kind === "id") return identifier(cell)
  if (kind === "number") return measure(cell, locale)
  if (kind === "time") return <Timestamp cell={cell} t={t} />
  return rawText(cell) ?? "—"
}


export function processLensHistory(
  rows: readonly DataRow[],
  lens: Lens,
): readonly ProcessHistorySeries[] {
  const selected = rows
    .slice()
    .sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  return PROCESS_HISTORY[lens].map((field) => ({
    ...field,
    points: historyPoints(selected, field.field, field.counter === true),
  }))
}

function historyPoints(
  rows: readonly DataRow[],
  field: string,
  counter: boolean,
): readonly ChartPoint[] {
  let earlier: { readonly value: number; readonly timestamp: number } | null = null
  return rows.map((row) => {
    const number = asNumber(value(row, field))
    const drawn = counter ? rate(earlier, number, row.timestamp) : number
    if (counter) earlier = number === null ? null : { value: number, timestamp: row.timestamp }
    return {
      segmentId: row.segmentId,
      timestamp: row.timestamp,
      value: drawn,
    }
  })
}

function rate(
  earlier: { readonly value: number; readonly timestamp: number } | null,
  number: number | null,
  timestamp: number,
): number | null {
  if (earlier === null || number === null) return null
  const seconds = (timestamp - earlier.timestamp) / 1_000_000
  if (seconds <= 0) return null
  const delta = number - earlier.value
  return delta < 0 ? null : delta / seconds
}
