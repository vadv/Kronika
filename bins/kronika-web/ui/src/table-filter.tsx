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
      {t("filter.entity", { entity: context })}
      <button aria-label={t("filter.show_all")} onClick={onContextClear} type="button"><X aria-hidden="true" size={11} /></button>
    </span>}
    {onPattern !== undefined && <><Search aria-hidden="true" size={12} />
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
