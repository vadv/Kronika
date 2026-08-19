import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type MutableRefObject, type PointerEvent as ReactPointerEvent, type ReactNode } from "react"
import { createPortal } from "react-dom"

import type { InspectorPanel } from "./address"
import type { Translate } from "./help"

const MIN_WIDTH = 320
const MAX_WIDTH = 520
const DEFAULT_WIDTH = 360
const WIDTH_KEY = "kronika.inspector-width"

interface PortalController {
  readonly target: HTMLElement | null
  readonly register: (identity: string, dismiss: () => void, title: string, autoOpen: boolean) => () => void
}

const InspectorPortalContext = createContext<PortalController | null>(null)

export function InspectorPortalProvider({ children, dismissRef, onOpen, onTitle, target }: {
  readonly children: ReactNode
  readonly dismissRef: MutableRefObject<(() => void) | null>
  readonly onOpen: () => void
  readonly onTitle: (title: string | null) => void
  readonly target: HTMLElement | null
}) {
  const active = useRef<string | null>(null)
  const callbacks = useRef({ dismissRef, onOpen, onTitle })
  callbacks.current = { dismissRef, onOpen, onTitle }
  const register = useMemo(() => (identity: string, dismiss: () => void, title: string, autoOpen: boolean) => {
    active.current = identity
    callbacks.current.dismissRef.current = dismiss
    callbacks.current.onTitle(title)
    if (autoOpen) callbacks.current.onOpen()
    return () => {
      if (active.current !== identity) return
      active.current = null
      callbacks.current.dismissRef.current = null
      callbacks.current.onTitle(null)
    }
  }, [])
  return <InspectorPortalContext.Provider value={{ register, target }}>{children}</InspectorPortalContext.Provider>
}

export function InspectorPortal({ autoOpen = true, children, identity, onClose, title }: {
  readonly autoOpen?: boolean | undefined
  readonly children: ReactNode
  readonly identity: string
  readonly onClose: () => void
  readonly title: string
}) {
  const controller = useContext(InspectorPortalContext)
  const dismiss = useRef(onClose)
  dismiss.current = onClose
  useLayoutEffect(() => controller?.register(identity, () => dismiss.current(), title, autoOpen), [autoOpen, controller?.register, identity, title])
  return controller === null || controller.target === null ? null : createPortal(children, controller.target)
}

export function loadInspectorWidth(storage: Pick<Storage, "getItem">): number {
  try {
    const stored = Number.parseInt(storage.getItem(WIDTH_KEY) ?? "", 10)
    return Number.isFinite(stored) ? clampWidth(stored) : DEFAULT_WIDTH
  } catch {
    return DEFAULT_WIDTH
  }
}

export function clampWidth(width: number): number {
  return Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, Math.round(width)))
}

export function Inspector({
  chart,
  detail,
  detailAvailable,
  onClose,
  onPanel,
  panel,
  title,
  t,
}: {
  readonly chart: ReactNode
  readonly detail: ReactNode
  readonly detailAvailable: boolean
  readonly onClose: () => void
  readonly onPanel: (panel: Exclude<InspectorPanel, null>) => void
  readonly panel: Exclude<InspectorPanel, null>
  readonly title: string
  readonly t: Translate
}) {
  const [width, setWidth] = useState(() => loadInspectorWidth(localStorage))
  const root = useRef<HTMLElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const style = { "--inspector-width": `${width}px` } as CSSProperties

  useLayoutEffect(() => {
    const active = document.activeElement
    if (active instanceof HTMLElement && active !== document.body && !root.current?.contains(active)) opener.current = active
    requestAnimationFrame(() => root.current?.focus({ preventScroll: true }))
  }, [])

  useEffect(() => {
    try { localStorage.setItem(WIDTH_KEY, String(width)) } catch {}
  }, [width])

  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) {
        event.preventDefault()
        onClose()
        requestAnimationFrame(() => opener.current?.isConnected && opener.current.focus({ preventScroll: true }))
        return
      }
      if (event.key !== "Tab" || !window.matchMedia("(max-width: 1000px)").matches) return
      const focusable = [...(root.current?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), [href], [tabindex]:not([tabindex="-1"])') ?? [])]
      const first = focusable[0]
      const last = focusable.at(-1)
      if (first === undefined || last === undefined) return
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    window.addEventListener("keydown", keydown)
    return () => window.removeEventListener("keydown", keydown)
  }, [onClose])

  const beginResize = (event: ReactPointerEvent<HTMLButtonElement>) => {
    event.preventDefault()
    const start = event.clientX
    const initial = width
    const move = (next: PointerEvent) => setWidth(clampWidth(initial + start - next.clientX))
    const finish = () => {
      window.removeEventListener("pointermove", move)
      window.removeEventListener("pointerup", finish)
    }
    window.addEventListener("pointermove", move)
    window.addEventListener("pointerup", finish, { once: true })
  }

  return <>
    <button aria-label={t("inspector.close")} className="inspector-scrim" onClick={onClose} type="button" />
    <aside aria-label={t("inspector.title")} className="inspector" data-panel={panel} data-testid="inspector" ref={root} role="dialog" style={style} tabIndex={-1}>
      <button
        aria-label={t("inspector.resize")}
        aria-orientation="vertical"
        aria-valuemax={MAX_WIDTH}
        aria-valuemin={MIN_WIDTH}
        aria-valuenow={width}
        className="inspector-splitter"
        onKeyDown={(event) => {
          if (event.key !== "ArrowLeft" && event.key !== "ArrowRight" && event.key !== "Home" && event.key !== "End") return
          event.preventDefault()
          setWidth(event.key === "Home" ? MIN_WIDTH : event.key === "End" ? MAX_WIDTH : clampWidth(width + (event.key === "ArrowLeft" ? 16 : -16)))
        }}
        onPointerDown={beginResize}
        role="separator"
        type="button"
      />
      <header className="inspector-head">
        <div className="inspector-tabs" role="tablist">
          <button aria-selected={panel === "detail"} disabled={!detailAvailable} onClick={() => onPanel("detail")} role="tab" type="button">{t("inspector.detail")}</button>
          <button aria-selected={panel === "chart"} onClick={() => onPanel("chart")} role="tab" type="button">{t("inspector.chart")}</button>
        </div>
        <strong title={title}>{title}</strong>
        <button aria-label={t("common.close")} className="inspector-close" onClick={onClose} type="button">×</button>
      </header>
      <div className="inspector-body" data-testid={`inspector-${panel}`} role="tabpanel">
        {panel === "detail" && detailAvailable ? detail : chart}
      </div>
    </aside>
  </>
}
