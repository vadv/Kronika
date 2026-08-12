import { Activity, HelpCircle, Languages, Moon, Sun } from "lucide-react"
import { dictionaries } from "kronika:i18n"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { createRoot } from "react-dom/client"

import {
  TIMELINE_REQUESTS,
  loadTimeline,
  loadSeries,
  hourOf,
  loadSnapshot,
  loadSectionFindings,
  segmentBoundAt,
  requestsForSegment,
  fieldNameForLocator,
  replaceSections,
  replaceFindings,
  resolveLocator,
  PRODUCT_SECTION_GROUPS,
  type DataRow,
  type SectionRequest,
  type SegmentBound,
  type Finding,
  type HourData,
} from "./api"
import type { TableOrder } from "./entity-table"
import { hostSectionOf, pgSectionOf, readAddress, sourceOf, stepOf, viewOf, writeAddress } from "./address"
import { DetailDock, PROCESS_HISTORY_FIELDS } from "./detail"
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
  rawText,
  shownMoment,
  snapshot,
  value,
  type Lens,
  type Locale,
} from "./model"
import { PostgresView, type PostgresSection } from "./postgres-view"
import { POSTGRES_SECTION_REQUESTS, planRequest, postgresProjection, statementRequest, type PlanLens, type StatementLens } from "./postgres-metrics"
import { ProcessSummary, ProcessTable } from "./process-table"
import { SYSTEM_REQUESTS, SystemView } from "./system-view"
import { Timeline } from "./timeline"

type Source = "host" | "postgresql" | "events"
type Theme = "dark" | "light"
type HostSection = "system" | "processes"

const EMPTY_DATA: HourData = {
  sections: {}, rateColumns: {}, availableSections: [], processes: [], activities: [], load: [], memory: [], pressure: [], health: [],
  pgOverview: [], pgStatements: [], pgPlans: [], pgLocks: [], pgDatabases: [], pgEvents: [], points: [], lanePoints: [], findings: [],
  sourceFamilies: [], segmentCount: 0,
}

/** Each view is built from what it draws: the timeline every view shows, plus
 *  that view's own sections. Nothing asks for an hour of everything. */
const VIEW_REQUESTS: Readonly<Record<string, readonly SectionRequest[]>> = {
  system: [...TIMELINE_REQUESTS, ...SYSTEM_REQUESTS],
  processes: [...TIMELINE_REQUESTS, { section: "os_process" }, { section: "pg_stat_activity" }, { section: "instance_metadata" }],
  "postgresql:overview": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlOverview.map(section)],
  "postgresql:activity": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlActivity.map(section)],
  "postgresql:locks": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlLocks.map(section)],
  "postgresql:databases": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlDatabases.map(section)],
  events: [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.events.map(section)],
}

