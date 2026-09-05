import { CalendarDays, ChevronLeft, ChevronRight, Download, X } from "lucide-react"
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react"

import type { SegmentBound } from "./api"
import { calendarDateLabel, calendarMonthDays, calendarMonthLabel } from "./display-time"
import { useDisplayTime } from "./display-time-context"
import { ExportResponseError, ExportServerStateUnknownError, fetchExportArtifact, triggerHtmlDownload } from "./export-download"
import {
  EXPORT_PRESETS,
  activePreset,
  exportFilename,
  hourRange,
  presetRange,
  rangeCoverage,
  rangeSeconds,
  readLastExportSeconds,
  sameRange,
  shiftRange,
  validRange,
  writeLastExportSeconds,
  type ExportRange,
} from "./export-range"
import { formatExportDuration, resolveExportEndpoint } from "./export-time"
import type { Translate } from "./help"
import { humanBytes, measure, type Locale } from "./model"
import { apiFetch } from "./session"

type Endpoint = "from" | "to"

type ExportJob =
  | { readonly phase: "idle" }
  | { readonly phase: "preparing"; readonly range: ExportRange; readonly startedAt: number }
  | { readonly phase: "downloading"; readonly range: ExportRange; readonly received: number; readonly startedAt: number; readonly total: number | null }
  | { readonly phase: "saved"; readonly name: string; readonly seconds: number; readonly size: number }
  | { readonly phase: "unknown"; readonly range: ExportRange }

interface EndpointDraft {
  readonly text: string
  readonly error: string | null
  readonly candidates: readonly number[]
}

const SERVER_ERROR_KEYS = new Set(["bad_parameter", "export_busy", "export_empty", "export_failed"])
const MICROS = 1_000_000
const HOUR_MICROS = 3_600 * MICROS

