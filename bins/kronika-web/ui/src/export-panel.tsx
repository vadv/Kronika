import { CalendarDays, ChevronLeft, ChevronRight, Download, RotateCcw, X } from "lucide-react"
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import { createPortal } from "react-dom"

import { browserTimeZone, calendarDateLabel, calendarMonthLabel, type DisplayTimeZone } from "./display-time"
import { ExportResponseError, ExportServerStateUnknownError, fetchExportArtifact, triggerHtmlDownload } from "./export-download"
import {
  exportCalendarCells,
  exportDurationSeconds,
  exportRangeDefaults,
  formatExportDuration,
  formatExportEndpoint,
  resolveExportEndpoint,
  shiftExportMonth,
  type ExportEndpointError,
  type ExportEndpointResolution,
  type ExportEndpointValue,
} from "./export-time"
import type { Translate } from "./help"
import { humanAge, humanBytes, type Locale } from "./model"
import { apiFetch } from "./session"

type Endpoint = "from" | "to"
type Control = "from-date" | "from-time" | "from-occurrence" | "to-date" | "to-time" | "to-occurrence"

interface FoldPreference {
  readonly occurrence: number
  readonly second: number
}

interface EndpointState {
  readonly date: string
  readonly fold: FoldPreference | null
  readonly lastSecond: number
  readonly resolution: ExportEndpointResolution
  readonly time: string
}

interface FormIssue {
  readonly control: Control
  readonly key: string
}

type ExportJob =
  | { readonly phase: "idle" }
  | { readonly from: number; readonly phase: "preparing"; readonly startedAt: number; readonly to: number }
  | { readonly from: number; readonly phase: "downloading"; readonly received: number; readonly startedAt: number; readonly to: number; readonly total: number | null }
  | { readonly from: number; readonly phase: "unknown"; readonly startedAt: number; readonly to: number }

type DownloadingJob = Extract<ExportJob, { readonly phase: "downloading" }>

interface PanelPosition extends CSSProperties {
  readonly maxHeight: number
  readonly width: number
}

const SERVER_ERROR_KEYS = new Set(["bad_parameter", "export_busy", "export_empty", "export_failed"])
const EMPTY_PREFERENCE = { occurrence: null, preferred: null } as const
const WEEKDAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const
const COMPACT_EXPORT_MEDIA = "(max-width: 519px)"

