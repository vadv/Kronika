import { X } from "lucide-react"
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
import { SeriesChart, type ChartPoint } from "./series-chart"

interface ProcessField {
  readonly field: string
  readonly key: string
  readonly kind: "id" | "number" | "kib" | "bytes" | "ns" | "state"
}

const PROCESS_DETAIL: Readonly<Record<Lens, readonly ProcessField[]>> = {
  generic: [
    processField("state", "col.state", "state"), processField("ppid", "col.ppid", "id"),
    processField("uid", "col.uid", "id"), processField("euid", "col.euid", "id"),
    processField("gid", "col.gid", "id"), processField("egid", "col.egid", "id"),
    processField("num_threads", "col.threads", "number"), processField("tty", "col.tty", "id"),
    processField("exit_signal", "col.exit_signal", "id"),
  ],
  cpu: [
    processField("state", "col.state", "state"), processField("curcpu", "col.curcpu", "id"),
    processField("utime", "col.utime", "number"), processField("stime", "col.stime", "number"),
    processField("rundelay_ns", "col.rundelay", "ns"), processField("blkdelay_ticks", "col.blkdelay", "number"),
    processField("nvcsw", "col.nvcsw", "number"), processField("nivcsw", "col.nivcsw", "number"),
    processField("nice", "col.nice", "number"), processField("prio", "col.prio", "number"),
    processField("rtprio", "col.rtprio", "number"), processField("policy", "col.policy", "id"),
  ],
  memory: [
    processField("state", "col.state", "state"), processField("rmem_kb", "col.rmem", "kib"),
    processField("vmem_kb", "col.vmem", "kib"), processField("vswap_kb", "col.vswap", "kib"),
    processField("minflt", "col.minflt", "number"), processField("majflt", "col.majflt", "number"),
  ],
  disk: [
    processField("state", "col.state", "state"), processField("read_bytes", "col.read_bytes", "bytes"),
    processField("write_bytes", "col.write_bytes", "bytes"), processField("cancelled_write_bytes", "col.cancelled_write", "bytes"),
    processField("syscr", "col.syscr", "number"), processField("syscw", "col.syscw", "number"),
    processField("rchar", "col.rchar", "number"), processField("wchar", "col.wchar", "number"),
    processField("blkdelay_ticks", "col.blkdelay", "number"),
  ],
}

interface HistoryField {
  readonly field: string
  readonly key: string
  readonly format: (value: number, locale: Locale) => string
}

export interface ProcessHistorySeries extends HistoryField {
  readonly points: readonly ChartPoint[]
}

/** The extremes of a chart read in the unit of the column it draws. Every
 *  cumulative one is already a rate, so these are per second. */
function plain(value: number, locale: Locale): string {
  return measure(value, locale)
}

function ticks(value: number, locale: Locale): string {
  return measure(value, locale, " tick/s")
}

function delay(value: number, locale: Locale): string {
  return millisecondsPerSecond(value, locale) + " ms/s"
}

function bytes(value: number, locale: Locale): string {
  return humanBytes(value, locale, "/s")
}

function kibibytes(value: number, locale: Locale): string {
  return humanBytes(value * 1024, locale)
}

