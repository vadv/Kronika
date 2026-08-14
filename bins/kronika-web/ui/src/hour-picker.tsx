import { useEffect, useMemo, useRef, useState } from "react"

import type { Translate } from "./help"
import { inputDay, inputHour, localHourPair, selectedHour, type Locale } from "./model"

const HOUR_US = 3_600_000_000
const ALL_HOURS = Array.from({ length: 24 }, (_, hour) => hour)

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
  const committedDay = hour === null ? "" : inputDay(hour)
  const hourNumber = hour === null ? 0 : inputHour(hour)
  const [open, setOpen] = useState(false)
  const [day, setDay] = useState(committedDay)
  const [month, setMonth] = useState(hour === null ? 0 : pickerMonthStart(hour))
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const hourCells = useRef<(HTMLButtonElement | null)[]>([])
  const recorded = useMemo(() => new Set(availableHours), [availableHours])
  const selectable = useMemo(() => new Set([...availableHours, hour].filter((candidate): candidate is number => candidate !== null)), [availableHours, hour])
  const availableDays = new Set([...selectable].map(inputDay))
  const dayHours = [...selectable].flatMap((candidate) => inputDay(candidate) === day ? [inputHour(candidate)] : [])
  dayHours.sort((left, right) => left - right)
  const months = [...new Set([...selectable].map(pickerMonthStart))].sort((left, right) => left - right)
  const monthIndex = months.indexOf(month)
  const label = hour === null ? null : localHourPair(hour, locale)

  useEffect(() => {
    if (!open) return
    const frame = requestAnimationFrame(() => hourCells.current[hourNumber]?.focus())
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

  const show = () => {
    if (hour === null) return
    if (open) {
      setOpen(false)
      return
    }
    setDay(committedDay)
    setMonth(pickerMonthStart(hour))
    setOpen(true)
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
    <button aria-label={t("hour.previous")} data-testid="hour-previous" disabled={hour === null} onClick={() => { setOpen(false); if (hour !== null) changeHour(hour - HOUR_US) }} type="button">‹</button>
    <button
      aria-expanded={open}
      aria-haspopup="dialog"
      aria-controls="hour-picker-popover"
      aria-label={label === null ? t("hour.picker") : `${t("hour.picker")}: ${label.date} · ${label.primary}`}
      className="hour-trigger"
      data-testid="hour-picker-trigger"
      disabled={hour === null}
      onClick={show}
      ref={trigger}
      type="button"
    >
      <strong>{label?.primary ?? "—"}</strong>
      {label !== null && <small>{label.date}</small>}
    </button>
    <button aria-label={t("hour.next")} data-testid="hour-next" disabled={hour === null} onClick={() => { setOpen(false); if (hour !== null) changeHour(hour + HOUR_US) }} type="button">›</button>
    {open && hour !== null && <div aria-label={t("hour.picker")} className="hour-popover" data-testid="hour-popover" id="hour-picker-popover" role="dialog">
      <header>
        <strong data-testid="hour-current">{pickerDateLabel(hour, locale)}</strong>
        <span>{t("hour.context")}</span>
      </header>
      <div className="day-picker">
        <div className="month-navigation">
          <button aria-label={t("hour.month.previous")} disabled={monthIndex <= 0} onClick={() => setMonth(months[monthIndex - 1] ?? month)} type="button">‹</button>
          <strong aria-live="polite" data-testid="picker-month">{pickerMonthLabel(month, locale)}</strong>
          <button aria-label={t("hour.month.next")} disabled={monthIndex >= months.length - 1} onClick={() => setMonth(months[monthIndex + 1] ?? month)} type="button">›</button>
        </div>
        <div aria-label={t("hour.picker")} className="day-grid" role="group">
          {pickerMonthDays(month).map((candidate) => {
            const hasData = availableDays.has(candidate)
            return <button aria-label={pickerDateLabel(selectedHour(candidate, 0) ?? 0, locale)} aria-pressed={candidate === day} className={hasData ? "day-cell day-cell-available" : "day-cell"} data-day={candidate} disabled={!hasData} key={candidate} onClick={() => setDay(candidate)} type="button">{candidate.slice(-2)}</button>
          })}
        </div>
      </div>
      <div aria-label={t("hour.hours")} className="hour-grid" role="group">
          {ALL_HOURS.map((number) => {
            const cellHour = selectedHour(day, number)
            const hasData = cellHour !== null && selectable.has(cellHour)
            const captured = cellHour !== null && recorded.has(cellHour)
            return <button aria-label={`${twoDigits(number)}:00${captured ? ` · ${t("hour.available")}` : ""}`} aria-pressed={cellHour === hour} className="hour-cell" data-available={captured ? "true" : "false"} data-hour={twoDigits(number)} disabled={!hasData} key={number} onClick={() => { if (cellHour !== null && hasData) { changeHour(cellHour); setOpen(false); trigger.current?.focus() } }} onKeyDown={(event) => { const next = pickerFocusHour(number, event.key, dayHours); if (next !== null) { event.preventDefault(); hourCells.current[next]?.focus() } }} ref={(node) => { hourCells.current[number] = node }} tabIndex={number === (day === committedDay ? hourNumber : dayHours[0]) ? 0 : -1} type="button">{twoDigits(number)}</button>
          })}
      </div>
    </div>}
  </div>
}

export function pickerRangeLabel(hour: number): string {
  return `${twoDigits(inputHour(hour))}:00–${twoDigits(inputHour(hour + HOUR_US))}:00`
}

export function pickerDateLabel(hour: number, locale: Locale): string {
  return new Intl.DateTimeFormat(locale, {
    day: "2-digit",
    month: "short",
    timeZone: "UTC",
    year: "numeric",
  }).format(new Date(hour / 1_000))
}

export function pickerMonthLabel(month: number, locale: Locale): string {
  return new Intl.DateTimeFormat(locale, { month: "long", timeZone: "UTC", year: "numeric" }).format(new Date(month / 1_000))
}

export function pickerMonthStart(hour: number): number {
  const date = new Date(hour / 1_000)
  return Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1) * 1_000
}

export function pickerMonthDays(month: number): readonly string[] {
  const date = new Date(month / 1_000)
  const year = date.getUTCFullYear()
  const monthNumber = date.getUTCMonth()
  const count = new Date(Date.UTC(year, monthNumber + 1, 0)).getUTCDate()
  return Array.from({ length: count }, (_, day) => inputDay(Date.UTC(year, monthNumber, day + 1) * 1_000))
}

export function pickerFocusHour(current: number, key: string, enabled: readonly number[] = ALL_HOURS): number | null {
  if (key === "Home") return enabled[0] ?? null
  if (key === "End") return enabled.at(-1) ?? null
  const step = key === "ArrowLeft" ? -1 : key === "ArrowRight" ? 1 : key === "ArrowUp" ? -6 : key === "ArrowDown" ? 6 : 0
  if (step === 0) return null
  return enabled.filter((hour) => Math.sign(hour - current) === Math.sign(step)).sort((a, b) => Math.abs(a - current - step) - Math.abs(b - current - step))[0] ?? current
}

export function hourHasData(hour: number, available: readonly number[]): boolean {
  return available.includes(hour)
}

function twoDigits(number: number): string {
  return String(number).padStart(2, "0")
}