export function ExportPanel({ anchor, hour, locale, mode, onActiveChange, onClose, t }: {
  readonly anchor: HTMLButtonElement | null
  readonly hour: number
  readonly locale: Locale
  readonly mode: DisplayTimeZone
  readonly onActiveChange: (active: boolean) => void
  readonly onClose: () => void
  readonly t: Translate
}) {
  const initial = useMemo(() => exportRangeDefaults(hour, mode), []) // eslint-disable-line react-hooks/exhaustive-deps
  const [from, setFrom] = useState(() => endpointFromValue(initial.from, mode))
  const [to, setTo] = useState(() => endpointFromValue(initial.to, mode))
  const [activeEndpoint, setActiveEndpoint] = useState<Endpoint>("from")
  const [month, setMonth] = useState(initial.from.date.slice(0, 7))
  const [calendarFocus, setCalendarFocus] = useState(initial.from.date)
  const [calendarOpen, setCalendarOpen] = useState(false)
  const [compact, setCompact] = useState(() => window.matchMedia(COMPACT_EXPORT_MEDIA).matches)
  const [issues, setIssues] = useState<readonly FormIssue[]>([])
  const [requestError, setRequestError] = useState<string | null>(null)
  const [job, setJob] = useState<ExportJob>({ phase: "idle" })
  const [elapsedNow, setElapsedNow] = useState(() => performance.now())
  const [position, setPosition] = useState<PanelPosition | null>(null)
  const panel = useRef<HTMLElement>(null)
  const fromDate = useRef<HTMLInputElement>(null)
  const fromTime = useRef<HTMLInputElement>(null)
  const fromOccurrence = useRef<HTMLFieldSetElement>(null)
  const toDate = useRef<HTMLInputElement>(null)
  const toTime = useRef<HTMLInputElement>(null)
  const toOccurrence = useRef<HTMLFieldSetElement>(null)
  const calendarDays = useRef(new Map<string, HTMLButtonElement>())
  const calendarOpenFocus = useRef(false)
  const transferFrame = useRef<number | null>(null)
  const pendingTransfer = useRef<DownloadingJob | null>(null)
  const activeJob = useRef(false)
  const openedFocus = useRef(false)
  const previousMode = useRef(mode)
  const previousHour = useRef(hour)
  const pristineRange = useRef({ from: initial.from.second, to: initial.to.second })
  const titleId = useId()
  const zoneId = useId()
  const statusId = useId()
  const calendarId = useId()
  const localZone = useMemo(browserTimeZone, [])
  const busy = job.phase !== "idle"
  const closeLabel = job.phase === "unknown" ? t("export.close_unknown") : busy ? t("export.close_busy") : t("export.close")
  const cells = useMemo(() => exportCalendarCells(month), [month])
  const duration = exportDurationSeconds(from.resolution.second, to.resolution.second)
  const pristine = from.resolution.second === pristineRange.current.from && to.resolution.second === pristineRange.current.to

  const close = useCallback(() => {
    if (activeJob.current) return
    onClose()
    requestAnimationFrame(() => {
      if (anchor?.isConnected) anchor.focus({ preventScroll: true })
    })
  }, [anchor, onClose])

  const returnToEditor = useCallback((endpoint: Endpoint) => {
    setCalendarOpen(false)
    requestAnimationFrame(() => {
      const target = endpoint === "from" ? fromDate.current : toDate.current
      target?.focus({ preventScroll: true })
    })
  }, [])

  const clearPendingTransfer = useCallback(() => {
    if (transferFrame.current !== null) cancelAnimationFrame(transferFrame.current)
    transferFrame.current = null
    pendingTransfer.current = null
  }, [])

  const publishTransfer = useCallback((next: DownloadingJob) => {
    pendingTransfer.current = next
    if (transferFrame.current !== null) return
    transferFrame.current = requestAnimationFrame(() => {
      transferFrame.current = null
      const pending = pendingTransfer.current
      pendingTransfer.current = null
      if (pending !== null) setJob(pending)
    })
  }, [])

  useEffect(() => clearPendingTransfer, [clearPendingTransfer])

  useLayoutEffect(() => {
    const place = () => {
      if (anchor === null) return
      setPosition(exportPanelPlacement(anchor.getBoundingClientRect(), {
        compact: window.matchMedia(COMPACT_EXPORT_MEDIA).matches,
        height: window.innerHeight,
        width: document.documentElement.clientWidth,
      }))
    }
    place()
    window.addEventListener("resize", place)
    window.addEventListener("scroll", place, true)
    return () => {
      window.removeEventListener("resize", place)
      window.removeEventListener("scroll", place, true)
    }
  }, [anchor])

  useLayoutEffect(() => {
    const media = window.matchMedia(COMPACT_EXPORT_MEDIA)
    const update = () => setCompact(media.matches)
    update()
    media.addEventListener("change", update)
    return () => media.removeEventListener("change", update)
  }, [])

  useEffect(() => {
    if (!compact) setCalendarOpen(false)
  }, [compact])

  useLayoutEffect(() => {
    if (position === null || openedFocus.current) return
    openedFocus.current = true
    fromDate.current?.focus({ preventScroll: true })
  }, [position])

  useLayoutEffect(() => {
    if (!compact || !calendarOpen || !calendarOpenFocus.current) return
    calendarOpenFocus.current = false
    const selectedDate = activeEndpoint === "from" ? from.date : to.date
    const target = calendarDays.current.get(selectedDate)
      ?? calendarDays.current.get(calendarFocus)
      ?? [...calendarDays.current.values()][0]
    target?.focus({ preventScroll: true })
  }, [activeEndpoint, calendarFocus, calendarOpen, compact, from.date, to.date])

  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      if (activeJob.current) {
        if (panel.current?.contains(document.activeElement)) event.stopPropagation()
        return
      }
      if (compact && calendarOpen) {
        event.preventDefault()
        event.stopPropagation()
        returnToEditor(activeEndpoint)
        return
      }
      event.preventDefault()
      event.stopPropagation()
      close()
    }
    window.addEventListener("keydown", escape, true)
    return () => window.removeEventListener("keydown", escape, true)
  }, [activeEndpoint, calendarOpen, close, compact, returnToEditor])

  useEffect(() => {
    if (job.phase !== "preparing") return
    setElapsedNow(performance.now())
    const timer = window.setInterval(() => setElapsedNow(performance.now()), 250)
    return () => window.clearInterval(timer)
  }, [job.phase])

  useLayoutEffect(() => {
    if (previousMode.current === mode) return
    previousMode.current = mode
    const nextFrom = endpointFromValue(formatExportEndpoint(from.resolution.second ?? from.lastSecond, mode), mode)
    const nextTo = endpointFromValue(formatExportEndpoint(to.resolution.second ?? to.lastSecond, mode), mode)
    setFrom(nextFrom)
    setTo(nextTo)
    setMonth((activeEndpoint === "from" ? nextFrom : nextTo).date.slice(0, 7))
    setCalendarFocus((activeEndpoint === "from" ? nextFrom : nextTo).date)
    setIssues([])
    setRequestError(null)
  }, [mode])

  useEffect(() => {
    if (previousHour.current === hour) return
    previousHour.current = hour
    if (activeJob.current || !pristine) return
    const next = exportRangeDefaults(hour, mode)
    pristineRange.current = { from: next.from.second, to: next.to.second }
    setFrom(endpointFromValue(next.from, mode))
    setTo(endpointFromValue(next.to, mode))
    setMonth(next.from.date.slice(0, 7))
    setCalendarFocus(next.from.date)
  }, [hour, mode, pristine])

  const clearFeedback = () => {
    setIssues([])
    setRequestError(null)
  }
  const setEndpoint = (endpoint: Endpoint, update: (current: EndpointState) => EndpointState) => {
    clearFeedback()
    if (endpoint === "from") setFrom(update)
    else setTo(update)
  }
  const chooseEndpoint = (endpoint: Endpoint) => {
    setActiveEndpoint(endpoint)
    const date = endpoint === "from" ? from.date : to.date
    if (/^\d{4}-\d{2}-\d{2}$/.test(date)) {
      setMonth(date.slice(0, 7))
      setCalendarFocus(date)
    }
  }
  const openCalendar = (endpoint: Endpoint) => {
    chooseEndpoint(endpoint)
    if (!compact) return
    calendarOpenFocus.current = true
    setCalendarOpen(true)
  }
  const edit = (endpoint: Endpoint, part: "date" | "time", value: string) => {
    chooseEndpoint(endpoint)
    if (part === "date" && exportCalendarCells(value.slice(0, 7)).includes(value)) {
      setMonth(value.slice(0, 7))
      setCalendarFocus(value)
    }
    setEndpoint(endpoint, (current) => editEndpoint(current, part, value, mode))
  }
  const chooseOccurrence = (endpoint: Endpoint, occurrence: number) => {
    setEndpoint(endpoint, (current) => chooseEndpointOccurrence(current, occurrence, mode))
  }
  const chooseCalendarDay = (date: string) => {
    const endpoint = activeEndpoint
    setEndpoint(activeEndpoint, (current) => editEndpoint(current, "date", date, mode))
    setMonth(date.slice(0, 7))
    setCalendarFocus(date)
    if (endpoint === "from") setActiveEndpoint("to")
    else if (compact) returnToEditor("to")
  }
  const reset = () => {
    if (activeJob.current) return
    const defaults = exportRangeDefaults(hour, mode)
    pristineRange.current = { from: defaults.from.second, to: defaults.to.second }
    setFrom(endpointFromValue(defaults.from, mode))
    setTo(endpointFromValue(defaults.to, mode))
    setActiveEndpoint("from")
    setMonth(defaults.from.date.slice(0, 7))
    setCalendarFocus(defaults.from.date)
    setCalendarOpen(false)
    clearFeedback()
    requestAnimationFrame(() => fromDate.current?.focus({ preventScroll: true }))
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (activeJob.current) return
    const nextIssues = endpointIssues("from", from.resolution).concat(endpointIssues("to", to.resolution))
    const fromSecond = from.resolution.second
    const toSecond = to.resolution.second
    if (nextIssues.length === 0 && fromSecond !== null && toSecond !== null && fromSecond > toSecond) {
      const control = to.resolution.candidates.length > 1
        ? "to-occurrence" : from.date === to.date ? "to-time" : "to-date"
      nextIssues.push({ control, key: "export.error.order" })
    }
    if (nextIssues.length !== 0 || fromSecond === null || toSecond === null) {
      setIssues(nextIssues)
      setRequestError(null)
      const focusIssue = () => focusControl(nextIssues[0]?.control ?? "from-date", {
          "from-date": fromDate.current,
          "from-occurrence": fromOccurrence.current?.querySelector("input") ?? fromOccurrence.current,
          "from-time": fromTime.current,
          "to-date": toDate.current,
          "to-occurrence": toOccurrence.current?.querySelector("input") ?? toOccurrence.current,
          "to-time": toTime.current,
        })
      if (compact && calendarOpen) {
        setCalendarOpen(false)
        requestAnimationFrame(focusIssue)
      } else focusIssue()
      return
    }

    const startedAt = performance.now()
    setCalendarOpen(false)
    activeJob.current = true
    onActiveChange(true)
    setIssues([])
    setRequestError(null)
    setElapsedNow(startedAt)
    setJob({ from: fromSecond, phase: "preparing", startedAt, to: toSecond })
    try {
      const artifact = await fetchExportArtifact(apiFetch, fromSecond, toSecond, ({ received, total }) => {
        publishTransfer({ from: fromSecond, phase: "downloading", received, startedAt, to: toSecond, total })
      })
      clearPendingTransfer()
      triggerHtmlDownload(artifact.blob, artifact.filename)
      activeJob.current = false
      onActiveChange(false)
      setJob({ phase: "idle" })
      close()
    } catch (reason) {
      if (reason instanceof ExportServerStateUnknownError) {
        setJob({ from: fromSecond, phase: "unknown", startedAt, to: toSecond })
        return
      }
      clearPendingTransfer()
      activeJob.current = false
      onActiveChange(false)
      setJob({ phase: "idle" })
      setRequestError(requestErrorKey(reason))
    }
  }

  const messages = issues.map(({ key }) => t(key))
  if (requestError !== null) messages.push(t(requestError))
  const rangeFrom = job.phase === "idle" ? from.resolution.second : job.from
  const rangeTo = job.phase === "idle" ? to.resolution.second : job.to
  const status = job.phase === "preparing"
    ? t("export.preparing")
    : job.phase === "downloading"
      ? job.total === null
        ? t("export.downloading", { received: humanBytes(job.received, locale) })
        : t("export.downloading_total", { received: humanBytes(job.received, locale), total: humanBytes(job.total, locale) })
      : job.phase === "unknown" ? t("export.error.server_state_unknown")
      : messages.join(" · ")

  const calendar = <section
    aria-label={t("export.calendar")}
    className={compact ? "min-w-0" : "min-w-0 border-l border-line2 pl-2.5 max-[640px]:border-l-0 max-[640px]:border-t max-[640px]:pl-0 max-[640px]:pt-2.5"}
    data-testid="export-calendar"
    hidden={compact && !calendarOpen}
    id={calendarId}
  >
    {compact && <div className="mb-1.5 flex h-11 items-center gap-1 border-b border-line2 pb-1">
      <button className="inline-flex h-11 cursor-pointer items-center gap-1 rounded-[var(--radius-xs)] border-0 bg-transparent px-1.5 font-sans text-sm font-medium text-fg3 hover:bg-s3 hover:text-fg" data-testid="export-calendar-done" onClick={() => returnToEditor(activeEndpoint)} type="button"><ChevronLeft aria-hidden="true" size={14} />{t("export.calendar.done")}</button>
      <div aria-label={t("export.calendar")} className="ml-auto flex items-center gap-1" role="group">
        {(["from", "to"] as const).map((endpoint) => <button
          aria-label={t(`export.${endpoint}.calendar`)}
          aria-pressed={activeEndpoint === endpoint}
          className="inline-flex h-11 min-w-11 cursor-pointer items-center justify-center rounded-[var(--radius-xs)] border border-line2 bg-s2 px-2 font-sans text-sm font-semibold text-fg3 hover:bg-s3 hover:text-fg aria-pressed:border-accent2 aria-pressed:bg-s4 aria-pressed:text-accent3"
          data-testid={`export-calendar-${endpoint}-target`}
          disabled={busy}
          key={endpoint}
          onClick={() => chooseEndpoint(endpoint)}
          type="button"
        >{t(`export.${endpoint}`)}</button>)}
      </div>
    </div>}
    <div className="flex h-8 items-center justify-between coarse:h-11">
      <button aria-label={t("export.month.previous")} className="icon-button coarse:!h-11 coarse:!w-11" data-testid="export-calendar-prev" disabled={shiftExportMonth(month, -1) === null || busy} onClick={() => setMonth(shiftExportMonth(month, -1) ?? month)} type="button"><ChevronLeft aria-hidden="true" size={14} /></button>
      <strong className="font-sans text-sm font-medium text-fg2" data-testid="export-calendar-month">{calendarMonthLabel(month, locale)}</strong>
      <button aria-label={t("export.month.next")} className="icon-button coarse:!h-11 coarse:!w-11" data-testid="export-calendar-next" disabled={shiftExportMonth(month, 1) === null || busy} onClick={() => setMonth(shiftExportMonth(month, 1) ?? month)} type="button"><ChevronRight aria-hidden="true" size={14} /></button>
    </div>
    <div className="grid grid-cols-7 gap-0.5">
      {WEEKDAYS.map((weekday) => <abbr className="flex h-5 items-center justify-center border-0 font-sans text-sm font-medium text-fg4 no-underline" key={weekday} title={t(`export.weekday.${weekday}.full`)}>{t(`export.weekday.${weekday}`)}</abbr>)}
    </div>
    <div className="grid grid-cols-7 gap-0.5">
      {cells.map((date, index) => date === null
        ? <span aria-hidden="true" className="h-7 coarse:h-11" key={`empty-${index}`} />
        : <button
          aria-label={calendarDateLabel(date, locale)}
          aria-pressed={date === (activeEndpoint === "from" ? from.date : to.date)}
          className="export-calendar-day h-7 cursor-pointer rounded-[var(--radius-xs)] border border-line2 bg-s2 p-0 font-mono text-sm tabular-nums text-fg3 transition-colors hover:border-line4 hover:bg-s3 hover:text-fg aria-pressed:border-accent2 aria-pressed:text-accent3 coarse:h-11 coarse:min-w-11"
          data-day={date}
          data-range={calendarRange(date, from.date, to.date)}
          data-testid="export-calendar-day"
          disabled={busy}
          key={date}
          onClick={() => chooseCalendarDay(date)}
          onFocus={() => setCalendarFocus(date)}
          onKeyDown={(event) => moveCalendarFocus(event, cells, index, calendarDays.current)}
          ref={(node) => { if (node === null) calendarDays.current.delete(date); else calendarDays.current.set(date, node) }}
          tabIndex={calendarTabStop(date, cells, calendarFocus) ? 0 : -1}
          type="button"
        >{String(Number(date.slice(-2)))}</button>)}
    </div>
  </section>

  const content = <section
    aria-describedby={zoneId}
    aria-labelledby={titleId}
    aria-modal="false"
    className="export-panel fixed z-[1100] flex flex-col overflow-auto rounded-[var(--radius-md)] border border-line3 bg-s1 text-fg shadow-[var(--shadow-pop)]"
    data-phase={job.phase}
    data-range-from={rangeFrom ?? ""}
    data-range-to={rangeTo ?? ""}
    data-view={compact && calendarOpen ? "calendar" : "editor"}
    data-testid="export-panel"
    onKeyDown={(event) => event.stopPropagation()}
    ref={panel}
    role="dialog"
    style={position ?? { visibility: "hidden" }}
  >
    <header className="flex min-h-10 flex-none items-center gap-2 border-b border-line3 px-2.5">
      <span className="rounded-[var(--radius-xs)] border border-line3 bg-s2 px-1.5 py-0.5 font-mono text-sm text-fg3">HTML</span>
      <h2 className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-md" id={titleId}>{t("export.title")}</h2>
      <button aria-label={closeLabel} className="icon-button coarse:!h-11 coarse:!w-11" data-testid="export-close" disabled={busy} onClick={close} title={closeLabel} type="button"><X aria-hidden="true" size={14} /></button>
    </header>
    <div className="flex min-h-8 flex-none flex-wrap items-center gap-x-2 gap-y-0.5 border-b border-line2 bg-s2 px-2.5 py-1 font-sans text-sm text-fg3" id={zoneId}>
      <span>{mode === "browser" ? t("timezone.browser") : t("timezone.utc")}</span>
      {mode === "browser" && <><span aria-hidden="true">·</span><code className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-accent3" data-testid="export-zone" translate="no">{localZone}</code></>}
      <span className="ml-auto text-fg4">{t("export.inclusive")}</span>
    </div>
    <form aria-busy={busy} className="flex min-h-0 flex-col" data-testid="export-form" noValidate onSubmit={(event) => { void submit(event) }}>
      <div className="export-panel-body grid min-h-0 grid-cols-[minmax(260px,1fr)_252px] gap-2.5 p-2.5 max-[640px]:grid-cols-1">
        <div className="min-w-0">
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy} hidden={compact && calendarOpen}>
          <legend className="mb-1.5 font-sans text-sm font-semibold text-fg2">{t("export.range")}</legend>
          <div className="grid grid-cols-[44px_minmax(120px,1fr)_96px] gap-x-1.5 px-px font-sans text-sm text-fg4" aria-hidden="true">
            <span />
            <span>{t("export.date")}</span>
            <span>{t("export.time")}</span>
          </div>
          <EndpointEditor active={activeEndpoint === "from"} calendarId={calendarId} describedBy={zoneId} endpoint="from" expanded={compact ? calendarOpen && activeEndpoint === "from" : undefined} issues={issues} onChoose={() => chooseEndpoint("from")} onDate={(value) => edit("from", "date", value)} onOccurrence={(choice) => chooseOccurrence("from", choice)} onOpenCalendar={() => openCalendar("from")} onTime={(value) => edit("from", "time", value)} refs={{ date: fromDate, occurrence: fromOccurrence, time: fromTime }} state={from} t={t} />
          <EndpointEditor active={activeEndpoint === "to"} calendarId={calendarId} describedBy={zoneId} endpoint="to" expanded={compact ? calendarOpen && activeEndpoint === "to" : undefined} issues={issues} onChoose={() => chooseEndpoint("to")} onDate={(value) => edit("to", "date", value)} onOccurrence={(choice) => chooseOccurrence("to", choice)} onOpenCalendar={() => openCalendar("to")} onTime={(value) => edit("to", "time", value)} refs={{ date: toDate, occurrence: toOccurrence, time: toTime }} state={to} t={t} />
        </fieldset>
        {compact && calendar}
          <div className="mt-2 grid min-h-9 grid-cols-[minmax(0,1fr)_auto] items-center gap-2 border-y border-line2 bg-s2 px-2">
            <span className="min-w-0 font-sans text-sm text-fg3">
              {t("export.duration")}: <strong className="font-mono font-medium tabular-nums text-fg" data-testid="export-duration">{duration === null ? "—" : formatExportDuration(duration, locale)}</strong>
            </span>
            <button aria-label={t("export.selected_hour")} className="inline-flex h-7 cursor-pointer items-center gap-1 whitespace-nowrap rounded-[var(--radius-xs)] border-0 bg-transparent px-1.5 font-sans text-sm font-medium text-fg3 hover:bg-s3 hover:text-fg coarse:h-11" data-testid="export-selected-hour" onClick={reset} title={t("export.selected_hour")} type="button"><RotateCcw aria-hidden="true" size={12} /><span className="max-[519px]:hidden">{t("export.selected_hour")}</span><span aria-hidden="true" className="hidden max-[519px]:inline">{t("export.selected_hour.short")}</span></button>
          </div>
        </div>
        {!compact && calendar}
      </div>
      {(!compact || !calendarOpen) && <div className="export-status mx-2.5 flex h-11 flex-none items-center overflow-auto border-l-2 px-2 font-sans text-sm leading-[1.35]" data-error={messages.length > 0 || job.phase === "unknown" || undefined} data-testid="export-status" id={statusId}>
        {job.phase === "preparing" && <span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none !mr-2 h-3 w-3 flex-none" />}
        {job.phase === "downloading" && job.total !== null && <progress aria-label={t("export.download_progress")} className="mr-2 h-1 w-12 flex-none" max={Math.max(job.total, job.received, 1)} value={job.received} />}
        <span aria-hidden="true">{status}{job.phase === "preparing" && <span> · {humanAge(Math.max(0, (elapsedNow - job.startedAt) / 1_000), locale)}</span>}</span>
      </div>}
      <span aria-atomic="true" aria-live="polite" className="sr-only">{job.phase === "preparing" ? t("export.preparing") : job.phase === "downloading" ? t("export.downloading_phase") : status}</span>
      {(!compact || !calendarOpen) && <footer className="mt-1 flex min-h-10 flex-none items-center justify-between gap-2 border-t border-line2 px-2.5 py-1">
        <span className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-sans text-sm text-fg4">{t("export.format")}</span>
        <button className="inline-flex h-8 cursor-pointer items-center justify-center gap-1.5 rounded-[var(--radius-sm)] border-0 bg-accent px-3 font-sans text-sm font-semibold text-bg transition-colors hover:bg-accent2 disabled:cursor-wait disabled:opacity-55 coarse:h-11" data-testid="export-submit" disabled={busy} type="submit"><Download aria-hidden="true" size={14} />{t("export.submit")}</button>
      </footer>}
    </form>
  </section>
  return createPortal(content, document.body)
}

