import { Activity, ChevronLeft, ChevronRight, Database, HelpCircle, Languages, MemoryStick, Server, Users } from "lucide-react"
import { dictionaries } from "kronika:i18n"
import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react"
import { createRoot } from "react-dom/client"

import { discoverLatestHour, loadHour, type DataRow, type HourData } from "./api"
import { DetailDock } from "./detail"
import { HelpPanel, LabelHelp, type Translate } from "./help"
import {
  activityFor,
  asNumber,
  floorHour,
  formatUtc,
  inputDay,
  inputHour,
  interpolate,
  measure,
  nearestRow,
  processKey,
  selectedHour,
  snapshot,
  systemSnapshots,
  value,
  type Lens,
  type Locale,
} from "./model"
import { ProcessTable } from "./process-table"
import { SystemTable } from "./system-table"
import { Timeline } from "./timeline"

const EMPTY_DATA: HourData = {
  processes: [], activities: [], load: [], memory: [], pressure: [], health: [], points: [],
  findings: [], sourceFamilies: [], segmentCount: 0,
}

const HELP_SYSTEM = [
  { label: "metric.health.label", help: "metric.health.help" },
  { label: "metric.load1.label", help: "metric.load1.help" },
  { label: "metric.mem_available.label", help: "metric.mem_available.help" },
  { label: "metric.processes.label", help: "metric.processes.help" },
  { label: "metric.pg_backends.label", help: "metric.pg_backends.help" },
  { label: "lane.health.label", help: "lane.health.help" },
  { label: "lane.load.label", help: "lane.load.help" },
  { label: "lane.memory.label", help: "lane.memory.help" },
  { label: "lane.pressure.label", help: "lane.pressure.help" },
  { label: "lane.locators.label", help: "lane.locators.help" },
  { label: "system_table.time.label", help: "system_table.time.help" },
  { label: "system_table.health.label", help: "system_table.health.help" },
  { label: "system_table.load1.label", help: "system_table.load1.help" },
  { label: "system_table.mem_available.label", help: "system_table.mem_available.help" },
] as const

const HELP_PROCESS = [
  { label: "col.pid.label", help: "col.pid.help" },
  { label: "col.starttime.label", help: "col.starttime.help" },
  { label: "col.command.label", help: "col.command.help" },
  { label: "detail.pg.title", help: "detail.pg.help" },
  { label: "pg.query.label", help: "pg.query.help" },
] as const

