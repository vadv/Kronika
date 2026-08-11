import { Activity, HelpCircle, Languages, Moon, Sun } from "lucide-react"
import { dictionaries } from "kronika:i18n"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { createRoot } from "react-dom/client"

import {
  VIEW_SECTIONS,
  discoverHourSelection,
  fieldNameForLocator,
  loadHour,
  mergeHourData,
  resolveLocator,
  type DataRow,
  type Finding,
  type HourData,
} from "./api"
import { DetailDock } from "./detail"
import { EventsView } from "./events-view"
import { HelpPanel, type Translate } from "./help"
import { HourPicker } from "./hour-picker"
import {
  activityFor,
  asNumber,
  floorHour,
  formatUtc,
  interpolate,
  processKey,
  processLens,
  snapshot,
  value,
  type Lens,
  type Locale,
} from "./model"
import { PostgresView, type PostgresSection } from "./postgres-view"
import { ProcessSummary, ProcessTable } from "./process-table"
import { SystemView } from "./system-view"
import { Timeline } from "./timeline"

type Source = "host" | "postgresql" | "events"
type Theme = "dark" | "light"
type HostSection = "system" | "processes"

const EMPTY_DATA: HourData = {
  sections: {}, availableSections: [], processes: [], activities: [], load: [], memory: [], pressure: [], health: [],
  pgOverview: [], pgStatements: [], pgLocks: [], pgDatabases: [], pgEvents: [], points: [], findings: [],
  sourceFamilies: [], segmentCount: 0,
}

const HELP_SYSTEM = [
  { label: "system.metric.health.label", help: "system.metric.health.help" },
  { label: "system.metric.cpu_busy.label", help: "system.metric.cpu_busy.help" },
  { label: "system.metric.load1.label", help: "system.metric.load1.help" },
  { label: "system.metric.mem_available_percent.label", help: "system.metric.mem_available_percent.help" },
  { label: "system.metric.cpu_pressure.label", help: "system.metric.cpu_pressure.help" },
  { label: "system.metric.memory_pressure.label", help: "system.metric.memory_pressure.help" },
  { label: "system.metric.io_pressure.label", help: "system.metric.io_pressure.help" },
  { label: "system.metric.filesystem_free_min.label", help: "system.metric.filesystem_free_min.help" },
] as const

const HELP_PROCESS = [
  { label: "col.pid.label", help: "col.pid.help" },
  { label: "col.starttime.label", help: "col.starttime.help" },
  { label: "col.command.label", help: "col.command.help" },
  { label: "detail.pg.title", help: "detail.pg.help" },
  { label: "pg.query.label", help: "pg.query.help" },
] as const

const HELP_POSTGRESQL = [
  { label: "pg.pid.label", help: "pg.pid.help" },
  { label: "pg.backend_type.label", help: "pg.backend_type.help" },
  { label: "pg.state.label", help: "pg.state.help" },
  { label: "pg.wait_event.label", help: "pg.wait_event.help" },
  { label: "pg.query.label", help: "pg.query.help" },
] as const

const HELP_EVENTS = [
  { label: "locator.event", help: "locator.event.help" },
  { label: "locator.known_bad", help: "locator.known_bad.help" },
  { label: "locator.spike", help: "locator.spike.help" },
] as const

