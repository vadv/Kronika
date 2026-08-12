import { useEffect, useId, useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"

export type Translate = (key: string, slots?: Readonly<Record<string, string | number>>) => string

interface TooltipPosition {
  readonly left: number
  readonly placement: "above" | "below"
  readonly top: number
}

interface Rectangle {
  readonly bottom: number
  readonly height: number
  readonly left: number
  readonly top: number
  readonly width: number
}

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
  const [position, setPosition] = useState<TooltipPosition | null>(null)
  const id = useId()
  const anchor = useRef<HTMLButtonElement>(null)
  const root = useRef<HTMLSpanElement>(null)
  const tooltip = useRef<HTMLSpanElement>(null)
  const closeTimer = useRef<number | null>(null)
  const pinned = useRef(false)
  const cancelClose = () => {
    if (closeTimer.current === null) return
    window.clearTimeout(closeTimer.current)
    closeTimer.current = null
  }
  const close = () => {
    cancelClose()
    pinned.current = false
    setOpen(false)
  }
  const enter = () => {
    cancelClose()
    setOpen(true)
  }
  const leave = () => {
    cancelClose()
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null
      if (!pinned.current && document.activeElement !== anchor.current) setOpen(false)
    }, 80)
  }
  useEffect(() => {
    if (!open) return
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      close()
    }
    const outside = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return
      if (root.current?.contains(event.target) || tooltip.current?.contains(event.target)) return
      close()
    }
    window.addEventListener("keydown", escape)
    document.addEventListener("pointerdown", outside, true)
    return () => {
      window.removeEventListener("keydown", escape)
      document.removeEventListener("pointerdown", outside, true)
    }
  }, [open])
  useEffect(() => () => cancelClose(), [])
  useLayoutEffect(() => {
    if (!open) return
    const update = () => {
      const anchorRect = anchor.current?.getBoundingClientRect()
      const tooltipRect = tooltip.current?.getBoundingClientRect()
      if (anchorRect === undefined || tooltipRect === undefined) return
      const next = placeTooltip(anchorRect, tooltipRect, { height: window.innerHeight, width: window.innerWidth })
      setPosition((current) => current?.left === next.left && current.top === next.top && current.placement === next.placement ? current : next)
    }
    update()
    window.addEventListener("resize", update)
    window.addEventListener("scroll", update, true)
    return () => {
      window.removeEventListener("resize", update)
      window.removeEventListener("scroll", update, true)
    }
  }, [open, t(helpKey)])
  return (
    <>
      <span className="label-help" onMouseEnter={enter} onMouseLeave={leave} ref={root}>
        {!iconOnly && <span>{t(labelKey)}</span>}
        <button
          aria-describedby={open ? id : undefined}
          aria-expanded={open}
          aria-label={`${t(labelKey)} — ${t("help.open")}`}
          className="help-dot"
          data-testid={testId}
          onBlur={close}
          onClick={() => {
            pinned.current = !pinned.current
            setOpen(pinned.current)
          }}
          onFocus={enter}
          ref={anchor}
          type="button"
        >?</button>
      </span>
      {open && typeof document !== "undefined" && createPortal(
        <span
          className="tooltip"
          data-placement={position?.placement}
          data-testid="help-tooltip"
          id={id}
          onMouseEnter={enter}
          onMouseLeave={leave}
          ref={tooltip}
          role="tooltip"
          style={{ left: position?.left ?? 0, top: position?.top ?? 0, visibility: position === null ? "hidden" : "visible" }}
        >{t(helpKey)}</span>,
        document.body,
      )}
    </>
  )
}

export function placeTooltip(
  anchor: Rectangle,
  size: Pick<Rectangle, "height" | "width">,
  viewport: { readonly height: number; readonly width: number },
): TooltipPosition {
  const margin = 8
  const offset = 6
  const below = anchor.bottom + offset
  const above = anchor.top - size.height - offset
  const roomBelow = viewport.height - anchor.bottom
  const roomAbove = anchor.top
  const placement = below + size.height <= viewport.height - margin || (above < margin && roomBelow >= roomAbove)
    ? "below"
    : "above"
  const desiredTop = placement === "below" ? below : above
  const maximumTop = Math.max(margin, viewport.height - size.height - margin)
  const top = Math.max(margin, Math.min(maximumTop, desiredTop))
  const desiredLeft = anchor.left + anchor.width / 2 - size.width / 2
  const maximumLeft = Math.max(margin, viewport.width - size.width - margin)
  const left = Math.max(margin, Math.min(maximumLeft, desiredLeft))
  return { left, placement, top }
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
      <p className="shortcut">{t("help.shortcut")}</p>
      <dl className="help-list">
        {items.map((item) => <div key={item.label}><dt>{t(item.label)}</dt><dd>{t(item.help)}</dd></div>)}
      </dl>
    </aside>
  )
}