function EndpointEditor({ active, calendarId, describedBy, endpoint, expanded, issues, onChoose, onDate, onOccurrence, onOpenCalendar, onTime, refs, state, t }: {
  readonly active: boolean
  readonly calendarId: string
  readonly describedBy: string
  readonly endpoint: Endpoint
  readonly expanded?: boolean | undefined
  readonly issues: readonly FormIssue[]
  readonly onChoose: () => void
  readonly onDate: (value: string) => void
  readonly onOccurrence: (choice: number) => void
  readonly onOpenCalendar: () => void
  readonly onTime: (value: string) => void
  readonly refs: {
    readonly date: React.RefObject<HTMLInputElement | null>
    readonly occurrence: React.RefObject<HTMLFieldSetElement | null>
    readonly time: React.RefObject<HTMLInputElement | null>
  }
  readonly state: EndpointState
  readonly t: Translate
}) {
  const dateErrorId = useId()
  const timeErrorId = useId()
  const occurrenceErrorId = useId()
  const dateInvalid = hasIssue(issues, `${endpoint}-date`)
  const timeInvalid = hasIssue(issues, `${endpoint}-time`)
  const occurrenceInvalid = hasIssue(issues, `${endpoint}-occurrence`)
  const dateError = issueMessage(issues, `${endpoint}-date`, t)
  const timeError = issueMessage(issues, `${endpoint}-time`, t)
  const occurrenceError = issueMessage(issues, `${endpoint}-occurrence`, t)
  return <div className="mt-1.5">
    <div className="grid grid-cols-[44px_minmax(120px,1fr)_96px] gap-x-1.5">
      <button aria-controls={calendarId} aria-expanded={expanded} aria-label={t(`export.${endpoint}.calendar`)} aria-pressed={active} className="inline-flex h-8 cursor-pointer items-center justify-center gap-1 rounded-[var(--radius-xs)] border border-line2 bg-s2 font-sans text-sm font-semibold text-fg3 hover:bg-s3 hover:text-fg aria-pressed:border-accent2 aria-pressed:bg-s4 aria-pressed:text-accent3 coarse:h-11" data-testid={`export-${endpoint}-target`} onClick={onOpenCalendar} type="button"><CalendarDays aria-hidden="true" size={12} /><span>{t(`export.${endpoint}`)}</span></button>
      <label className="min-w-0">
        <span className="sr-only">{t(`export.${endpoint}_date`)}</span>
        <input aria-describedby={describedBy} aria-errormessage={dateInvalid ? dateErrorId : undefined} aria-invalid={dateInvalid || undefined} autoComplete="off" className="h-8 w-full rounded-[var(--radius-xs)] border border-line3 bg-bg px-2 font-mono text-sm tabular-nums text-fg outline-none transition-colors placeholder:text-fg-null focus:border-accent focus:shadow-[0_0_0_1px_var(--color-accent-line)] disabled:cursor-wait disabled:opacity-65 coarse:h-11" data-testid={`export-${endpoint}-date`} maxLength={10} onChange={(event) => onDate(event.target.value)} onFocus={onChoose} placeholder="YYYY-MM-DD" ref={refs.date} spellCheck={false} type="text" value={state.date} />
      </label>
      <label className="min-w-0">
        <span className="sr-only">{t(`export.${endpoint}_time`)}</span>
        <input aria-describedby={describedBy} aria-errormessage={timeInvalid ? timeErrorId : undefined} aria-invalid={timeInvalid || undefined} autoComplete="off" className="h-8 w-full rounded-[var(--radius-xs)] border border-line3 bg-bg px-2 font-mono text-sm tabular-nums text-fg outline-none transition-colors placeholder:text-fg-null focus:border-accent focus:shadow-[0_0_0_1px_var(--color-accent-line)] disabled:cursor-wait disabled:opacity-65 coarse:h-11" data-testid={`export-${endpoint}-time`} maxLength={8} onChange={(event) => onTime(event.target.value)} onFocus={onChoose} placeholder="HH:mm:ss" ref={refs.time} spellCheck={false} type="text" value={state.time} />
      </label>
    </div>
    {dateError !== null && <span className="sr-only" id={dateErrorId}>{dateError}</span>}
    {timeError !== null && <span className="sr-only" id={timeErrorId}>{timeError}</span>}
    {state.resolution.candidates.length > 1 && <fieldset aria-describedby={describedBy} aria-errormessage={occurrenceInvalid ? occurrenceErrorId : undefined} aria-invalid={occurrenceInvalid || undefined} className="ml-[50px] mt-1 flex min-h-7 items-center gap-1.5 border-0 p-0" data-testid={`export-${endpoint}-occurrence`} ref={refs.occurrence}>
      <legend className="sr-only">{t(`export.${endpoint}_occurrence`)}</legend>
      <span aria-hidden="true" className="font-sans text-sm text-fg4">{t("export.occurrence")}</span>
      {state.resolution.candidates.map((_, occurrence) => <label className="inline-flex h-7 cursor-pointer items-center gap-1 rounded-[var(--radius-xs)] border border-line2 bg-s2 px-1.5 font-sans text-sm text-fg3 has-[:checked]:border-accent2 has-[:checked]:bg-s4 has-[:checked]:text-accent3 coarse:h-11" key={occurrence}>
        <input checked={state.resolution.occurrence === occurrence} className="accent-accent" name={`export-${endpoint}-occurrence`} onChange={() => onOccurrence(occurrence)} type="radio" />
        {t(occurrence === 0 ? "export.occurrence.first" : "export.occurrence.second")}
      </label>)}
    </fieldset>}
    {occurrenceError !== null && <span className="sr-only" id={occurrenceErrorId}>{occurrenceError}</span>}
  </div>
}