function App() {
  const [locale, setLocale] = useState<Locale>(initialLocale)
  const [theme, setTheme] = useState<Theme>(initialTheme)
  const [hour, setHour] = useState<number | null>(null)
  const [availableHours, setAvailableHours] = useState<readonly number[]>([])
  const [cursor, setCursor] = useState(0)
  const [data, setData] = useState<HourData>(EMPTY_DATA)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [source, setSource] = useState<Source>("host")
  const [hostSection, setHostSection] = useState<HostSection>("system")
  const [pgSection, setPgSection] = useState<PostgresSection>("overview")
  const [lens, setLens] = useState<Lens>("generic")
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [dockClosed, setDockClosed] = useState(false)
  const [selectedFinding, setSelectedFinding] = useState<Finding | null>(null)
  const [eventScope, setEventScope] = useState<readonly Finding[] | null>(null)
  const [pgFocus, setPgFocus] = useState<DataRow | null>(null)
  const [systemFocus, setSystemFocus] = useState<Finding | null>(null)
  const [helpOpen, setHelpOpen] = useState(false)
  const t = useMemo<Translate>(() => (key, slots = {}) => {
    const template = dictionaries[locale][key as keyof typeof dictionaries.en] ?? dictionaries.en[key as keyof typeof dictionaries.en] ?? key
    return interpolate(template, slots)
  }, [locale])

  useEffect(() => {
    document.documentElement.lang = locale
    try { localStorage.setItem("kronika.locale", locale) } catch { /* storage can be disabled */ }
  }, [locale])
  useEffect(() => {
    document.documentElement.dataset.theme = theme
    try { localStorage.setItem("kronika.theme", theme) } catch { /* storage can be disabled */ }
  }, [theme])
  useEffect(() => {
    const controller = new AbortController()
    void discoverHourSelection(controller.signal).then((selection) => {
      setAvailableHours(selection.available)
      setHour(selection.latest)
      setCursor(selection.latest)
    }).catch((reason: unknown) => {
      if (!controller.signal.aborted) {
        const current = floorHour(Date.now() * 1_000)
        setError(reason instanceof Error ? reason.message : String(reason))
        setHour(current)
        setCursor(current)
      }
    })
    return () => controller.abort()
  }, [])
  // One view is on screen at a time, so one view's sections are what a load
  // fetches. Switching views adds the difference instead of reloading the hour.
  const viewKey = source === "host" ? hostSection : source === "postgresql" ? `postgresql:${pgSection}` : "events"
  const loaded = useRef({ hour: null as number | null, names: new Set<string>() })
  useEffect(() => {
    if (hour === null) return
    setDockClosed(false)
    setSelectedFinding(null)
    setEventScope(null)
    setPgFocus(null)
    setSystemFocus(null)
  }, [hour])
  useEffect(() => {
    if (hour === null) return
    const fresh = loaded.current.hour !== hour
    if (fresh) loaded.current = { hour, names: new Set() }
    const wanted = VIEW_SECTIONS[viewKey] ?? []
    const missing = wanted.filter((name) => !loaded.current.names.has(name))
    if (missing.length === 0) return
    for (const name of missing) loaded.current.names.add(name)
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    void loadHour(hour, controller.signal, missing).then((incoming) => {
      setData((before) => {
        const next = fresh ? incoming : mergeHourData(before, incoming)
        const times = [next.processes, next.health, next.load, next.memory, next.pressure, next.activities]
          .flatMap((rows) => rows.map((row) => row.timestamp))
        if (fresh) setCursor(times.length === 0 ? hour : Math.max(...times))
        return next
      })
      setLoading(false)
    }).catch((reason: unknown) => {
      if (!controller.signal.aborted) {
        for (const name of missing) loaded.current.names.delete(name)
        if (fresh) { setData(EMPTY_DATA); setCursor(hour) }
        setError(reason instanceof Error ? reason.message : String(reason))
        setLoading(false)
      }
    })
    return () => controller.abort()
  }, [hour, viewKey])
  useEffect(() => {
    const shortcuts = (event: KeyboardEvent) => {
      const target = event.target
      if (target instanceof HTMLInputElement || target instanceof HTMLSelectElement || target instanceof HTMLTextAreaElement) return
      if (event.key === "?") setHelpOpen((current) => !current)
      if (event.key === "Escape") setHelpOpen(false)
    }
    window.addEventListener("keydown", shortcuts)
    return () => window.removeEventListener("keydown", shortcuts)
  }, [])

  const processRows = useMemo(() => snapshot(data.processes, cursor), [cursor, data.processes])
  const pgRows = useMemo(() => snapshot(data.activities, cursor), [cursor, data.activities])
  const linkedPids = useMemo(() => new Set(pgRows.flatMap((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid === null ? [] : [pid]
  })), [pgRows])
  useEffect(() => {
    if (selectedKey !== null && processRows.some((row) => processKey(row) === selectedKey)) return
    if (dockClosed) return
    const preferred = processRows.find((row) => {
      const pid = asNumber(value(row, "pid"))
      return pid !== null && linkedPids.has(pid)
    }) ?? processRows[0]
    setSelectedKey(preferred === undefined ? null : processKey(preferred))
  }, [dockClosed, linkedPids, processRows, selectedKey])
  const selectedProcess = processRows.find((row) => processKey(row) === selectedKey) ?? null
  const joinedActivity = activityFor(selectedProcess, data.activities, selectedProcess?.timestamp ?? cursor)
  const processHistory = selectedProcess === null ? [] : data.processes.filter((row) => processKey(row) === processKey(selectedProcess))
  const changeHour = useCallback((next: number) => setHour(floorHour(next)), [])
  const selectProcess = useCallback((row: DataRow) => {
    setDockClosed(false)
    setSelectedKey(processKey(row))
  }, [])
  const selectFinding = useCallback((finding: Finding, grouped: readonly Finding[] = [finding]) => {
    setCursor(finding.timestamp)
    setSelectedFinding(finding)
    if (grouped.length > 1) {
      setEventScope(grouped)
      setSource("events")
      return
    }
    setEventScope(null)
    const resolved = resolveLocator(data, finding)
    const logicalName = finding.logicalName
    if (logicalName === "os_process") {
      setSource("host")
      setHostSection("processes")
      setLens(processLens(fieldNameForLocator(finding)))
      setDockClosed(false)
      if (resolved !== null) setSelectedKey(processKey(resolved.row))
      return
    }
    if (logicalName === "health" || logicalName.startsWith("os_") || logicalName === "instance_metadata") {
      setSource("host")
      setHostSection("system")
      setSystemFocus(finding)
      return
    }
    const section = postgresSection(logicalName)
    if (section !== null) {
      if (resolved === null) {
        setSource("events")
        return
      }
      setSource("postgresql")
      setPgSection(section)
      setPgFocus(resolved.row)
      return
    }
    setSource("events")
  }, [data])
  const pgPresent = data.activities.length !== 0 || data.availableSections.some((name) => name.startsWith("pg_") && !name.startsWith("pg_log_"))
  const eventsPresent = data.findings.length !== 0
  const helpItems = source === "postgresql"
    ? HELP_POSTGRESQL
    : source === "events"
      ? HELP_EVENTS
      : hostSection === "processes" ? HELP_PROCESS : HELP_SYSTEM
  useEffect(() => {
    if (source === "postgresql" && !pgPresent) setSource("host")
    if (source === "events" && !eventsPresent) setSource("host")
  }, [eventsPresent, pgPresent, source])

  return <main className="app-shell">
    <header className="topbar">
      <span className="brand-mark"><Activity aria-hidden="true" size={15} strokeWidth={2} /></span>
      <h1>{t("app.title")}</h1>

      <nav aria-label={t("nav.sources")} className="source-tabs">
        <button aria-current={source === "host" ? "page" : undefined} className={source === "host" ? "source-active" : undefined} onClick={() => setSource("host")} type="button">{t("nav.host")}</button>
        <button aria-current={source === "postgresql" ? "page" : undefined} className={source === "postgresql" ? "source-active" : undefined} disabled={!pgPresent} onClick={() => setSource("postgresql")} title={pgPresent ? undefined : t("nav.no_data")} type="button">{t("nav.postgresql")}</button>
        {eventsPresent && <button aria-current={source === "events" ? "page" : undefined} className={source === "events" ? "source-active" : undefined} onClick={() => { setEventScope(null); setSource("events") }} type="button">{t("nav.events")}</button>}
      </nav>

      {source === "host" && <div className="section-tabs" role="tablist">
        <button aria-selected={hostSection === "system"} onClick={() => setHostSection("system")} role="tab" type="button">{t("section.system")}</button>
        <button aria-selected={hostSection === "processes"} data-testid="process-tab" onClick={() => setHostSection("processes")} role="tab" type="button">{t("section.processes")}</button>
      </div>}

      <HourPicker availableHours={availableHours} changeHour={changeHour} hour={hour} locale={locale} t={t} />
      <span className="cursor-time">{formatUtc(cursor)}</span>

      <div className="top-actions">
        <button aria-label={t("common.theme.switch")} className="icon-button" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} title={t(theme === "dark" ? "common.theme.light" : "common.theme.dark")} type="button">
          {theme === "dark" ? <Sun aria-hidden="true" size={15} /> : <Moon aria-hidden="true" size={15} />}
        </button>
        <div aria-label={t("locale.switch")} className="locale-switch" role="group">
          <Languages aria-hidden="true" size={13} />
          {(["ru", "en"] as const).map((choice) => <button aria-pressed={locale === choice} data-testid={`locale-${choice}`} key={choice} onClick={() => setLocale(choice)} type="button">{t(`locale.${choice}`)}</button>)}
        </div>
        <button aria-expanded={helpOpen} aria-label={t("help.open")} className="icon-button" data-testid="help-trigger" onClick={() => setHelpOpen((current) => !current)} type="button"><HelpCircle aria-hidden="true" size={15} /></button>
      </div>
    </header>

    <section className="workspace">
      <p aria-live="polite" className="live-note">
        {t(`nav.${source}`)}
        {source === "host" ? ` · ${t(`section.${hostSection}`)}` : ""}
        {source === "postgresql" ? ` · ${t(`pg.section.${pgSection}`)}` : ""}
      </p>
      {loading && <StateCard message={t("status.loading")} />}
      {!loading && error !== null && <StateCard message={t("status.error")} />}
      {!loading && error === null && hour !== null && source === "host" && hostSection === "system" && <SystemView cursor={cursor} data={data} focus={systemFocus} hour={hour} locale={locale} onCursor={setCursor} onFinding={selectFinding} t={t} />}
      {!loading && error === null && hour !== null && source === "host" && hostSection === "processes" && <>
        <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={data.load} memory={data.memory} onCursor={setCursor} onFinding={selectFinding} pressure={data.pressure} t={t} />
        <div className="lensbar">
          <div aria-label={t("section.processes")} className="lens-tabs" role="group">
            {(["generic", "cpu", "memory", "disk"] as const).map((choice) => <button aria-pressed={lens === choice} data-testid={`lens-${choice}`} key={choice} onClick={() => setLens(choice)} type="button">{t(`lens.${choice}`)}</button>)}
          </div>
          <span>{processRows[0] === undefined ? t("status.no_data") : formatUtc(processRows[0].timestamp)}</span>
        </div>
        <ProcessSummary lens={lens} linkedPids={linkedPids} locale={locale} rows={processRows} t={t} />
        <div className={selectedProcess === null ? "process-layout process-layout-table" : "process-layout"}>
          <ProcessTable lens={lens} linkedPids={linkedPids} locale={locale} onSelect={selectProcess} rows={processRows} selectedKey={selectedKey} t={t} />
          {selectedProcess !== null && <DetailDock activity={joinedActivity.row} activityTime={joinedActivity.snapshotTime} hour={hour} lens={lens} locale={locale} onClose={() => { setDockClosed(true); setSelectedKey(null) }} process={selectedProcess} processHistory={processHistory} t={t} />}
        </div>
      </>}
      {!loading && error === null && hour !== null && source === "postgresql" && <PostgresView cursor={cursor} data={data} focus={pgFocus} focusFinding={selectedFinding} hour={hour} locale={locale} onCursor={setCursor} onFinding={selectFinding} onSection={setPgSection} section={pgSection} t={t} />}
      {!loading && error === null && hour !== null && source === "events" && <EventsView cursor={cursor} data={data} hour={hour} onCursor={setCursor} onFinding={selectFinding} onShowAll={() => setEventScope(null)} resolve={(finding) => resolveLocator(data, finding)?.row ?? null} scope={eventScope} selected={selectedFinding} t={t} />}
    </section>

    {helpOpen && <HelpPanel items={helpItems} onClose={() => setHelpOpen(false)} t={t} />}
  </main>
}

