import { Copy, X } from "lucide-react"
import { useMemo, useState, type ReactNode } from "react"

import type { Cell, DataRow } from "./api"
import { buildMetricSamples } from "./chart"
import { ChartOnly } from "./chart-visibility"
import { useDetailDismiss } from "./detail-dismiss"
import { DetailList, DetailRow } from "./detail-list"
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
import { CellValue, formatCell, LENS_FIELDS, PROCESS_USER_FIELDS, type Field } from "./process-table"
import { SeriesChart, type ChartPoint } from "./series-chart"
import { statementsForActivity, type RelatedNavigation } from "./statement-navigation"

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
  onRelated,
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
  readonly onRelated: (target: RelatedNavigation) => void
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
  const selectableHistory = availableHistory.length === 0 ? history : availableHistory
  const [selectedHistoryField, setSelectedHistoryField] = useState<string | null>(null)
  const detail = useDetailDismiss(onClose, pid)
  const selectedHistory = selectableHistory.find((series) => series.field === selectedHistoryField) ?? selectableHistory[0] ?? null
  const selectedHistoryPoints = useMemo(
    () => selectedHistory === null ? [] : processChartPoints(selectedHistory, ticksPerSecond),
    [selectedHistory, ticksPerSecond],
  )
  const relatedActivity = activity === null ? null : statementsForActivity(activity)
  return (
    <aside
      aria-label={t("detail.process.title")}
      className="h-full min-h-0 overflow-y-auto border border-line3 border-l-0 bg-s2 p-[13px] max-[1179px]:fixed max-[1179px]:bottom-2.5 max-[1179px]:right-2.5 max-[1179px]:top-[150px] max-[1179px]:z-80 max-[1179px]:h-auto max-[1179px]:w-[min(430px,calc(100vw-20px))] max-[1179px]:max-h-[calc(100dvh-160px)] max-[1179px]:max-w-[430px] max-[1179px]:border-l max-[1179px]:shadow-[-15px_0_45px_var(--color-shadow-a)]"
      data-testid={activity === null ? "process-dock" : "pg-linked-dock"}
      ref={detail}
    >
      <div className="flex items-center justify-between border-b border-line3 pb-2.5">
        <div>
          <span className="text-sm uppercase text-fg2"><LabelHelp helpKey="detail.process.help" labelKey="detail.process.title" t={t} /></span>
          <p className="mt-[5px] text-lg font-[650] text-fg-hi">PID {pid}</p>
        </div>
        <button aria-label={t("common.close")} className="icon-button flex-none" onClick={onClose} type="button"><X aria-hidden="true" size={15} /></button>
      </div>
      <section className="mt-2 flex items-center gap-1.5 border border-line3 bg-s1 px-1.5 py-[5px]" title={commandPath}>
        <code className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-sm leading-[1.5] text-fg [font-family:inherit] hover:overflow-visible hover:whitespace-normal hover:[text-overflow:clip] hover:[overflow-wrap:anywhere]" data-testid="process-cmdline">{processCommand(process)}</code>
        <button aria-label={t("common.raw")} className="inline-flex flex-none cursor-pointer items-center justify-center border border-line4 bg-transparent px-[3px] py-0.5 text-xs uppercase text-accent3" onClick={() => void navigator.clipboard?.writeText(processCommand(process))} type="button"><Copy aria-hidden="true" size={12} /></button>
      </section>
      <DetailList>
        <DetailField help="col.pid.help" label="col.pid.label" t={t} value={identifier(value(process, "pid"))} />
        {[...PROCESS_USER_FIELDS, ...LENS_FIELDS[lens]].filter((field, index, fields) => field.id !== "command" && field.id !== "pid" && field.field !== undefined && fields.findIndex((candidate) => candidate.id === field.id) === index && (field.kind === "user" || value(process, field.field) !== null)).map((field) => <DetailField help={field.help} key={field.id} label={field.label} t={t} value={<CellValue field={field} linked={false} locale={locale} row={process} t={t} ticksPerSecond={ticksPerSecond} />} />)}
      </DetailList>
      <ChartOnly><section aria-label={t(`lens.${lens}`)} className="process-history mt-2.5 grid min-w-0 gap-[7px] border-t border-line3 pt-[7px]" data-testid="process-history">
        <div aria-label={t(`lens.${lens}`)} className="history-selector flex max-w-full gap-[5px] overflow-x-auto p-px pb-[3px] [scrollbar-width:thin]" role="group">
          {selectableHistory.map((series) => (
            <button
              aria-pressed={series.field === selectedHistory?.field}
              className="min-h-[28px] flex-none cursor-pointer border border-line3 bg-s2 px-[7px] py-1 text-xs text-fg2 aria-pressed:border-accent aria-pressed:bg-accent-soft aria-pressed:text-fg"
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

      {activity !== null && <section className="mt-[13px] border-t border-line4 pt-3">
        <div className="flex items-center">
          <h3 className="m-0 text-sm font-[560] text-fg">{t("detail.pg_pid", { pid: identifier(value(activity, "pid")) })}</h3>
        </div>
        <DetailList>
          <DetailField help="detail.pg_snapshot.help" label="detail.pg_snapshot.label" t={t} value={activityTime === null ? "—" : <Timestamp raw={activityTime} t={t} />} />
          {ACTIVITY_DURATIONS.flatMap(([field, duration]) => {
            const elapsed = duration(activity)
            return elapsed === null ? [] : [<DetailField help={`pg.field.${field}.help`} key={field} label={`pg.field.${field}.label`} t={t} value={humanDuration(elapsed, locale)} />]
          })}
          {ACTIVITY_FIELDS.map(([field, key, kind]) => <DetailField help={`${key}.help`} key={field} label={`${key}.label`} t={t} value={field === "query_id" && relatedActivity !== null
            ? <button aria-label={t("pg.related.open_statements", { id: relatedActivity.queryId ?? "" })} className="cursor-pointer border-0 bg-transparent p-0 text-accent3 underline decoration-dotted underline-offset-2" onClick={() => onRelated(relatedActivity)} type="button">{identifier(value(activity, field))}</button>
            : formatActivity(value(activity, field), kind, locale, t)} />)}
        </DetailList>
        <section className="mt-2 border border-line3 bg-s1 px-1.5 py-[5px]">
          <span className="flex items-center justify-between text-xs uppercase text-fg3"><LabelHelp helpKey="pg.query.help" labelKey="pg.query.label" t={t} /></span>
          <pre className="mx-0 mb-0 mt-2 max-h-[170px] overflow-auto whitespace-pre-wrap break-words text-sm leading-[1.55] text-event-edge [font:inherit]" data-testid="pg-exact-query">{rawText(value(activity, "query")) ?? "—"}</pre>
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
  if (kind === "ns") return ""
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
  if (kind === "ns") return humanDuration(reading, locale, "milliseconds", t("unit.per_second"))
  if (kind === "kib") return humanBytes(reading, locale)
  if (kind === "bytes") return humanBytes(reading, locale, t("unit.per_second"))
  return formatCell(kind, reading, locale, t, ticksPerSecond)
}

function DetailField({ help, label, t, value: output }: { readonly help: string; readonly label: string; readonly t: Translate; readonly value: ReactNode }) {
  return <DetailRow term={<LabelHelp helpKey={help} labelKey={label} t={t} />} valueClassName="text-sm">{output}</DetailRow>
}

function Timestamp({ cell, raw, t }: { readonly cell?: Cell; readonly raw?: number; readonly t: Translate }) {
  const time = useDisplayTime()
  const timestamp = raw ?? asNumber(cell ?? null)
  if (timestamp === null || timestamp === undefined) return <>—</>
  return <span className="inline-flex items-center gap-[5px]"><span>{time.timestamp(timestamp)}</span><button aria-label={t("common.raw")} className="inline-flex cursor-pointer items-center justify-center border border-line4 bg-transparent px-[3px] py-0.5 text-accent3" onClick={() => void navigator.clipboard?.writeText(String(timestamp))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
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
