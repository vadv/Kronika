import { Copy } from "lucide-react"
import type { ReactNode } from "react"

import { copyText } from "./clipboard"
import type { Cell, DataRow } from "./api"
import { DetailList, DetailRow } from "./detail-list"
import { useDisplayTime } from "./display-time-context"
import { LabelHelp, type Translate } from "./help"
import { asNumber, humanDuration, identifier, measure, rawText, value, type Locale } from "./model"
import { activityDurationMs, backendAgeMs, stateDurationMs, transactionDurationMs } from "./postgres-activity"
import { statementsForActivity, type RelatedNavigation } from "./statement-navigation"

// The PostgreSQL backend recorded under the selected process PID. It is the
// same identity in another recorded section, so it reads as its own panel
// rather than as a tail below the process facts.

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

export function ActivityFacts({ activity, activityTime, locale, onRelated, t }: {
  readonly activity: DataRow
  readonly activityTime: number | null
  readonly locale: Locale
  readonly onRelated: (target: RelatedNavigation) => void
  readonly t: Translate
}) {
  const related = statementsForActivity(activity)
  return <section className="p-3" data-testid="process-activity-panel">
    <DetailList>
      <DetailField help="detail.pg_snapshot.help" label="detail.pg_snapshot.label" t={t} value={activityTime === null ? "—" : <Timestamp raw={activityTime} t={t} />} />
      {ACTIVITY_DURATIONS.flatMap(([field, duration]) => {
        const elapsed = duration(activity)
        return elapsed === null ? [] : [<DetailField help={`pg.field.${field}.help`} key={field} label={`pg.field.${field}.label`} t={t} value={humanDuration(elapsed, locale)} />]
      })}
      {ACTIVITY_FIELDS.map(([field, key, kind]) => <DetailField help={`${key}.help`} key={field} label={`${key}.label`} t={t} value={field === "query_id" && related !== null
        ? <button aria-label={t("pg.related.open_statements", { id: related.queryId ?? "" })} className="cursor-pointer border-0 bg-transparent p-0 text-accent3 underline decoration-dotted underline-offset-2" onClick={() => onRelated(related)} type="button">{identifier(value(activity, field))}</button>
        : formatActivity(value(activity, field), kind, locale, t)} />)}
    </DetailList>
    <section className="query-block">
      <span className="flex items-center justify-between text-xs font-medium text-fg3"><LabelHelp helpKey="pg.query.help" labelKey="pg.query.label" t={t} /></span>
      <pre className="mx-0 mb-0 mt-2 max-h-[170px] overflow-auto whitespace-pre-wrap break-words text-sm leading-[1.55] text-event-edge [font:inherit]" data-testid="pg-exact-query">{rawText(value(activity, "query")) ?? "—"}</pre>
    </section>
  </section>
}

function DetailField({ help, label, t, value: output }: { readonly help: string; readonly label: string; readonly t: Translate; readonly value: ReactNode }) {
  return <DetailRow term={<LabelHelp helpKey={help} labelKey={label} t={t} />} valueClassName="text-sm">{output}</DetailRow>
}

function Timestamp({ cell, raw, t }: { readonly cell?: Cell; readonly raw?: number; readonly t: Translate }) {
  const time = useDisplayTime()
  const timestamp = raw ?? asNumber(cell ?? null)
  if (timestamp === null || timestamp === undefined) return <>—</>
  return <span className="inline-flex items-center gap-[5px]"><span>{time.timestamp(timestamp)}</span><button aria-label={t("common.raw")} className="inline-flex cursor-pointer items-center justify-center border-0 bg-transparent p-0 text-fg4 hover:text-accent3" onClick={() => void copyText(String(timestamp))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
}

function formatActivity(cell: Cell, kind: string, locale: Locale, t: Translate): ReactNode {
  if (kind === "id") return identifier(cell)
  if (kind === "number") return measure(cell, locale)
  if (kind === "time") return <Timestamp cell={cell} t={t} />
  return rawText(cell) ?? "—"
}
