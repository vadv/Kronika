import { Copy } from "lucide-react"

import type { Translate } from "./help"
import { presentPlan, type PlanNode } from "./plan-presentation"

export function PlanSummary({ raw }: { readonly raw: string }) {
  const summary = presentPlan(raw).summary
  return <span className="block overflow-hidden text-ellipsis whitespace-nowrap font-medium text-fg2" title={summary}>{summary}</span>
}

export function PlanView({ raw, t }: { readonly raw: string; readonly t: Translate }) {
  const plan = presentPlan(raw)
  return <section className="query-block" data-testid="pg-plan-view">
    <header className="flex min-h-7 items-center justify-between gap-2 border-b border-line3 px-2 py-1">
      <strong className="text-xs uppercase tracking-[.04em] text-fg3">{t("pg.plan.tree")}</strong>
      {plan.kind === "raw" ? <button aria-label={t("common.raw")} className="inline-flex cursor-pointer items-center gap-1 border border-line4 bg-s2 px-1.5 py-1 text-xs uppercase text-accent3" onClick={() => void navigator.clipboard?.writeText(raw)} type="button"><Copy aria-hidden="true" size={12} />{t("pg.plan.copy_raw")}</button> : <details className="group relative">
        <summary className="cursor-pointer list-none border border-line4 bg-s2 px-1.5 py-1 text-xs uppercase text-accent3 marker:content-none">{t("pg.plan.raw")}</summary>
        <div className="mt-1 border border-line3 bg-s1 p-2">
          <button aria-label={t("common.raw")} className="mb-1.5 inline-flex cursor-pointer items-center gap-1 border border-line4 bg-transparent px-1.5 py-1 text-xs uppercase text-accent3" onClick={() => void navigator.clipboard?.writeText(raw)} type="button"><Copy aria-hidden="true" size={12} />{t("pg.plan.copy_raw")}</button>
          <pre className="m-0 max-h-64 overflow-auto whitespace-pre-wrap break-words text-xs leading-[1.45] text-fg2" data-testid="pg-exact-plan">{raw}</pre>
        </div>
      </details>}
    </header>
    {plan.kind === "tree" && <div className="max-h-[min(460px,48vh)] overflow-auto px-2 py-1.5"><PlanTree node={plan.root} /></div>}
    {plan.kind === "text" && <pre className="m-0 max-h-[min(460px,48vh)] overflow-auto whitespace-pre-wrap px-2 py-1.5 text-xs leading-[1.5] text-fg2" data-testid="pg-text-plan">{plan.lines.join("\n")}</pre>}
    {plan.kind === "raw" && <div data-testid="pg-plan-fallback"><p className="m-0 border-t border-line3 px-2 py-2 text-sm text-warn">{t("pg.plan.unrecognized")}</p><pre className="m-0 max-h-[min(460px,48vh)] overflow-auto whitespace-pre-wrap break-words border-t border-line3 px-2 py-1.5 text-xs leading-[1.45] text-fg2" data-testid="pg-exact-plan">{raw}</pre></div>}
  </section>
}

function PlanTree({ node, depth = 0 }: { readonly node: PlanNode; readonly depth?: number | undefined }) {
  return <div className={depth === 0 ? "" : "ml-3 border-l border-line3 pl-2"} data-plan-depth={depth}>
    <article className="py-1.5" data-testid="pg-plan-node">
      <h3 className="m-0 flex flex-wrap items-baseline gap-x-1.5 text-sm font-semibold text-fg">
        <span>{node.nodeType}</span>
        {node.relation !== null && <span className="font-normal text-accent3">{node.relation}</span>}
        {node.index !== null && <span className="font-normal text-fg3">using {node.index}</span>}
      </h3>
      {node.attributes.length !== 0 && <dl className="mt-1 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-2 gap-y-0.5 text-xs leading-[1.35]">
        {node.attributes.map(({ label, value }, index) => <div className="contents" key={`${label}:${index}`}><dt className="text-fg4">{label}</dt><dd className="m-0 break-words tabular-nums text-fg2">{value}</dd></div>)}
      </dl>}
    </article>
    {node.children.map((child, index) => <PlanTree depth={depth + 1} key={`${child.nodeType}:${child.relation ?? ""}:${index}`} node={child} />)}
  </div>
}