const PROCESS_HISTORY: Readonly<Record<Lens, readonly HistoryField[]>> = {
  generic: [{ field: "num_threads", key: "col.threads", format: plain }],
  cpu: [
    { field: "utime", key: "col.utime", format: ticks },
    { field: "stime", key: "col.stime", format: ticks },
    { field: "rundelay_ns", key: "col.rundelay", format: delay },
    { field: "blkdelay_ticks", key: "col.blkdelay", format: plain },
    { field: "nvcsw", key: "col.nvcsw", format: plain },
    { field: "nivcsw", key: "col.nivcsw", format: plain },
    { field: "minflt", key: "col.minflt", format: plain },
    { field: "majflt", key: "col.majflt", format: plain },
  ],
  memory: [
    { field: "rmem_kb", key: "col.rmem", format: kibibytes },
    { field: "vmem_kb", key: "col.vmem", format: kibibytes },
    { field: "vswap_kb", key: "col.vswap", format: kibibytes },
    { field: "minflt", key: "col.minflt", format: plain },
    { field: "majflt", key: "col.majflt", format: plain },
  ],
  disk: [
    { field: "read_bytes", key: "col.read_bytes", format: bytes },
    { field: "write_bytes", key: "col.write_bytes", format: bytes },
    { field: "cancelled_write_bytes", key: "col.cancelled_write", format: bytes },
    { field: "syscr", key: "col.syscr", format: plain },
    { field: "syscw", key: "col.syscw", format: plain },
    { field: "rchar", key: "col.rchar", format: bytes },
    { field: "wchar", key: "col.wchar", format: bytes },
    { field: "blkdelay_ticks", key: "col.blkdelay", format: plain },
  ],
}

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
  activityTime,
  hour,
  lens,
  locale,
  onClose,
  process,
  processHistory,
  t,
}: {
  readonly activity: DataRow | null
  readonly activityTime: number | null
  readonly hour: number
  readonly lens: Lens
  readonly locale: Locale
  readonly onClose: () => void
  readonly process: DataRow
  readonly processHistory: readonly DataRow[]
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
      <section className="command-block">
        <div className="command-heading">
          <LabelHelp helpKey="detail.cmdline.help" labelKey="detail.cmdline.label" t={t} />
          <code>{commandPath}</code>
        </div>
        <code data-testid="process-cmdline">{processCommand(process)}</code>
      </section>
      <dl className="detail-list">
        <DetailField help="col.pid.help" label="col.pid.label" t={t} value={identifier(value(process, "pid"))} />
        <DetailField help="col.starttime.help" label="col.starttime.label" t={t} value={<Timestamp cell={value(process, "starttime")} t={t} />} />
        {PROCESS_DETAIL[lens].map((field) => <DetailField help={`${field.key}.help`} key={field.field} label={`${field.key}.label`} t={t} value={formatProcess(value(process, field.field), field.kind, locale)} />)}
      </dl>
      <section aria-label={t(`lens.${lens}`)} className="process-history" data-testid="process-history">
        {history.map((series) => (
          <SeriesChart
            hour={hour}
            key={series.field}
            label={t(`${series.key}.label`)}
            locale={locale}
            format={series.format}
            points={series.points}
          />
        ))}
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
  return <span className="timestamp-value"><span>{formatUtc(timestamp)}</span><button aria-label={t("common.raw")} onClick={() => void navigator.clipboard?.writeText(String(timestamp))} type="button">{t("common.raw")}</button></span>
}

function formatProcess(cell: Cell, kind: ProcessField["kind"], locale: Locale): string {
  if (kind === "id") return identifier(cell)
  if (kind === "state") return stateText(cell)
  if (kind === "kib") return humanBytes(asNumber(cell) === null ? null : (asNumber(cell) ?? 0) * 1024, locale)
  if (kind === "bytes") return humanBytes(cell, locale, "/s")
  if (kind === "ns") return millisecondsPerSecond(cell, locale) + " ms/s"
  return measure(cell, locale)
}

function formatActivity(cell: Cell, kind: string, locale: Locale, t: Translate): ReactNode {
  if (kind === "id") return identifier(cell)
  if (kind === "number") return measure(cell, locale)
  if (kind === "time") return <Timestamp cell={cell} t={t} />
  return rawText(cell) ?? "—"
}

function processField(field: string, key: string, kind: ProcessField["kind"]): ProcessField {
  return { field, key, kind }
}

/// The rows are already one process: they were asked for by identity, and a
/// second filter here only hides a mismatch instead of showing it.
export function processLensHistory(
  rows: readonly DataRow[],
  lens: Lens,
): readonly ProcessHistorySeries[] {
  const selected = rows
    .slice()
    .sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  return PROCESS_HISTORY[lens].map((field) => ({
    ...field,
    points: historyPoints(selected, field.field),
  }))
}

function historyPoints(rows: readonly DataRow[], field: string): readonly ChartPoint[] {
  let previousSegment: string | null = null
  let run = 0
  return rows.map((row) => {
    if (row.segmentId !== previousSegment) {
      previousSegment = row.segmentId
      run = 0
    }
    const number = asNumber(value(row, field))
    const point = {
      segmentId: `${row.segmentId}:${run}`,
      timestamp: row.timestamp,
      value: number,
    }
    if (number === null) run += 1
    return point
  })
}
