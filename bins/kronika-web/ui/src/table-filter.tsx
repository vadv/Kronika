import { Search, X } from "lucide-react"
import { useId } from "react"

import type { Translate } from "./help"

export function TableFilter({
  kept,
  onPattern,
  pattern,
  t,
  total,
}: {
  readonly kept: number
  readonly onPattern: (pattern: string) => void
  readonly pattern: string
  readonly t: Translate
  readonly total: number
}) {
  const id = useId()
  return <div className="table-filter">
    <Search aria-hidden="true" size={12} />
    <input
      aria-label={t("filter.label")}
      data-testid="table-filter"
      id={id}
      onChange={(event) => onPattern(event.target.value)}
      placeholder={t("filter.placeholder")}
      spellCheck={false}
      title={t("filter.hint")}
      type="search"
      value={pattern}
    />
    {pattern !== "" && <>
      <span className="table-filter-count">{t("filter.kept", { kept: String(kept), total: String(total) })}</span>
      <button aria-label={t("filter.clear")} onClick={() => onPattern("")} type="button"><X aria-hidden="true" size={12} /></button>
    </>}
  </div>
}
