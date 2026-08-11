import { ChevronLeft, ChevronRight } from "lucide-react"
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react"

import type { Translate } from "./help"
import { inputDay, inputHour, selectedHour, type Locale } from "./model"

const HOUR_US = 3_600_000_000
const DAY_US = 24 * HOUR_US

export function HourPicker({
  availableHours,
  changeHour,
  hour,
  locale,
  t,
}: {
  readonly availableHours: readonly number[]
  readonly changeHour: (hour: number) => void
  readonly hour: number | null
  readonly locale: Locale
  readonly t: Translate
}) {
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const hourCells = useRef<(HTMLButtonElement | null)[]>([])
  const hourNumber = hour === null ? 0 : inputHour(hour)
  const day = hour === null ? "" : inputDay(hour)
  const available = useMemo(() => new Set(availableHours), [availableHours])

  useEffect(() => {
    if (!open) return
    const selected = hourCells.current[hourNumber]
    const frame = requestAnimationFrame(() => selected?.focus())
    const dismiss = (event: PointerEvent) => {
      if (event.target instanceof Node && root.current?.contains(event.target)) return
      setOpen(false)
    }
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      setOpen(false)
      trigger.current?.focus()
    }
    document.addEventListener("pointerdown", dismiss, true)
    window.addEventListener("keydown", escape)
    return () => {
      cancelAnimationFrame(frame)
      document.removeEventListener("pointerdown", dismiss, true)
      window.removeEventListener("keydown", escape)
    }
  }, [hourNumber, open])

  const select = (number: number) => {
    const next = selectedHour(day, number)
    if (next !== null) changeHour(next)
    setOpen(false)
    trigger.current?.focus()
  }
  const moveDay = (direction: -1 | 1) => {
    if (hour === null) return
    changeHour(hour + direction * DAY_US)
    requestAnimationFrame(() => hourCells.current[hourNumber]?.focus())
  }
  const moveFocus = (event: KeyboardEvent<HTMLButtonElement>, number: number) => {
    if (event.key === "PageUp" || event.key === "PageDown") {
      event.preventDefault()
      moveDay(event.key === "PageUp" ? -1 : 1)
      return
    }
    const next = pickerFocusHour(number, event.key)
    if (next === null) return
    event.preventDefault()
    hourCells.current[next]?.focus()
  }

  return <div
    className="hour-picker"
    data-testid="hour-picker"
    onBlur={(event) => {
      if (event.relatedTarget instanceof Node && root.current?.contains(event.relatedTarget)) return
      setOpen(false)
    }}
    ref={root}
  >
    <button aria-label={t("hour.previous")} data-testid="hour-previous" disabled={hour === null} onClick={() => hour !== null && changeHour(hour - HOUR_US)} type="button"><ChevronLeft aria-hidden="true" size={15} /></button>
    <button
      aria-expanded={open}
      aria-haspopup="dialog"
      aria-controls="hour-picker-popover"
      aria-label={hour === null ? t("hour.picker") : t("hour.open", { date: pickerDateLabel(hour, locale), range: pickerRangeLabel(hour) })}
      className="hour-trigger"
      data-testid="hour-picker-trigger"
      disabled={hour === null}
      onClick={() => setOpen((current) => !current)}
      ref={trigger}
      type="button"
    >
      <strong>{hour === null ? "—" : pickerRangeLabel(hour)}</strong>
      {hour !== null && <small>{pickerDateLabel(hour, locale)} · UTC</small>}
    </button>
    <button aria-label={t("hour.next")} data-testid="hour-next" disabled={hour === null} onClick={() => hour !== null && changeHour(hour + HOUR_US)} type="button"><ChevronRight aria-hidden="true" size={15} /></button>
    {open && hour !== null && <div aria-label={t("hour.picker")} className="hour-popover" data-testid="hour-popover" id="hour-picker-popover" role="dialog">
      <header>
        <strong>{pickerDateLabel(hour, locale)}</strong>
        <span>{t("hour.context")}</span>
      </header>
      <div aria-label={t("hour.hours")} className="hour-grid" role="group">
        {Array.from({ length: 24 }, (_, number) => {
          const cellHour = selectedHour(day, number)
          const hasData = cellHour !== null && available.has(cellHour)
          const label = `${twoDigits(number)}:00${hasData ? ` · ${t("hour.available")}` : ""}`
          return <button
            aria-label={label}
            aria-pressed={number === hourNumber}
            className={hasData ? "hour-cell hour-cell-available" : "hour-cell"}
            data-available={hasData ? "true" : "false"}
            data-hour={twoDigits(number)}
            key={number}
            onClick={() => select(number)}
            onKeyDown={(event) => moveFocus(event, number)}
            ref={(node) => { hourCells.current[number] = node }}
            tabIndex={number === hourNumber ? 0 : -1}
            type="button"
          >{twoDigits(number)}</button>
        })}
      </div>
    </div>}
  </div>
}

export function pickerRangeLabel(hour: number): string {
  return `${twoDigits(inputHour(hour))}:00–${twoDigits(inputHour(hour + HOUR_US))}:00`
}

export function pickerDateLabel(hour: number, locale: Locale): string {
  const parts = new Intl.DateTimeFormat(locale, {
    day: "2-digit",
    month: "short",
    timeZone: "UTC",
    year: "numeric",
  }).formatToParts(new Date(hour / 1_000))
  const part = (type: Intl.DateTimeFormatPartTypes) => parts.find((candidate) => candidate.type === type)?.value ?? ""
  return [part("day"), part("month"), part("year")].filter((value) => value !== "").join(" ")
}

export function pickerFocusHour(current: number, key: string): number | null {
  if (key === "ArrowLeft") return Math.max(0, current - 1)
  if (key === "ArrowRight") return Math.min(23, current + 1)
  if (key === "ArrowUp") return Math.max(0, current - 6)
  if (key === "ArrowDown") return Math.min(23, current + 6)
  if (key === "Home") return 0
  if (key === "End") return 23
  return null
}

export function hourHasData(hour: number, available: readonly number[]): boolean {
  return available.includes(hour)
}

function twoDigits(number: number): string {
  return String(number).padStart(2, "0")
}
