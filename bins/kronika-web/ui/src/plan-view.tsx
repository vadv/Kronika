import { Copy } from "lucide-react"

import type { Translate } from "./help"
import { planTextSummary } from "./plan-text"

export function PlanSummary({ raw }: { readonly raw: string | null }) {
  const summary = planTextSummary(raw) ?? "—"
  return <span className="block overflow-hidden text-ellipsis whitespace-nowrap font-medium text-fg2" title={summary}>{summary}</span>
}

export function PlanView({ raw, t }: { readonly raw: string | null; readonly t: Translate }) {
  const available = planTextSummary(raw) !== null
  return <section className="query-block" data-testid="pg-plan-view">
    <header className="flex min-h-7 items-center justify-between gap-2 border-b border-line3 px-2 py-1">
      <strong className="text-xs uppercase tracking-[.04em] text-fg3">{t("pg.plan.text")}</strong>
      {available && raw !== null && <button aria-label={t("pg.plan.copy")} className="inline-flex cursor-pointer items-center gap-1 border border-line4 bg-s2 px-1.5 py-1 text-xs uppercase text-accent3" onClick={() => void navigator.clipboard?.writeText(raw)} type="button"><Copy aria-hidden="true" size={12} />{t("pg.plan.copy")}</button>}
    </header>
    {available && raw !== null
      ? <pre className="m-0 max-h-[min(460px,48vh)] overflow-auto whitespace-pre-wrap px-2 py-1.5 text-xs leading-[1.5] text-fg2" data-testid="pg-text-plan">{raw}</pre>
      : <p className="m-0 px-2 py-2 text-sm text-fg3" data-testid="pg-plan-unavailable">{t("pg.plan.unavailable")}</p>}
  </section>
}
