import { Copy, X } from "lucide-react"
import { useMemo, useState, type ReactNode } from "react"

import type { Cell, DataRow } from "./api"
import { buildMetricSamples } from "./chart"
import { ChartOnly } from "./chart-visibility"
import { useDetailDismiss } from "./detail-dismiss"
import { useDisplayTime } from "./display-time-context"
import { LabelHelp, type Translate } from "./help"
import type { HistoryStatus } from "./history-request"
import {
  asNumber,
  humanBytes,
  humanCores,
  humanDuration,
  identifier,
  measure,
  processCommand,
  rawText,
  value,
  type Lens,
  type Locale,
} from "./model"
import { activityDurationMs, backendAgeMs, stateDurationMs, transactionDurationMs } from "./postgres-activity"
import { CellValue, formatCell, LENS_FIELDS, type Field } from "./process-table"
import { SeriesChart, type ChartPoint } from "./series-chart"

interface HistoryField {
  readonly field: string
  readonly key: string
  readonly kind: Field["kind"]
  readonly counter?: true
  readonly scale?: "nonnegative" | "signed"
  readonly unit?: string
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
    { field: "nice", key: "col.nice", kind: "number", scale: "signed", unit: "priority" },
    { field: "prio", key: "col.prio", kind: "number", unit: "priority" },
    { field: "rtprio", key: "col.rtprio", kind: "number", unit: "priority" },
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
  "pid",
  ...new Set(Object.values(PROCESS_HISTORY).flatMap((lens) => lens.map((field) => field.field))),
]

const ACTIVITY_FIELDS = [
  ["leader_pid", "pg.leader_pid", "id"], ["backend_type", "pg.backend_type", "text"], ["datname", "pg.datname", "text"],
  ["usename", "pg.usename", "text"],
  ["application_name", "pg.application_name", "text"], ["client_addr", "pg.client_addr", "text"],
  ["state", "pg.state", "text"],
  ["wait_event_type", "pg.wait_event_type", "text"], ["wait_event", "pg.wait_event", "text"],
  ["query_id", "pg.query_id", "id"], ["backend_xid_age", "pg.backend_xid_age", "number"],
  ["backend_xmin_age", "pg.backend_xmin_age", "number"],
] as const

const ACTIVITY_DURATIONS = [
  ["backend_age_ms", backendAgeMs],
  ["transaction_duration_ms", transactionDurationMs],
  ["query_duration_ms", activityDurationMs],
  ["state_duration_ms", stateDurationMs],
] as const

