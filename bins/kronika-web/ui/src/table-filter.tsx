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
  return <div className="table-filter">
    {context !== undefined && <span className="entity-context-filter" data-testid="entity-context-filter">
      <strong>{context}</strong>
      <button onClick={onContextClear} type="button"><X aria-hidden="true" size={11} />{t("filter.show_all")}</button>
    </span>}
    {context !== undefined && onPattern !== undefined && <span className="filter-intersection">{t("filter.and")}</span>}
    {onPattern !== undefined && <><span className="table-text-search"><Search aria-hidden="true" size={12} /><span>{t("filter.text")}</span></span>
      <input
      aria-label={t("filter.label")}
      data-testid="table-filter"
      onChange={(event) => onPattern(event.target.value)}
      placeholder={t("filter.placeholder")}
      spellCheck={false}
      title={t("filter.hint")}
      type="search"
      value={pattern}
    />
    {pattern !== "" && <>
      {kept >= 0 && <span className="table-filter-count">{t("filter.kept", { kept: String(kept), total: String(total) })}</span>}
      <button aria-label={t("filter.clear")} onClick={() => onPattern("")} type="button"><X aria-hidden="true" size={12} /></button>
    </>}</>}
  </div>
}