function StateCard({ message }: { readonly message: string }) {
  return <div className="loading-card"><p className="eyebrow">KRONIKA</p><h2>{message}</h2></div>
}

function postgresSection(logicalName: string): PostgresSection | null {
  if (logicalName === "pg_stat_activity" || logicalName === "pg_stat_progress_vacuum") return "activity"
  if (logicalName === "pg_stat_statements") return "statements"
  if (logicalName === "pg_locks") return "locks"
  if (logicalName === "pg_stat_database") return "databases"
  if (logicalName.startsWith("pg_") && !logicalName.startsWith("pg_log_")) return "overview"
  return null
}

function initialTheme(): Theme {
  try {
    const saved = localStorage.getItem("kronika.theme")
    if (saved === "dark" || saved === "light") return saved
  } catch { /* storage can be disabled */ }
  return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark"
}

function initialLocale(): Locale {
  try {
    const saved = localStorage.getItem("kronika.locale")
    if (saved === "ru" || saved === "en") return saved
  } catch { /* storage can be disabled */ }
  for (const language of navigator.languages) {
    if (language.toLowerCase().startsWith("ru")) return "ru"
    if (language.toLowerCase().startsWith("en")) return "en"
  }
  return "en"
}

const root = document.getElementById("root")
if (root === null) throw new Error("Kronika UI root is missing")
createRoot(root).render(<App />)