export function DetailDock({
  activity,
  cursor,
  activityTime,
  hour,
  lens,
  locale,
  onClose,
  onCursor,
  process,
  processHistory,
  processHistoryStatus,
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
  readonly onCursor: (timestamp: number) => void
  readonly process: DataRow
  readonly processHistory: readonly DataRow[]
  readonly processHistoryStatus: HistoryStatus
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
  const availableHistory = useMemo(
    () => history.filter((series) => series.points.some((point) => point.value !== null && Number.isFinite(point.value))),
    [history],
  )
  const [selectedHistoryField, setSelectedHistoryField] = useState<string | null>(null)
  const detail = useDetailDismiss(onClose, pid)
  const selectedHistory = availableHistory.find((series) => series.field === selectedHistoryField) ?? availableHistory[0] ?? null
  const selectedHistoryPoints = useMemo(
    () => selectedHistory === null ? [] : processChartPoints(selectedHistory, ticksPerSecond),
    [selectedHistory, ticksPerSecond],
  )
  return (
    <aside
      aria-label={t("detail.process.title")}
      className="detail-dock"
      data-testid={activity === null ? "process-dock" : "pg-linked-dock"}
      ref={detail}
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
      <ChartOnly><section aria-label={t(`lens.${lens}`)} className="process-history" data-testid="process-history">
        <div aria-label={t(`lens.${lens}`)} className="process-history-selector" role="group">
          {availableHistory.map((series) => (
            <button
              aria-pressed={series.field === selectedHistory?.field}
              data-testid={`process-history-metric-${series.field}`}
              key={series.field}
              onClick={() => setSelectedHistoryField(series.field)}
              type="button"
            >
              {t(`${series.key}.label`)}
            </button>
          ))}
        </div>
        {selectedHistory !== null && (
          <SeriesChart
            cursor={cursor}
            helpKey={`${selectedHistory.key}.help`}
            hour={hour}
            key={selectedHistory.field}
            labelKey={`${selectedHistory.key}.label`}
            locale={locale}
            format={(reading, place) => formatProcessChartValue(selectedHistory.kind, reading, place, t, ticksPerSecond)}
            onCursor={onCursor}
            points={selectedHistoryPoints}
            scale={selectedHistory.scale ?? "nonnegative"}
            status={processHistoryStatus}
            t={t}
            unit={selectedHistory.unit ?? processChartUnit(selectedHistory.kind, t, ticksPerSecond)}
          />
        )}
      </section></ChartOnly>

      {activity !== null && <section className="pg-section">
        <div className="pg-title">
          <h3>{t("detail.pg_pid", { pid: identifier(value(activity, "pid")) })}</h3>
        </div>
        <dl className="detail-list">
          <DetailField help="detail.pg_snapshot.help" label="detail.pg_snapshot.label" t={t} value={activityTime === null ? "—" : <Timestamp raw={activityTime} t={t} />} />
          {ACTIVITY_DURATIONS.flatMap(([field, duration]) => {
            const elapsed = duration(activity)
            return elapsed === null ? [] : [<DetailField help={`pg.field.${field}.help`} key={field} label={`pg.field.${field}.label`} t={t} value={humanDuration(elapsed, locale)} />]
          })}
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

export function processChartPoints(
  series: ProcessHistorySeries,
  ticksPerSecond: number | null,
): readonly ChartPoint[] {
  const divisor = series.kind === "cores" && ticksPerSecond !== null && ticksPerSecond > 0
    ? ticksPerSecond
    : series.kind === "ns" ? 1_000_000 : 1
  const multiplier = series.kind === "kib" ? 1024 : 1
  if (divisor === 1 && multiplier === 1) return series.points
  return series.points.map((point) => ({
    ...point,
    value: point.value === null ? null : point.value * multiplier / divisor,
  }))
}

export function processChartUnit(kind: Field["kind"], t: Translate, ticksPerSecond: number | null): string {
  if (kind === "cores") return ticksPerSecond !== null && ticksPerSecond > 0 ? t("unit.cores").trim() : `ticks${t("unit.per_second")}`
  if (kind === "ns") return t("unit.ms_per_second").trim()
  if (kind === "kib") return "B"
  if (kind === "bytes") return `B${t("unit.per_second")}`
  if (kind === "rate") return `#${t("unit.per_second")}`
  return "#"
}

function formatProcessChartValue(
  kind: Field["kind"],
  reading: number,
  locale: Locale,
  t: Translate,
  ticksPerSecond: number | null,
): string {
  if (kind === "cores" && ticksPerSecond !== null && ticksPerSecond > 0) return humanCores(reading, locale, t("unit.cores"))
  if (kind === "ns") return measure(reading, locale, t("unit.ms_per_second"))
  if (kind === "kib") return humanBytes(reading, locale)
  if (kind === "bytes") return humanBytes(reading, locale, t("unit.per_second"))
  return formatCell(kind, reading, locale, t, ticksPerSecond)
}

function DetailField({ help, label, t, value: output }: { readonly help: string; readonly label: string; readonly t: Translate; readonly value: ReactNode }) {
  return <div><dt><LabelHelp helpKey={help} labelKey={label} t={t} /></dt><dd>{output}</dd></div>
}

function Timestamp({ cell, raw, t }: { readonly cell?: Cell; readonly raw?: number; readonly t: Translate }) {
  const time = useDisplayTime()
  const timestamp = raw ?? asNumber(cell ?? null)
  if (timestamp === null || timestamp === undefined) return <>—</>
  return <span className="timestamp-value"><span>{time.timestamp(timestamp)}</span><button aria-label={t("common.raw")} onClick={() => void navigator.clipboard?.writeText(String(timestamp))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
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
  return buildMetricSamples(rows, (row) => Object.hasOwn(row.values, field)
    ? asNumber(value(row, field))
    : undefined).map((sample) => {
    const drawn = counter ? rate(earlier, sample.value, sample.timestamp) : sample.value
    if (counter) earlier = sample.value === null ? null : { value: sample.value, timestamp: sample.timestamp }
    return {
      segmentId: sample.segmentId,
      timestamp: sample.timestamp,
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
