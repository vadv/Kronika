import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"

import { calendarDateLabel, calendarMonthDays, calendarMonthLabel, type DisplayTimeFormatter } from "./display-time"
import { useDisplayTime } from "./display-time-context"
import type { Translate } from "./help"
import type { Locale } from "./model"

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
  const time = useDisplayTime()
  const selectable = useMemo(() => catalogueHours(availableHours, hour), [availableHours, hour])
  const committedDay = hour === null ? "" : time.dayKey(hour)
  const committedMonth = hour === null ? "" : time.monthKey(hour)
  const [open, setOpen] = useState(false)
  const [day, setDay] = useState(committedDay)
  const [month, setMonth] = useState(committedMonth)
  const [popover, setPopover] = useState<HourPopoverPlacement | null>(null)
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const hourCells = useRef<(HTMLButtonElement | null)[]>([])
  const availableDays = useMemo(() => new Set(selectable.map((candidate) => time.dayKey(candidate))), [selectable, time])
  const dayHours = useMemo(() => hoursForDay(selectable, day, time), [day, selectable, time])
  const months = useMemo(() => [...new Set(selectable.map((candidate) => time.monthKey(candidate)))].sort(), [selectable, time])
  const monthIndex = months.indexOf(month)
  const currentIndex = hour === null ? -1 : selectable.indexOf(hour)
  const label = hour === null ? null : time.hourRange(hour)

  useEffect(() => {
    if (!open) return
    const selected = dayHours.indexOf(hour ?? Number.NaN)
    const frame = requestAnimationFrame(() => hourCells.current[Math.max(0, selected)]?.focus())
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
  }, [dayHours, hour, open])

  useLayoutEffect(() => {
    if (!open) return
    const place = () => {
      const anchor = root.current?.getBoundingClientRect()
      if (anchor === undefined) return
      setPopover(hourPopoverPlacement(anchor, {
        compact: window.matchMedia("(max-width: 760px)").matches,
        height: window.innerHeight,
        width: document.documentElement.clientWidth,
      }))
    }
    place()
    window.addEventListener("resize", place)
    return () => window.removeEventListener("resize", place)
  }, [open])

  const show = () => {
    if (hour === null) return
    if (open) return setOpen(false)
    setDay(committedDay)
    setMonth(committedMonth)
    setOpen(true)
  }
  const move = (index: number) => {
    const candidate = selectable[index]
    setOpen(false)
    if (candidate !== undefined) changeHour(candidate)
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
    <button aria-label={t("hour.previous")} data-testid="hour-previous" disabled={currentIndex <= 0} onClick={() => move(currentIndex - 1)} type="button">‹</button>
    <button
      aria-controls="hour-picker-popover"
      aria-expanded={open}
      aria-haspopup="dialog"
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
    <button aria-label={t("hour.next")} data-testid="hour-next" disabled={currentIndex < 0 || currentIndex >= selectable.length - 1} onClick={() => move(currentIndex + 1)} type="button">›</button>
    {open && hour !== null && <div aria-label={t("hour.picker")} className="hour-popover" data-testid="hour-popover" id="hour-picker-popover" role="dialog" style={popover ?? undefined}>
      <header>
        <strong data-testid="hour-current">{calendarDateLabel(time.dayKey(hour), locale)}</strong>
      </header>
      <div className="day-picker">
        <div className="month-navigation">
          <button aria-label={t("hour.month.previous")} disabled={monthIndex <= 0} onClick={() => setMonth(months[monthIndex - 1] ?? month)} type="button">‹</button>
          <strong aria-live="polite" data-testid="picker-month">{calendarMonthLabel(month, locale)}</strong>
          <button aria-label={t("hour.month.next")} disabled={monthIndex < 0 || monthIndex >= months.length - 1} onClick={() => setMonth(months[monthIndex + 1] ?? month)} type="button">›</button>
        </div>
        <div aria-label={t("hour.picker")} className="day-grid" role="group">
          {calendarMonthDays(month).map((candidate) => {
            const hasData = availableDays.has(candidate)
            return <button aria-label={calendarDateLabel(candidate, locale)} aria-pressed={candidate === day} className={hasData ? "day-cell day-cell-available" : "day-cell"} data-day={candidate} disabled={!hasData} key={candidate} onClick={() => setDay(candidate)} type="button">{candidate.slice(-2)}</button>
          })}
        </div>
      </div>
      <div aria-label={t("hour.hours")} className="hour-grid" role="group">
        {dayHours.map((candidate, index) => {
          const clock = time.hourLabel(candidate)
          return <button aria-label={clock} aria-pressed={candidate === hour} className="hour-cell" data-instant={candidate} key={candidate} onClick={() => { changeHour(candidate); setOpen(false); trigger.current?.focus() }} onKeyDown={(event) => { const next = pickerFocusIndex(index, event.key, dayHours.length); if (next !== null) { event.preventDefault(); hourCells.current[next]?.focus() } }} ref={(node) => { hourCells.current[index] = node }} tabIndex={candidate === hour || hour !== null && time.dayKey(hour) !== day && index === 0 ? 0 : -1} type="button">{clock}</button>
        })}
      </div>
    </div>}
  </div>
}

interface HourPopoverPlacement {
  readonly left: number
  readonly maxHeight: number
  readonly top: number
  readonly width: number
}

export function hourPopoverPlacement(
  anchor: { readonly bottom: number; readonly left: number },
  viewport: { readonly compact?: boolean; readonly height: number; readonly width: number },
): HourPopoverPlacement {
  const edge = 10
  const compact = viewport.compact ?? viewport.width <= 760
  const width = Math.max(0, Math.min(compact ? 304 : 560, viewport.width - 2 * edge))
  const left = Math.min(Math.max(edge, anchor.left), Math.max(edge, viewport.width - width - edge))
  const top = Math.max(edge, anchor.bottom + 6)
  return { left, maxHeight: Math.max(0, viewport.height - top - edge), top, width }
}

export function catalogueHours(available: readonly number[], current: number | null): readonly number[] {
  return [...new Set([...available, current].filter((candidate): candidate is number => candidate !== null && Number.isSafeInteger(candidate)))].sort((left, right) => left - right)
}

export function hoursForDay(hours: readonly number[], day: string, time: Pick<DisplayTimeFormatter, "dayKey">): readonly number[] {
  return hours.filter((candidate) => time.dayKey(candidate) === day)
}

export function pickerFocusIndex(current: number, key: string, length: number): number | null {
  if (length === 0) return null
  if (key === "Home") return 0
  if (key === "End") return length - 1
  const step = key === "ArrowLeft" ? -1 : key === "ArrowRight" ? 1 : key === "ArrowUp" ? -3 : key === "ArrowDown" ? 3 : 0
  return step === 0 ? null : Math.max(0, Math.min(length - 1, current + step))
}

export function hourHasData(hour: number, available: readonly number[]): boolean {
  return available.includes(hour)
}