export function exportPanelPlacement(
  anchor: { readonly bottom: number; readonly right: number },
  viewport: { readonly compact: boolean; readonly height: number; readonly width: number },
): PanelPosition {
  const edge = 8
  const width = Math.max(0, Math.min(viewport.compact ? viewport.width - 2 * edge : 640, viewport.width - 2 * edge))
  if (viewport.compact) return { bottom: edge, left: edge, maxHeight: Math.max(0, viewport.height - 2 * edge), width }
  const left = Math.max(edge, Math.min(anchor.right - width, viewport.width - width - edge))
  const top = Math.max(edge, anchor.bottom + 6)
  return { left, maxHeight: Math.max(0, viewport.height - top - edge), top, width }
}

function endpointFromValue(value: ExportEndpointValue, mode: DisplayTimeZone): EndpointState {
  const resolution = resolveExportEndpoint(value.date, value.time, mode, { occurrence: null, preferred: value.second })
  return {
    date: value.date,
    fold: resolution.candidates.length > 1 && resolution.occurrence !== null
      ? { occurrence: resolution.occurrence, second: value.second } : null,
    lastSecond: value.second,
    resolution,
    time: value.time,
  }
}

function editEndpoint(current: EndpointState, part: "date" | "time", value: string, mode: DisplayTimeZone): EndpointState {
  const date = part === "date" ? value : current.date
  const time = part === "time" ? value : current.time
  const preference = current.fold === null
    ? EMPTY_PREFERENCE
    : { occurrence: current.fold.occurrence, preferred: current.fold.second }
  const resolution = resolveExportEndpoint(date, time, mode, preference)
  return endpointWithResolution(current, date, time, resolution)
}

