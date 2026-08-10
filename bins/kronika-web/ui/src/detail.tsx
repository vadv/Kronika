import { X } from "lucide-react"
import { registry } from "kronika:registry"
import type { ReactNode } from "react"

import type { Cell, DataRow } from "./api"
import { LabelHelp, type Translate } from "./help"
import {
  asNumber,
  formatUtc,
  identifier,
  measure,
  payloadMeta,
  processCommand,
  rawText,
  stateText,
  value,
  type Lens,
  type Locale,
} from "./model"

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
    processField("scope", "col.scope", "id"), processField("exit_signal", "col.exit_signal", "id"),
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

const ACTIVITY_FIELDS = [
  ["pid", "pg.pid", "id"], ["leader_pid", "pg.leader_pid", "id"],
  ["datname", "pg.datname", "text"], ["usename", "pg.usename", "text"],
  ["application_name", "pg.application_name", "text"], ["client_addr", "pg.client_addr", "text"],
  ["backend_type", "pg.backend_type", "text"], ["state", "pg.state", "text"],
  ["wait_event_type", "pg.wait_event_type", "text"], ["wait_event", "pg.wait_event", "text"],
  ["query_id", "pg.query_id", "id"], ["backend_xid_age", "pg.backend_xid_age", "number"],
  ["backend_xmin_age", "pg.backend_xmin_age", "number"], ["backend_start", "pg.backend_start", "time"],
  ["xact_start", "pg.xact_start", "time"], ["query_start", "pg.query_start", "time"],
  ["state_change", "pg.state_change", "time"],
] as const

export function DetailDock({
  activity,
  activitySnapshotTime,
  lens,
  locale,
  onClose,
  process,
  t,
}: {
  readonly activity: DataRow | null
  readonly activitySnapshotTime: number | null
  readonly lens: Lens
  readonly locale: Locale
  readonly onClose: () => void
  readonly process: DataRow
  readonly t: Translate
}) {
  const commandCell = value(process, "cmdline")
  const commandMeta = payloadMeta(commandCell)
  return (
    <aside
      aria-label={t("detail.process.title")}
      className="detail-dock"
      data-testid={activity === null ? "process-dock" : "pg-linked-dock"}
    >
      <div className="panel-head detail-head">
        <div>
          <LabelHelp helpKey="detail.process.help" labelKey="detail.process.title" t={t} />
          <p className="detail-identity">PID {identifier(value(process, "pid"))}</p>
        </div>
        <button aria-label={t("common.close")} className="icon-button dock-close" onClick={onClose} type="button"><X aria-hidden="true" size={15} /></button>
      </div>
      <section className="command-block">
        <LabelHelp helpKey="detail.cmdline.help" labelKey="detail.cmdline.label" t={t} />
        <code data-testid="process-cmdline">{processCommand(process)}</code>
        {commandMeta !== null && <small>{commandMeta}</small>}
      </section>
      <dl className="detail-list">
        <DetailField help="col.pid.help" label="col.pid.label" t={t} value={identifier(value(process, "pid"))} />
        <DetailField help="col.starttime.help" label="col.starttime.label" t={t} value={<Timestamp cell={value(process, "starttime")} t={t} />} />
        <DetailField help="detail.os_source.help" label="detail.os_source.label" t={t} value={<Timestamp raw={process.timestamp} t={t} />} />
        <DetailField help="detail.os_layout.help" label="detail.os_layout.label" t={t} value={layoutName(process)} />
        {PROCESS_DETAIL[lens].map((field) => <DetailField help={`${field.key}.help`} key={field.field} label={`${field.key}.label`} t={t} value={formatProcess(value(process, field.field), field.kind, locale)} />)}
      </dl>

      <section className="pg-section">
        <div className="pg-title">
          <span className="pg-badge">{t("pg.badge")}</span>
          <LabelHelp helpKey="detail.pg.help" labelKey="detail.pg.title" t={t} />
        </div>
        {activitySnapshotTime !== null && <dl className="detail-list"><DetailField help="detail.pg_source.help" label="detail.pg_source.label" t={t} value={<Timestamp raw={activitySnapshotTime} t={t} />} /></dl>}
        {activity === null
          ? <p className="pg-empty">{t("detail.pg_none")}</p>
          : <>
            <p className="backend-type">{rawText(value(activity, "backend_type")) ?? "—"}</p>
            <dl className="detail-list">
              <DetailField help="detail.pg_layout.help" label="detail.pg_layout.label" t={t} value={layoutName(activity)} />
              {ACTIVITY_FIELDS.map(([field, key, kind]) => <DetailField help={`${key}.help`} key={field} label={`${key}.label`} t={t} value={formatActivity(value(activity, field), kind, locale, t)} />)}
            </dl>
            <section className="query-block">
              <LabelHelp helpKey="pg.query.help" labelKey="pg.query.label" t={t} />
              <pre data-testid="pg-exact-query">{rawText(value(activity, "query")) ?? "—"}</pre>
              {payloadMeta(value(activity, "query")) !== null && <small>{payloadMeta(value(activity, "query"))}</small>}
            </section>
          </>}
      </section>
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
  if (kind === "kib") return measure(cell, locale, " KiB")
  if (kind === "bytes") return measure(cell, locale, " B")
  if (kind === "ns") return measure(cell, locale, " ns")
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

function layoutName(row: DataRow): string {
  const layout = registry.find((candidate) => candidate.typeId === row.typeId)
  return `${layout?.physicalName ?? "unknown"} · type_id=${row.typeId}`
}