function App() {
  const [locale, setLocale] = useState<Locale>(initialLocale)
  const [hour, setHour] = useState<number | null>(null)
  const [cursor, setCursor] = useState(0)
  const [data, setData] = useState<HourData>(EMPTY_DATA)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [section, setSection] = useState<"system" | "processes">("system")
  const [lens, setLens] = useState<Lens>("generic")
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [dockClosed, setDockClosed] = useState(false)
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
    const controller = new AbortController()
    void discoverLatestHour(controller.signal).then((latest) => {
      setHour(latest)
      setCursor(latest)
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
  useEffect(() => {
    if (hour === null) return
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    setDockClosed(false)
    void loadHour(hour, controller.signal).then((loaded) => {
      const storedTimes = [loaded.processes, loaded.health, loaded.load, loaded.memory, loaded.pressure, loaded.activities]
        .flatMap((rows) => rows.map((row) => row.timestamp))
      setData(loaded)
      setCursor(storedTimes.length === 0 ? hour : Math.max(...storedTimes))
      setLoading(false)
    }).catch((reason: unknown) => {
      if (!controller.signal.aborted) {
        setData(EMPTY_DATA)
        setCursor(hour)
        setError(reason instanceof Error ? reason.message : String(reason))
        setLoading(false)
      }
    })
    return () => controller.abort()
  }, [hour])
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
  const joinedActivity = activityFor(selectedProcess, data.activities, cursor)
  const changeHour = useCallback((next: number) => setHour(floorHour(next)), [])
  const selectProcess = useCallback((row: DataRow) => {
    setDockClosed(false)
    setSelectedKey(processKey(row))
  }, [])
  const day = hour === null ? "" : inputDay(hour)
  const hourNumber = hour === null ? 0 : inputHour(hour)
  const pgPresent = data.sourceFamilies.some((source) => source.name === "postgresql" && source.present)
  const snapshotCount = new Set(data.processes.map((row) => row.timestamp)).size

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark"><Activity aria-hidden="true" size={18} strokeWidth={1.75} /></span>
          <div><p className="eyebrow">{t("app.kicker")}</p><h1>{t("app.title")}</h1></div>
        </div>
        <div className="top-actions">
          <span className="offline-state">{t("app.offline")}</span>
          <div aria-label={t("locale.switch")} className="locale-switch" role="group">
            <Languages aria-hidden="true" size={14} />
            {(["ru", "en"] as const).map((choice) => (
              <button aria-pressed={locale === choice} data-testid={`locale-${choice}`} key={choice} onClick={() => setLocale(choice)} type="button">{t(`locale.${choice}`)}</button>
            ))}
          </div>
          <button aria-expanded={helpOpen} aria-label={t("help.open")} className="icon-button" data-testid="help-trigger" onClick={() => setHelpOpen((current) => !current)} type="button"><HelpCircle aria-hidden="true" size={17} /></button>
        </div>
      </header>

      <nav aria-label="Sources" className="source-tabs">
        <button aria-current="page" className="source-active" type="button">{t("nav.host")}</button>
        {["postgresql", "pgbouncer", "clickhouse", "events"].map((name) => <button disabled key={name} title={t("nav.future")} type="button">{t(`nav.${name}`)}</button>)}
      </nav>

      <section className="toolbar">
        <div className="section-tabs" role="tablist">
          <button aria-selected={section === "system"} onClick={() => setSection("system")} role="tab" type="button">{t("section.system")}</button>
          <button aria-selected={section === "processes"} data-testid="process-tab" onClick={() => setSection("processes")} role="tab" type="button">{t("section.processes")}</button>
        </div>
        <div className="hour-picker" data-testid="hour-picker">
          <button aria-label={t("hour.previous")} disabled={hour === null} onClick={() => hour !== null && changeHour(hour - 3_600_000_000)} type="button"><ChevronLeft aria-hidden="true" size={15} /></button>
          <label><span>{t("hour.day")}</span><input onChange={(event) => { const next = selectedHour(event.target.value, hourNumber); if (next !== null) changeHour(next) }} type="date" value={day} /></label>
          <label><span>{t("hour.hour")}</span><select onChange={(event) => { const next = selectedHour(day, Number(event.target.value)); if (next !== null) changeHour(next) }} value={hourNumber}>{Array.from({ length: 24 }, (_, number) => <option key={number} value={number}>{String(number).padStart(2, "0")}:00</option>)}</select></label>
          <button aria-label={t("hour.next")} disabled={hour === null} onClick={() => hour !== null && changeHour(hour + 3_600_000_000)} type="button"><ChevronRight aria-hidden="true" size={15} /></button>
        </div>
      </section>

      <section className="workspace">
        <div className="status-strip" aria-live="polite">
          <span>{t("status.segments", { count: data.segmentCount })}</span>
          <span>{t("status.snapshots", { count: snapshotCount })}</span>
          <span>{t("status.process_rows", { count: processRows.length })}</span>
          <span className={pgPresent ? "status-on" : ""}>{t(pgPresent ? "status.pg_present" : "status.pg_absent")}</span>
          <span className="cursor-time">{formatUtc(cursor)}</span>
        </div>
        {loading && <StateCard message={t("status.loading")} />}
        {!loading && error !== null && <StateCard code={error} message={t("status.error")} />}
        {!loading && error === null && hour !== null && section === "system" && <SystemView cursor={cursor} data={data} hour={hour} locale={locale} onCursor={setCursor} processRows={processRows} pgRows={pgRows} t={t} />}
        {!loading && error === null && hour !== null && section === "processes" && <>
          <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={data.load} memory={data.memory} onCursor={setCursor} pressure={data.pressure} t={t} />
          <div className="lensbar">
            <div aria-label={t("section.processes")} className="lens-tabs" role="group">
              {(["generic", "cpu", "memory", "disk"] as const).map((choice) => <button aria-pressed={lens === choice} data-testid={`lens-${choice}`} key={choice} onClick={() => setLens(choice)} type="button">{t(`lens.${choice}`)}</button>)}
            </div>
            <span>{processRows[0] === undefined ? t("status.no_data") : formatUtc(processRows[0].timestamp)}</span>
          </div>
          <div className={selectedProcess === null ? "process-layout process-layout-table" : "process-layout"}>
            <ProcessTable lens={lens} linkedPids={linkedPids} locale={locale} onSelect={selectProcess} rows={processRows} selectedKey={selectedKey} t={t} />
            {selectedProcess !== null && <DetailDock activity={joinedActivity.row} activitySnapshotTime={joinedActivity.snapshotTime} lens={lens} locale={locale} onClose={() => { setDockClosed(true); setSelectedKey(null) }} process={selectedProcess} t={t} />}
          </div>
        </>}
      </section>

      {helpOpen && <HelpPanel items={section === "system" ? HELP_SYSTEM : [...HELP_SYSTEM, ...HELP_PROCESS]} onClose={() => setHelpOpen(false)} t={t} />}
    </main>
  )
}

function SystemView({
  cursor,
  data,
  hour,
  locale,
  onCursor,
  processRows,
  pgRows,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (value: number) => void
  readonly processRows: readonly DataRow[]
  readonly pgRows: readonly DataRow[]
  readonly t: Translate
}) {
  const health = nearestRow(data.health, cursor)
  const load = nearestRow(data.load, cursor)
  const memory = nearestRow(data.memory, cursor)
  const snapshots = useMemo(() => systemSnapshots(data.health, data.load, data.memory, data.pressure), [data.health, data.load, data.memory, data.pressure])
  return <>
    <section className="metric-strip">
      <Metric icon={<Activity size={15} />} help="metric.health.help" label="metric.health.label" t={t} value={measure(value(health, "os_health"), locale, "%")} />
      <Metric icon={<Server size={15} />} help="metric.load1.help" label="metric.load1.label" t={t} value={measure(value(load, "load1"), locale)} />
      <Metric icon={<MemoryStick size={15} />} help="metric.mem_available.help" label="metric.mem_available.label" t={t} value={measure(value(memory, "mem_available"), locale, " KiB")} />
      <Metric icon={<Users size={15} />} help="metric.processes.help" label="metric.processes.label" t={t} value={data.processes.length === 0 ? "—" : measure(processRows.length, locale)} />
      <Metric icon={<Database size={15} />} help="metric.pg_backends.help" label="metric.pg_backends.label" t={t} value={data.sourceFamilies.some((source) => source.name === "postgresql" && source.present) ? measure(pgRows.length, locale) : "—"} />
    </section>
    <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} load={data.load} memory={data.memory} onCursor={onCursor} pressure={data.pressure} t={t} />
    <div className="locator-legend">
      {(["known_bad", "spike", "event"] as const).map((kind) => <span key={kind}><i className={`marker-key marker-${kind}`} />{t(`locator.${kind}`)} <b>{data.findings.filter((finding) => finding.kind === kind).length}</b></span>)}
    </div>
    <SystemTable cursor={cursor} locale={locale} onCursor={onCursor} rows={snapshots} t={t} />
  </>
}

function Metric({ icon, help, label, t, value: output }: { readonly icon: ReactNode; readonly help: string; readonly label: string; readonly t: Translate; readonly value: string }) {
  return <article className="metric-card"><div className="metric-label"><span>{icon}</span><LabelHelp helpKey={help} labelKey={label} t={t} /></div><strong>{output}</strong></article>
}

function StateCard({ code, message }: { readonly code?: string; readonly message: string }) {
  return <div className="loading-card"><p className="eyebrow">HOST</p><h2>{message}</h2>{code !== undefined && <code>{code}</code>}</div>
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
