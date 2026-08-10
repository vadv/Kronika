import { useEffect, useId, useState } from "react"

export type Translate = (key: string, slots?: Readonly<Record<string, string | number>>) => string

export function LabelHelp({
  labelKey,
  helpKey,
  iconOnly = false,
  t,
  testId,
}: {
  readonly labelKey: string
  readonly helpKey: string
  readonly iconOnly?: boolean
  readonly t: Translate
  readonly testId?: string
}) {
  const [open, setOpen] = useState(false)
  const id = useId()
  useEffect(() => {
    if (!open) return
    const close = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false)
    }
    window.addEventListener("keydown", close)
    return () => window.removeEventListener("keydown", close)
  }, [open])
  return (
    <span className="label-help" onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
      {!iconOnly && <span>{t(labelKey)}</span>}
      <button
        aria-describedby={open ? id : undefined}
        aria-expanded={open}
        aria-label={`${t(labelKey)} — ${t("help.open")}`}
        className="help-dot"
        data-testid={testId}
        onBlur={() => setOpen(false)}
        onClick={() => setOpen((current) => !current)}
        onFocus={() => setOpen(true)}
        type="button"
      >?</button>
      {open && <span className="tooltip" id={id} role="tooltip">{t(helpKey)}</span>}
    </span>
  )
}

export function HelpPanel({
  items,
  onClose,
  t,
}: {
  readonly items: readonly { readonly label: string; readonly help: string }[]
  readonly onClose: () => void
  readonly t: Translate
}) {
  return (
    <aside aria-label={t("help.title")} className="help-panel" data-testid="help-panel">
      <div className="panel-head">
        <div>
          <p className="eyebrow">?</p>
          <h2>{t("help.title")}</h2>
        </div>
        <button aria-label={t("help.close")} className="icon-button" onClick={onClose} type="button">×</button>
      </div>
      <p className="help-intro">{t("help.intro")}</p>
      <p className="shortcut">{t("help.shortcut")}</p>
      <dl className="help-list">
        {items.map((item) => <div key={item.label}><dt>{t(item.label)}</dt><dd>{t(item.help)}</dd></div>)}
      </dl>
    </aside>
  )
}