function chooseEndpointOccurrence(current: EndpointState, occurrence: number, mode: DisplayTimeZone): EndpointState {
  const resolution = resolveExportEndpoint(current.date, current.time, mode, { occurrence, preferred: null })
  return endpointWithResolution(current, current.date, current.time, resolution)
}

function endpointWithResolution(
  current: EndpointState,
  date: string,
  time: string,
  resolution: ExportEndpointResolution,
): EndpointState {
  const second = resolution.second
  const fold = resolution.candidates.length > 1 && resolution.occurrence !== null && second !== null
    ? { occurrence: resolution.occurrence, second } : resolution.error === null ? null : current.fold
  return {
    date,
    fold,
    lastSecond: second ?? current.lastSecond,
    resolution,
    time,
  }
}

function endpointIssues(endpoint: Endpoint, resolution: ExportEndpointResolution): FormIssue[] {
  if (resolution.error === null) return []
  const control = endpointControl(endpoint, resolution.error)
  return [{ control, key: `export.error.${endpoint}.${resolution.error}` }]
}

function endpointControl(endpoint: Endpoint, error: ExportEndpointError): Control {
  if (error.startsWith("date_")) return `${endpoint}-date`
  if (error === "occurrence_required") return `${endpoint}-occurrence`
  return `${endpoint}-time`
}