// The strip sits under the timeline and describes a selection on it. The
// range comes from context first: the hour, a window around the cursor, a
// shift by an hour; the exact seconds are one editable line, and the day
// picker offers only days that hold recordings. What the file will contain and
// how it will be named is said before anything is requested. States are facts:
// seconds spent, bytes received, the saved name. No progress bars.
export function ExportStrip({ availableHours, cursor, hour, locale, onActiveChange, onClose, onRange, range, segments, t }: {
  readonly availableHours: readonly number[]
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onActiveChange: (active: boolean) => void
  readonly onClose: () => void
  readonly onRange: (range: ExportRange) => void
  readonly range: ExportRange
  readonly segments: readonly SegmentBound[]
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const [job, setJob] = useState<ExportJob>({ phase: "idle" })
  const [message, setMessage] = useState<string | null>(null)
  const [now, setNow] = useState(() => performance.now())
  const [dayPicker, setDayPicker] = useState<Endpoint | null>(null)
  const [drafts, setDrafts] = useState<Readonly<Record<Endpoint, EndpointDraft>>>(() => ({
    from: { text: time.clock(range.from * MICROS), error: null, candidates: [] },
    to: { text: time.clock(range.to * MICROS), error: null, candidates: [] },
  }))
  const [lastSeconds, setLastSeconds] = useState(() => readLastExportSeconds(localStorage))
  const activeJob = useRef(false)
  const previousHour = useRef(hour)
  const strip = useRef<HTMLElement>(null)
  const firstPreset = useRef<HTMLButtonElement>(null)
  const titleId = useId()
  const statusId = useId()
  const busy = job.phase === "preparing" || job.phase === "downloading" || job.phase === "unknown"
  const preset = activePreset(range, hour, cursor)
  const duration = validRange(range) ? formatExportDuration(rangeSeconds(range), locale) : "—"
  const coverage = useMemo(() => rangeCoverage(range, segments), [range, segments])
  const filename = exportFilename(range)
  const orderError = range.from > range.to

  // The drafts follow the range while nobody is typing an invalid value.
  useEffect(() => {
    setDrafts((current) => ({
      from: current.from.error === null ? { text: time.clock(range.from * MICROS), error: null, candidates: [] } : current.from,
      to: current.to.error === null ? { text: time.clock(range.to * MICROS), error: null, candidates: [] } : current.to,
    }))
  }, [range.from, range.to, time])

  // The hour moved on: a range that was this hour follows it, an edited one stays.
  useEffect(() => {
    if (previousHour.current === hour) return
    const before = previousHour.current
    previousHour.current = hour
    if (!activeJob.current && sameRange(range, hourRange(before))) onRange(hourRange(hour))
  }, [hour, onRange, range])

  useLayoutEffect(() => {
    firstPreset.current?.focus({ preventScroll: true })
  }, [])

  useEffect(() => {
    if (job.phase !== "preparing") return
    setNow(performance.now())
    const timer = window.setInterval(() => setNow(performance.now()), 250)
    return () => window.clearInterval(timer)
  }, [job.phase])

  const close = useCallback(() => {
    if (activeJob.current) return
    onClose()
  }, [onClose])

  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      if (dayPicker !== null) {
        event.preventDefault()
        event.stopPropagation()
        setDayPicker(null)
        return
      }
      if (activeJob.current) {
        if (strip.current?.contains(document.activeElement)) event.stopPropagation()
        return
      }
      event.preventDefault()
      event.stopPropagation()
      close()
    }
    window.addEventListener("keydown", escape, true)
    return () => window.removeEventListener("keydown", escape, true)
  }, [close, dayPicker])

  const choose = (next: ExportRange) => {
    if (activeJob.current) return
    setMessage(null)
    setDrafts({
      from: { text: time.clock(next.from * MICROS), error: null, candidates: [] },
      to: { text: time.clock(next.to * MICROS), error: null, candidates: [] },
    })
    onRange(next)
  }

  // A typed clock is resolved against the endpoint's own civil date in the
  // shown zone; a repeated clock at a backward transition asks which one.
  const editTime = (endpoint: Endpoint, text: string) => {
    if (activeJob.current) return
    setMessage(null)
    const dayKey = time.dayKey(range[endpoint] * MICROS)
    const resolution = resolveExportEndpoint(dayKey, text.trim(), time.mode, { occurrence: null, preferred: range[endpoint] })
    if (resolution.second !== null) {
      setDrafts((current) => ({ ...current, [endpoint]: { text, error: null, candidates: [] } }))
      onRange({ ...range, [endpoint]: resolution.second })
      return
    }
    const error = resolution.error === "occurrence_required"
      ? null
      : resolution.error === "nonexistent" ? t("export.error.nonexistent") : t("export.error.time")
    setDrafts((current) => ({ ...current, [endpoint]: { text, error, candidates: resolution.candidates } }))
  }
  const chooseOccurrence = (endpoint: Endpoint, second: number) => {
    setDrafts((current) => ({ ...current, [endpoint]: { text: time.clock(second * MICROS), error: null, candidates: [] } }))
    onRange({ ...range, [endpoint]: second })
  }
  // A day from the picker keeps the endpoint's clock; a clock the new day lacks
  // or repeats takes the first instant.
  const chooseDay = (endpoint: Endpoint, dayKey: string) => {
    const clock = time.clock(range[endpoint] * MICROS)
    const resolution = resolveExportEndpoint(dayKey, clock, time.mode, { occurrence: 0, preferred: null })
    const second = resolution.second ?? resolution.candidates[0] ?? null
    setDayPicker(null)
    if (second !== null) choose({ ...range, [endpoint]: second })
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (activeJob.current) return
    if (drafts.from.error !== null || drafts.to.error !== null || drafts.from.candidates.length > 1 || drafts.to.candidates.length > 1) return
    if (!validRange(range)) {
      setMessage(t("export.error.order"))
      return
    }
    const startedAt = performance.now()
    const submitted = range
    activeJob.current = true
    onActiveChange(true)
    setMessage(null)
    setDayPicker(null)
    setNow(startedAt)
    setJob({ phase: "preparing", range: submitted, startedAt })
    let headersAt: number | null = null
    try {
      const artifact = await fetchExportArtifact(apiFetch, submitted.from, submitted.to, ({ received, total }) => {
        headersAt ??= performance.now()
        setJob({ phase: "downloading", range: submitted, received, startedAt, total })
      })
      triggerHtmlDownload(artifact.blob, artifact.filename)
      const seconds = ((headersAt ?? performance.now()) - startedAt) / 1_000
      writeLastExportSeconds(localStorage, seconds)
      setLastSeconds(seconds)
      activeJob.current = false
      onActiveChange(false)
      setJob({ phase: "saved", name: artifact.filename, seconds: Math.round((performance.now() - startedAt) / 1_000), size: artifact.blob.size })
    } catch (reason) {
      if (reason instanceof ExportServerStateUnknownError) {
        setJob({ phase: "unknown", range: submitted })
        return
      }
      activeJob.current = false
      onActiveChange(false)
      setJob({ phase: "idle" })
      setMessage(t(requestErrorKey(reason)))
    }
  }

  const clockOf = (second: number) => time.clock(second * MICROS)
  const dayOf = (second: number) => time.dayKey(second * MICROS)
  const hourDay = time.dayKey(hour)
  const withDay = (second: number) => dayOf(second) === hourDay ? clockOf(second) : `${time.date(second * MICROS)} · ${clockOf(second)}`
  const elapsed = job.phase === "preparing" ? Math.max(0, Math.round((now - job.startedAt) / 1_000)) : 0
  const status = job.phase === "preparing"
    ? lastSeconds === null
      ? t("export.preparing", { seconds: String(elapsed) })
      : t("export.preparing.last", { last: measure(Math.round(lastSeconds), locale), seconds: String(elapsed) })
    : job.phase === "downloading"
      ? job.total === null
        ? t("export.downloading", { received: humanBytes(job.received, locale) })
        : t("export.downloading_total", { received: humanBytes(job.received, locale), total: humanBytes(job.total, locale) })
      : job.phase === "saved"
        ? t("export.saved", { name: job.name, seconds: measure(job.seconds, locale), size: humanBytes(job.size, locale) })
        : job.phase === "unknown"
          ? t("export.error.server_state_unknown")
          : message ?? drafts.from.error ?? drafts.to.error ?? (orderError ? t("export.error.order") : null)
  const closeLabel = job.phase === "unknown" ? t("export.close_unknown") : busy ? t("export.close_busy") : t("export.close")

  const coverageText = coverage.recorded === null
    ? t("export.coverage.none")
    : [
      t("export.coverage.recorded", { from: withDay(coverage.recorded.from), to: withDay(coverage.recorded.to) }),
      coverage.gaps.length === 0
        ? t("export.coverage.no_gaps")
        : coverage.gaps.length === 1 && coverage.gaps[0] !== undefined
          ? t("export.coverage.gap", { from: clockOf(coverage.gaps[0].from), to: clockOf(coverage.gaps[0].to) })
          : t("export.coverage.gaps", { count: String(coverage.gaps.length) }),
    ].join(" · ")
  const unrecordedHours = useMemo(() => {
    const first = Math.floor(range.from * MICROS / HOUR_MICROS) * HOUR_MICROS
    const last = Math.floor(range.to * MICROS / HOUR_MICROS) * HOUR_MICROS
    let missing = 0
    for (let candidate = first; candidate <= last; candidate += HOUR_MICROS) {
      if (candidate !== hour && !availableHours.includes(candidate)) missing += 1
    }
    return missing
  }, [availableHours, hour, range.from, range.to])

  const endpointField = (endpoint: Endpoint) => {
    const draft = drafts[endpoint]
    const second = range[endpoint]
    const otherDay = dayOf(second) !== hourDay
    return <span className="export-endpoint inline-flex min-w-0 items-center gap-1" key={endpoint}>
      {otherDay && <span className="whitespace-nowrap font-sans text-sm text-fg3" data-testid={`export-${endpoint}-date`}>{time.date(second * MICROS)}</span>}
      <input
        aria-invalid={draft.error !== null || undefined}
        aria-label={t(`export.${endpoint}`)}
        autoComplete="off"
        className="export-clock h-8 w-[84px] rounded-[var(--radius-xs)] border border-line2 bg-s2 px-1.5 text-center font-mono text-sm tabular-nums text-fg focus-visible:outline-2 focus-visible:outline-accent coarse:h-11 coarse:w-[96px]"
        data-testid={`export-${endpoint}`}
        disabled={busy}
        inputMode="numeric"
        onChange={(event) => editTime(endpoint, event.currentTarget.value)}
        spellCheck={false}
        value={draft.text}
      />
      <button
        aria-expanded={dayPicker === endpoint}
        aria-label={t(`export.${endpoint}_day`)}
        className="icon-button coarse:!h-11 coarse:!w-11"
        data-testid={`export-${endpoint}-day`}
        disabled={busy}
        onClick={() => setDayPicker((current) => current === endpoint ? null : endpoint)}
        title={t(`export.${endpoint}_day`)}
        type="button"
      ><CalendarDays aria-hidden="true" size={14} /></button>
      {draft.candidates.length > 1 && <span className="inline-flex items-center gap-1 font-sans text-sm text-fg3" data-testid={`export-${endpoint}-occurrence`} role="group">
        <span>{t("export.occurrence")}</span>
        {draft.candidates.map((candidate, index) => <button className="h-7 cursor-pointer rounded-[var(--radius-xs)] border border-line2 bg-s2 px-1.5 font-sans text-sm text-fg2 hover:bg-s3 coarse:h-11" key={candidate} onClick={() => chooseOccurrence(endpoint, candidate)} type="button">{t(index === 0 ? "export.occurrence.first" : "export.occurrence.second")}</button>)}
      </span>}
    </span>
  }

  return <section
    aria-busy={busy}
    aria-describedby={statusId}
    aria-labelledby={titleId}
    className="export-strip relative flex-none border-b border-line2 bg-s1 text-fg"
    data-phase={job.phase}
    data-range-from={range.from}
    data-range-to={range.to}
    data-testid="export-strip"
    ref={strip}
    role="region"
  >
    <button aria-label={closeLabel} className="icon-button absolute right-2 top-1.5 coarse:!h-11 coarse:!w-11" data-testid="export-close" disabled={busy} onClick={close} title={closeLabel} type="button"><X aria-hidden="true" size={14} /></button>
    <form aria-busy={busy} className="grid gap-x-3 gap-y-1.5 py-1.5 pl-2.5 pr-11 coarse:pr-14" data-testid="export-form" noValidate onSubmit={(event) => { void submit(event) }}>
      <div className="export-strip-row flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5">
        <h2 className="m-0 font-sans text-sm font-semibold text-fg2" id={titleId}>{t("export.title")}</h2>
        <div aria-label={t("export.presets")} className="export-presets flex flex-wrap items-center gap-1" role="group">
          {EXPORT_PRESETS.map((candidate, index) => <button
            aria-pressed={preset === candidate}
            className="export-chip h-7 cursor-pointer whitespace-nowrap rounded-[var(--radius-xs)] border border-line2 bg-s2 px-2 font-sans text-sm text-fg2 hover:bg-s3 aria-pressed:border-accent aria-pressed:bg-accent-soft aria-pressed:text-fg coarse:h-11"
            data-testid={`export-preset-${candidate}`}
            disabled={busy}
            key={candidate}
            onClick={() => choose(presetRange(candidate, hour, cursor))}
            ref={index === 0 ? firstPreset : undefined}
            type="button"
          >{t(`export.preset.${candidate}`)}</button>)}
          <button aria-label={t("export.shift.back")} className="icon-button coarse:!h-11 coarse:!w-11" data-testid="export-shift-back" disabled={busy} onClick={() => choose(shiftRange(range, -3_600))} title={t("export.shift.back")} type="button"><ChevronLeft aria-hidden="true" size={14} /></button>
          <button aria-label={t("export.shift.forward")} className="icon-button coarse:!h-11 coarse:!w-11" data-testid="export-shift-forward" disabled={busy} onClick={() => choose(shiftRange(range, 3_600))} title={t("export.shift.forward")} type="button"><ChevronRight aria-hidden="true" size={14} /></button>
        </div>
        <div className="export-exact flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1">
          {endpointField("from")}
          <span aria-hidden="true" className="font-mono text-sm text-fg4">→</span>
          {endpointField("to")}
          <strong aria-label={t("export.duration")} className="ml-1 whitespace-nowrap font-mono text-sm font-medium tabular-nums text-fg" data-testid="export-duration">{duration}</strong>
        </div>
      </div>
      <div className="export-strip-row flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
        <span className="min-w-0 font-sans text-sm text-fg3" data-testid="export-coverage">
          {coverageText}
          {unrecordedHours > 0 && <> · {t("export.coverage.outside", { hours: String(unrecordedHours) })}</>}
        </span>
        <span className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-sm text-fg3" data-testid="export-filename" title={filename}>{t("export.filename", { name: filename })}</span>
        <span className={`export-status min-w-0 font-sans text-sm ${message !== null || job.phase === "unknown" || orderError ? "text-warn" : "text-fg2"}`} data-testid="export-status" id={statusId}>
          {status}
        </span>
        <span aria-atomic="true" aria-live="polite" className="sr-only">{job.phase === "preparing" ? t("export.preparing.phase") : status ?? ""}</span>
        {job.phase === "saved"
          ? <button className="export-primary ml-auto inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-[var(--radius-sm)] border border-line2 bg-s2 px-3 font-sans text-sm font-semibold text-fg2 hover:bg-s3 coarse:h-11" data-testid="export-again" onClick={() => { setJob({ phase: "idle" }); setMessage(null) }} type="button">{t("export.again")}</button>
          : <button className="export-primary ml-auto inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-[var(--radius-sm)] border-0 bg-accent px-3 font-sans text-sm font-semibold text-bg hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-50 coarse:h-11" data-testid="export-submit" disabled={busy || orderError || drafts.from.error !== null || drafts.to.error !== null} type="submit"><Download aria-hidden="true" size={14} />{t("export.submit")}</button>}
      </div>
    </form>
    {dayPicker !== null && <DayPicker
      availableHours={availableHours}
      dayKey={dayOf(range[dayPicker])}
      label={t(dayPicker === "from" ? "export.from_day" : "export.to_day")}
      locale={locale}
      onChoose={(day) => chooseDay(dayPicker, day)}
      onClose={() => setDayPicker(null)}
      t={t}
    />}
  </section>
}

