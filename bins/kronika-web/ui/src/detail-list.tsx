import type { ReactNode } from "react"

// Ordinary operator detail docks all use this compact term/value composition.
// Tile dashboards and prose definitions are deliberately separate patterns.
export function DetailList({ children, className }: { readonly children: ReactNode; readonly className?: string | undefined }) {
  return <dl className={`m-0 mt-2${className === undefined ? "" : ` ${className}`}`}>{children}</dl>
}

export function DetailRow({ children, term, valueClassName }: {
  readonly children: ReactNode
  readonly term: ReactNode
  readonly valueClassName?: string | undefined
}) {
  return <div className="detail-row max-[520px]:detail-row-stacked">
    <dt className="detail-dt">{term}</dt>
    <dd className={`detail-dd max-[520px]:text-left${valueClassName === undefined ? "" : ` ${valueClassName}`}`}>{children}</dd>
  </div>
}