function hasIssue(issues: readonly FormIssue[], control: string): boolean {
  return issues.some((issue) => issue.control === control)
}

function issueMessage(issues: readonly FormIssue[], control: Control, t: Translate): string | null {
  const issue = issues.find((candidate) => candidate.control === control)
  return issue === undefined ? null : t(issue.key)
}

function focusControl(control: Control, controls: Readonly<Record<Control, HTMLElement | null>>): void {
  controls[control]?.focus({ preventScroll: true })
}

function calendarRange(date: string, from: string, to: string): string | undefined {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(from) || !/^\d{4}-\d{2}-\d{2}$/.test(to) || from > to) return undefined
  if (date === from && date === to) return "single"
  if (date === from) return "from"
  if (date === to) return "to"
  return date > from && date < to ? "inside" : undefined
}

function calendarTabStop(date: string, cells: readonly (string | null)[], activeDate: string): boolean {
  if (date === activeDate) return true
  return !cells.includes(activeDate) && date === cells.find((candidate) => candidate !== null)
}

function moveCalendarFocus(
  event: ReactKeyboardEvent<HTMLButtonElement>,
  cells: readonly (string | null)[],
  index: number,
  nodes: ReadonlyMap<string, HTMLButtonElement>,
): void {
  const step = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : event.key === "ArrowUp" ? -7 : event.key === "ArrowDown" ? 7 : 0
  let target = step === 0 ? index : index + step
  let scan = target < index ? -1 : 1
  if (event.key === "Home") {
    target = index - index % 7
    scan = 1
  } else if (event.key === "End") {
    target = index + 6 - index % 7
    scan = -1
  } else if (step === 0) return
  while (target >= 0 && target < cells.length && cells[target] === null) target += scan
  const date = cells[target]
  if (date === undefined || date === null) return
  event.preventDefault()
  nodes.get(date)?.focus({ preventScroll: true })
}

function requestErrorKey(reason: unknown): string {
  if (!(reason instanceof ExportResponseError)) return "export.error.unavailable"
  if (reason.code !== null && SERVER_ERROR_KEYS.has(reason.code)) return `export.error.${reason.code}`
  return "export.error.unavailable"
}