// Only days that hold recordings can be chosen: the picker is a fact about the
// store, not a month of guesses.
function DayPicker({ availableHours, dayKey, label, locale, onChoose, onClose, t }: {
  readonly availableHours: readonly number[]
  readonly dayKey: string
  readonly label: string
  readonly locale: Locale
  readonly onChoose: (dayKey: string) => void
  readonly onClose: () => void
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const days = useMemo(() => new Set(availableHours.map((candidate) => time.dayKey(candidate))), [availableHours, time])
  const months = useMemo(() => [...new Set([...days].map((day) => day.slice(0, 7)))].sort(), [days])
  const [month, setMonth] = useState(() => months.includes(dayKey.slice(0, 7)) ? dayKey.slice(0, 7) : months.at(-1) ?? dayKey.slice(0, 7))
  const monthIndex = months.indexOf(month)
  const root = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const dismiss = (event: PointerEvent) => {
      if (event.target instanceof Node && root.current?.contains(event.target)) return
      onClose()
    }
    document.addEventListener("pointerdown", dismiss, true)
    return () => document.removeEventListener("pointerdown", dismiss, true)
  }, [onClose])
  const cells: ReactNode[] = calendarMonthDays(month).map((candidate) => {
    const hasData = days.has(candidate)
    return <button
      aria-label={hasData ? calendarDateLabel(candidate, locale) : `${calendarDateLabel(candidate, locale)} · ${t("export.day.empty")}`}
      aria-pressed={candidate === dayKey}
      className="export-day h-[29px] cursor-pointer rounded-[var(--radius-xs)] border border-line2 bg-s2 p-0 font-mono text-sm tabular-nums text-fg2 hover:bg-s3 disabled:cursor-default disabled:border-transparent disabled:bg-transparent disabled:text-fg4 aria-pressed:border-accent aria-pressed:bg-accent-soft aria-pressed:text-fg coarse:h-11"
      data-day={candidate}
      data-has-data={hasData || undefined}
      data-testid="export-day"
      disabled={!hasData}
      key={candidate}
      onClick={() => onChoose(candidate)}
      type="button"
    >{String(Number(candidate.slice(-2)))}</button>
  })
  return <div aria-label={label} className="export-days absolute left-2.5 top-full z-[1100] mt-1 w-[min(304px,calc(100vw-20px))] rounded-[var(--radius-md)] border border-line3 bg-s1 p-2 shadow-[var(--shadow-pop)]" data-testid="export-day-grid" ref={root} role="dialog">
    <div className="mb-1 flex items-center justify-between">
      <button aria-label={t("hour.month.previous")} className="icon-button coarse:!h-11 coarse:!w-11" disabled={monthIndex <= 0} onClick={() => setMonth(months[monthIndex - 1] ?? month)} type="button"><ChevronLeft aria-hidden="true" size={14} /></button>
      <strong aria-live="polite" className="font-sans text-sm font-medium text-fg2" data-testid="export-day-month">{calendarMonthLabel(month, locale)}</strong>
      <button aria-label={t("hour.month.next")} className="icon-button coarse:!h-11 coarse:!w-11" disabled={monthIndex < 0 || monthIndex >= months.length - 1} onClick={() => setMonth(months[monthIndex + 1] ?? month)} type="button"><ChevronRight aria-hidden="true" size={14} /></button>
    </div>
    <div className="grid grid-cols-7 gap-0.5" role="group">{cells}</div>
    <p className="mb-0 mt-1.5 font-sans text-sm text-fg4">{t("export.days")}</p>
  </div>
}

function requestErrorKey(reason: unknown): string {
  if (!(reason instanceof ExportResponseError)) return "export.error.unavailable"
  if (reason.code !== null && SERVER_ERROR_KEYS.has(reason.code)) return `export.error.${reason.code}`
  return "export.error.unavailable"
}