function section(name: string): SectionRequest { return { section: name } }

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
  // The address is read once. From then on it is written from the state:
  // syncing both ways would be a loop.
  const opened = useRef(readAddress(window.location.search))
  const [locale, setLocale] = useState<Locale>(initialLocale)
  const [theme, setTheme] = useState<Theme>(initialTheme)
  const [hour, setHour] = useState<number | null>(opened.current.at === null ? null : floorHour(opened.current.at))
  // The moment the link named, kept until the hour it belongs to has loaded.
  const wanted = useRef(opened.current.at)
  const [availableHours, setAvailableHours] = useState<readonly number[]>([])
  const [cursor, setCursor] = useState(0)
  const [data, setData] = useState<HourData>(EMPTY_DATA)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [source, setSource] = useState<Source>(sourceOf(opened.current.view))
  const [hostSection, setHostSection] = useState<HostSection>(hostSectionOf(opened.current.view))
  const [pgSection, setPgSection] = useState<PostgresSection>(pgSectionOf(opened.current.view))
  const [statementLens, setStatementLens] = useState<StatementLens>("load")
  const [planLens, setPlanLens] = useState<PlanLens>("load")
  const [lens, setLens] = useState<Lens>(opened.current.lens)
  const [find, setFind] = useState(opened.current.find)
  // The statement table is ordered and cut by the server, so its header asks
  // for a different order rather than reshuffling the rows that arrived.
  const [order, setOrder] = useState<TableOrder | null>(opened.current.sort)
  const [selectedKey, setSelectedKey] = useState<string | null>(opened.current.row)
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
  // One view is on screen at a time, so one view's sections are what a load
  // fetches. Switching views adds the difference instead of reloading the hour.
  const baseViewKey = source === "host" ? hostSection : source === "postgresql" ? `postgresql:${pgSection}` : "events"
  const viewKey = pgSection === "statements" && source === "postgresql"
    ? `${baseViewKey}:${statementLens}`
    : pgSection === "plans" && source === "postgresql" ? `${baseViewKey}:${planLens}` : baseViewKey
  const viewRequests = useMemo(() => {
    if (source === "postgresql" && pgSection === "statements") return [...TIMELINE_REQUESTS, statementRequest(statementLens)]
    if (source === "postgresql" && pgSection === "plans") return [
      ...TIMELINE_REQUESTS,
      planRequest(planLens),
      ...POSTGRES_SECTION_REQUESTS.filter((request) => request.section === "pg_store_plans_info"),
    ]
    return VIEW_REQUESTS[baseViewKey] ?? []
  }, [baseViewKey, pgSection, planLens, source, statementLens])
  const [segments, setSegments] = useState<readonly SegmentBound[]>([])
  const loaded = useRef({ hour: null as number | null, keys: new Set<string>() })
  const loadedFindingSections = useRef(new Set<string>())
  const drawn = useRef<number | null>(null)
  useEffect(() => {
    if (hour === null) return
    setDockClosed(false)
    setSelectedFinding(null)
    setEventScope(null)
    setPgFocus(null)
    setSystemFocus(null)
  }, [hour])
  // Two loads, and they are different in kind. The line, its marks and the
  // segments of the hour are one request. A table shows the snapshot under the
  // cursor, so it wants the one segment holding it — not the nine an hour has.
  useEffect(() => {
    if (hour !== null && drawn.current === hour) return
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    loaded.current = { hour, keys: new Set() }
    loadedFindingSections.current.clear()
    void loadTimeline(hour, controller.signal).then((timeline) => {
      drawn.current = timeline.hour
      setAvailableHours(timeline.availableHours)
      setHour(timeline.hour)
      setSegments(timeline.segments)
      setData(hourOf(timeline))
      const times = timeline.health.map((row) => row.timestamp)
      const asked = wanted.current
      wanted.current = null
      setCursor(asked !== null && floorHour(asked) === timeline.hour
        ? asked
        : times.length === 0 ? timeline.hour : Math.max(...times))
      setLoading(false)
    }).catch((reason: unknown) => {
      if (controller.signal.aborted) return
      const fallback = hour ?? floorHour(Date.now() * 1_000)
      drawn.current = fallback
      setHour(fallback)
      setSegments([])
      setData(EMPTY_DATA)
      setCursor(fallback)
      setError(reason instanceof Error ? reason.message : String(reason))
      setLoading(false)
    })
    return () => controller.abort()
  }, [hour])

  // A table shows the moment under the cursor, and each section is sampled on
  // its own schedule: asking for the cursor lets the server pick the last
  // sample of that section, rather than a moment borrowed from another one.
  const cursorSegment = useMemo(() => segmentBoundAt(segments, cursor), [cursor, segments])
  // Dragging moves the cursor at once and the tables a round trip later, and
  // the cursor can rest where nothing was sampled at all. Both are said out
  // loud beside the clock instead of being left to guess.
  const [cursorState, setCursorState] = useState<"ready" | "loading" | "missing">("ready")
  useEffect(() => {
    if (hour === null || cursorSegment === null) return
    const wanted = requestsForSegment(
      viewRequests.filter((request) => request.section !== "health"),
      cursorSegment,
    )
    const key = `${cursorSegment.id}:${cursor}:${viewKey}:${order?.column ?? ""}:${order?.descending ?? ""}`
    if (loaded.current.keys.has(key) || wanted.length === 0) {
      setCursorState("ready")
      return
    }
    setCursorState("loading")
    const controller = new AbortController()
    // Dragging the cursor crosses many samples; only the one it rests on is
    // worth a round trip over a link that costs more than a second.
    const timer = setTimeout(() => {
      loaded.current.keys.add(key)
      void loadSnapshot(cursorSegment.id, cursor, wanted, controller.signal, order ?? undefined)
        .then((incoming) => {
          setData((before) => replaceSections(before, incoming))
          setCursorState("ready")
        })
        // A moment the collector never reached is an ordinary answer, not a
        // broken screen: the tables keep the last moment they hold.
        .catch((reason: unknown) => {
          if (controller.signal.aborted) return
          loaded.current.keys.delete(key)
          setCursorState("missing")
          console.error("kronika: snapshot at the cursor failed", reason)
        })
    }, 250)
    return () => { clearTimeout(timer); controller.abort() }
  }, [cursor, cursorSegment, hour, order, viewKey, viewRequests])

  const findingSection = source !== "postgresql" ? null
    : pgSection === "activity" ? "pg_stat_activity"
      : pgSection === "statements" ? "pg_stat_statements"
        : pgSection === "databases" ? "pg_stat_database" : null
  useEffect(() => {
    if (hour === null || findingSection === null || segments.length === 0) return
    const key = `${hour}:${findingSection}`
    if (loadedFindingSections.current.has(key)) return
    loadedFindingSections.current.add(key)
    const controller = new AbortController()
    void loadSectionFindings(segments, findingSection, controller.signal)
      .then((findings) => setData((before) => replaceFindings(before, findingSection, findings)))
      .catch((reason: unknown) => {
        loadedFindingSections.current.delete(key)
        if (!controller.signal.aborted) console.error("kronika: section findings failed", reason)
      })
    return () => controller.abort()
  }, [findingSection, hour, segments])

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

  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  const processRows = useMemo(() => snapshot(data.processes, cursor), [cursor, data.processes])
  // A CPU counter is ticks per second, and the machine says how many ticks its
  // second holds.
  const ticksPerSecond = useMemo(() => {
    const metadata = (data.sections.instance_metadata ?? [])[0]
    return metadata === undefined ? null : asNumber(value(metadata, "clock_ticks_per_sec"))
  }, [data.sections])
  const pgRows = useMemo(() => snapshot(data.activities, cursor), [cursor, data.activities])
  const linkedPids = useMemo(() => new Set(pgRows.flatMap((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid === null ? [] : [pid]
  })), [pgRows])
  useEffect(() => {
    if (selectedKey !== null && processRows.some((row) => processKey(row) === selectedKey)) return
    // Before the table has rows there is nothing to fall back to, and falling
    // back anyway would throw away the row the link asked for.
    if (processRows.length === 0 || dockClosed) return
    const preferred = processRows.find((row) => {
      const pid = asNumber(value(row, "pid"))
      return pid !== null && linkedPids.has(pid)
    }) ?? processRows[0]
    setSelectedKey(preferred === undefined ? null : processKey(preferred))
  }, [dockClosed, linkedPids, processRows, selectedKey])
  const selectedProcess = processRows.find((row) => processKey(row) === selectedKey) ?? null
  useEffect(() => {
    if (selectedFinding?.logicalName !== "os_process") return
    const exact = resolveLocator(data, selectedFinding)?.row
    if (exact !== undefined && processRows.some((row) => processKey(row) === processKey(exact))) {
      setSelectedKey(processKey(exact))
    }
  }, [data, processRows, selectedFinding])
  useEffect(() => {
    if (selectedFinding === null || postgresSection(selectedFinding.logicalName) === null) return
    const exact = resolveLocator(data, selectedFinding)?.row
    if (exact !== undefined) {
      setPgFocus(exact)
      return
    }
    if (selectedFinding.logicalName !== "pg_stat_statements" && selectedFinding.logicalName !== "pg_store_plans") return
    const fields = postgresProjection(selectedFinding.typeId)
    if (fields.length === 0) return
    const controller = new AbortController()
    void loadSnapshot(
      selectedFinding.segmentId,
      selectedFinding.timestamp,
      [{ section: selectedFinding.logicalName, fields, typeId: selectedFinding.typeId }],
      controller.signal,
      undefined,
      { typeId: selectedFinding.typeId, rowOrdinal: selectedFinding.rowOrdinal },
    ).then((incoming) => {
      const row = incoming.sections[selectedFinding.logicalName]?.[0]
      if (row !== undefined) setPgFocus(row)
    }).catch(() => { /* the marker remains routed to its table */ })
    return () => controller.abort()
  }, [data, selectedFinding])
  const joinedActivity = activityFor(selectedProcess, data.activities, selectedProcess?.timestamp ?? cursor)
  // A table holds one moment, so the charts of the selected process are their
  // own request across the hour.
  const [processHistory, setProcessHistory] = useState<readonly DataRow[]>([])
  const selectedPid = selectedProcess === null ? null : rawText(value(selectedProcess, "pid"))
  const selectedStart = selectedProcess === null ? null : rawText(value(selectedProcess, "starttime"))
  useEffect(() => {
    if (hour === null || selectedPid === null || selectedStart === null) {
      setProcessHistory([])
      return
    }
    const controller = new AbortController()
    void loadSeries(hour, "os_process", { pid: selectedPid, starttime: selectedStart }, PROCESS_HISTORY_FIELDS, controller.signal)
      .then(setProcessHistory)
      .catch(() => { /* the panel stands without its charts */ })
    return () => controller.abort()
  }, [hour, selectedPid, selectedStart])
  // The address follows the state. Dragging the cursor replaces the entry, so
  // the back button does not walk back through every pixel of the drag;
  // choosing a view or a row pushes one.
  const address = useMemo(() => writeAddress({
    at: cursor === 0 ? null : cursor,
    view: viewOf(source, hostSection, pgSection),
    lens,
    sort: order,
    row: selectedKey,
    find,
  }), [cursor, find, hostSection, lens, order, pgSection, selectedKey, source])
  const steps = useRef<string | null>(null)
  useEffect(() => {
    if (window.location.pathname + window.location.search === address) return
    const dragging = steps.current !== null && stepOf(steps.current) === stepOf(address)
    steps.current = address
    window.history[dragging ? "replaceState" : "pushState"]({}, "", address)
  }, [address])
  useEffect(() => {
    const back = () => {
      const opening = readAddress(window.location.search)
      setSource(sourceOf(opening.view))
      setHostSection(hostSectionOf(opening.view))
      setPgSection(pgSectionOf(opening.view))
      setLens(opening.lens)
      setOrder(opening.sort)
      setSelectedKey(opening.row)
      setFind(opening.find)
      if (opening.at !== null) {
        wanted.current = opening.at
        setCursor(opening.at)
        setHour(floorHour(opening.at))
      }
    }
    window.addEventListener("popstate", back)
    return () => window.removeEventListener("popstate", back)
  }, [])
  const changeHour = useCallback((next: number) => setHour(floorHour(next)), [])
  const selectProcess = useCallback((row: DataRow) => {
    setDockClosed(false)
    setSelectedKey(processKey(row))
  }, [])
  const selectFinding = useCallback((finding: Finding, grouped: readonly Finding[] = [finding]) => {
    setCursor(finding.timestamp)
    setSelectedFinding(finding)
    setFind("")
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
      setSystemFocus(null)
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
      setSource("postgresql")
      setPgSection(section)
      if (logicalName === "pg_stat_statements") setStatementLens("load")
      setSystemFocus(null)
      setPgFocus(resolved?.row ?? null)
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
    // While the hour is still loading nothing is present yet, and sending the
    // reader to the host view would throw away the view their link asked for.
    if (loading) return
    if (source === "postgresql" && !pgPresent) setSource("host")
    if (source === "events" && !eventsPresent) setSource("host")
  }, [eventsPresent, loading, pgPresent, source])

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
      <span className="cursor-time">{formatUtc(cursor)}
        {cursorState !== "ready" && <span className={cursorState === "loading" ? "cursor-behind" : "cursor-missing"} data-testid="cursor-behind">{t(cursorState === "loading" ? "status.updating" : "status.no_sample")}</span>}
      </span>

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

    <section className={cursorState === "loading" ? "workspace workspace-behind" : "workspace"}>
      <p aria-live="polite" className="live-note">
        {t(`nav.${source}`)}
        {source === "host" ? ` · ${t(`section.${hostSection}`)}` : ""}
        {source === "postgresql" ? ` · ${t(`pg.section.${pgSection}`)}` : ""}
      </p>
      {loading && <StateCard message={t("status.loading")} />}
      {!loading && error !== null && <StateCard message={t("status.error")} />}
      {!loading && error === null && hour !== null && source === "host" && hostSection === "system" && <SystemView cursor={cursor} data={data} focus={systemFocus} hour={hour} locale={locale} onCursor={setCursor} onFinding={selectFinding} t={t} />}
      {!loading && error === null && hour !== null && source === "host" && hostSection === "processes" && <>
        <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} onCursor={setCursor} onFinding={selectFinding} primaryLane={lens === "cpu" ? "cpu_busy" : lens === "memory" ? "memory" : lens === "disk" ? "io_stall" : "health"} shownAt={shownAt} t={t} />
        <div className="lensbar">
          <div aria-label={t("section.processes")} className="lens-tabs" role="group">
            {(["generic", "cpu", "memory", "disk"] as const).map((choice) => <button aria-pressed={lens === choice} data-testid={`lens-${choice}`} key={choice} onClick={() => setLens(choice)} type="button">{t(`lens.${choice}`)}</button>)}
          </div>
          <span>{processRows[0] === undefined ? t("status.no_data") : formatUtc(processRows[0].timestamp)}</span>
        </div>
        <ProcessSummary lens={lens} linkedPids={linkedPids} locale={locale} rows={processRows} t={t} ticksPerSecond={ticksPerSecond} />
        <div className={selectedProcess === null ? "process-layout process-layout-table" : "process-layout"}>
          <ProcessTable finding={selectedFinding?.logicalName === "os_process" ? selectedFinding : null} findingField={selectedFinding?.logicalName === "os_process" ? fieldNameForLocator(selectedFinding) : null} lens={lens} linkedPids={linkedPids} locale={locale} onOrder={setOrder} onPattern={setFind} onSelect={selectProcess} order={order} pattern={find} rows={processRows} selectedKey={selectedKey} t={t} ticksPerSecond={ticksPerSecond} />
          {selectedProcess !== null && <DetailDock activity={joinedActivity.row} activityTime={joinedActivity.snapshotTime} cursor={cursor} hour={hour} lens={lens} locale={locale} onClose={() => { setDockClosed(true); setSelectedKey(null) }} process={selectedProcess} processHistory={processHistory} t={t} ticksPerSecond={ticksPerSecond} />}
        </div>
      </>}
      {!loading && error === null && hour !== null && source === "postgresql" && <PostgresView onOrder={setOrder} onPattern={setFind} order={order ?? undefined} pattern={find} cursor={cursor} data={data} focus={pgFocus} focusFinding={selectedFinding} hour={hour} locale={locale} onCursor={setCursor} onFinding={selectFinding} onPlanLens={(next) => { setOrder(null); setPlanLens(next) }} onSection={setPgSection} onStatementLens={(next) => { setOrder(null); setStatementLens(next) }} planLens={planLens} section={pgSection} statementLens={statementLens} t={t} />}
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
  if (logicalName === "pg_store_plans" || logicalName === "pg_store_plans_info") return "plans"
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
