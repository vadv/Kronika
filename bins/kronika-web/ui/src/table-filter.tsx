import { Search, X } from "lucide-react"

import type { Translate } from "./help"

export function TableFilter({
  context,
  kept,
  onContextClear,
  onPattern,
  pattern,
  t,
  total,
}: {
  readonly context?: string | undefined
  readonly kept: number
  readonly onContextClear?: (() => void) | undefined
  readonly onPattern?: ((pattern: string) => void) | undefined
  readonly pattern: string
  readonly t: Translate
  readonly total: number
}) {
  return <div className="flex min-h-[26px] items-center gap-1.5 border-b border-line2 bg-s2 px-[7px] text-fg3">
    {context !== undefined && <span className="inline-flex max-w-[58%] items-center gap-1.5 overflow-hidden whitespace-nowrap border border-accent2 bg-accent-soft pl-1.5 text-xs text-fg" data-testid="entity-context-filter">
      <strong className="overflow-hidden text-ellipsis font-semibold">{context}</strong>
      <button className="inline-flex cursor-pointer items-center gap-[3px] self-stretch border-0 border-l border-accent2 bg-transparent px-[5px] text-fg2" onClick={onContextClear} type="button"><X aria-hidden="true" size={11} />{t("filter.show_all")}</button>
    </span>}
    {context !== undefined && onPattern !== undefined && <span className="text-xs uppercase text-fg4">{t("filter.and")}</span>}
    {onPattern !== undefined && <><span className="inline-flex flex-none items-center gap-[3px] text-xs text-fg3"><Search aria-hidden="true" size={12} /><span>{t("filter.text")}</span></span>
      <input
      aria-label={t("filter.label")}
      className="min-w-0 flex-auto border-0 bg-transparent py-1 text-sm text-fg outline-none [font-family:inherit] placeholder:text-fg4 [&::-webkit-search-cancel-button]:hidden [&::-webkit-search-cancel-button]:appearance-none"
      data-testid="table-filter"
      onChange={(event) => onPattern(event.target.value)}
      placeholder={t("filter.placeholder")}
      spellCheck={false}
      title={t("filter.hint")}
      type="search"
      value={pattern}
    />
    {pattern !== "" && <>
      {kept >= 0 && <span className="flex-none text-xs tabular-nums text-fg3">{t("filter.kept", { kept: String(kept), total: String(total) })}</span>}
      <button aria-label={t("filter.clear")} className="inline-flex flex-none cursor-pointer items-center border-0 bg-transparent p-0.5 text-accent3" onClick={() => onPattern("")} type="button"><X aria-hidden="true" size={12} /></button>
    </>}</>}
  </div>
}
