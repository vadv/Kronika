import { Activity } from "lucide-react"
import { translation } from "kronika:i18n"
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react"
import { createRoot } from "react-dom/client"

import {
  TIMELINE_REQUESTS,
  loadTimeline,
  loadSeries,
  hourOf,
  loadSnapshot,
  mergeSnapshotData,
  segmentBoundAt,
  requestsForSegment,
  fieldNameForLocator,
  viewData,
  resolveLocator,
  PRODUCT_SECTION_GROUPS,
  POSTGRESQL_OVERVIEW_REQUESTS,
  type DataRow,
  type SectionRequest,
  type SegmentBound,
  type Finding,
  type HourData,
  type TimelineData,
} from "./api"
import type { TableOrder } from "./entity-table"
import { hostSectionOf, pgSectionOf, readAddress, sourceOf, stepOf, viewOf, writeAddress, type PgLens } from "./address"
import { DetailDock, PROCESS_HISTORY_FIELDS } from "./detail"
import { loadDisplayTimeZone, saveDisplayTimeZone, type DisplayTimeZone } from "./display-time"
import { DisplayTimeProvider, useDisplayTime } from "./display-time-context"
import { contextualRows, entityContext, findingRoute } from "./entity-context"
import { EventsView, type FindingResolution } from "./events-view"
import { findingHistory, findingHistoryRequest, findingProjection } from "./finding-presentation"
import { HelpPanel, type Translate } from "./help"
import { HourPicker } from "./hour-picker"
import { keyboardTargetOwnsArrows } from "./keyboard"
import { rowMatchesLocator } from "./locator"
import { Login } from "./login"
import {
  activityFor,
  asNumber,
  floorHour,
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
import { PLAN_INFO_REQUEST, planRequest, statementRequest, type PlanLens, type StatementLens } from "./postgres-metrics"
import { isRelationLens, relationRequest, type RelationGroup, type RelationLens, type RelationNavigation, type RelationSection } from "./postgres-relations"
import { ProcessSummary, ProcessTable } from "./process-table"
import { latestTimelineTimestamp, refreshedCursor, scheduleRefresh } from "./refresh"
import type { ChartPoint } from "./series-chart"
import { bootstrapSession, getSessionSnapshot, logout, subscribeSession } from "./session"
import { SYSTEM_REQUESTS, SystemView } from "./system-view"
import { Timeline } from "./timeline"

type Source = "host" | "postgresql" | "events"
type Theme = "dark" | "light"
type HostSection = "system" | "processes"

const EMPTY_DATA: HourData = {
  sections: {}, rateColumns: {}, snapshotRows: [], availableSections: [], postgresqlConfigured: false, processes: [], activities: [], load: [], memory: [], pressure: [], health: [],
  pgOverview: [], points: [], lanePoints: [], findings: [], findingGroups: [],
}

const VIEW_REQUESTS: Readonly<Record<string, readonly SectionRequest[]>> = {
  system: [...TIMELINE_REQUESTS, ...SYSTEM_REQUESTS],
  processes: [...TIMELINE_REQUESTS, { section: "os_process" }, { section: "pg_stat_activity" }, { section: "instance_metadata" }],
  "postgresql:overview": [...TIMELINE_REQUESTS, ...POSTGRESQL_OVERVIEW_REQUESTS],
  "postgresql:activity": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlActivity.map(section)],
  "postgresql:locks": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlLocks.map(section)],
  "postgresql:databases": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlDatabases.map(section)],
  events: TIMELINE_REQUESTS,
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

function Kronika() {
  const session = useSyncExternalStore(subscribeSession, getSessionSnapshot, getSessionSnapshot)
  const [locale, setLocale] = useState<Locale>(initialLocale)
  const [displayZone, setDisplayZone] = useState<DisplayTimeZone>(() => loadDisplayTimeZone(localStorage))
  const t = useMemo<Translate>(() => (key, slots = {}) => {
    const template = translation(locale, key) ?? key
    return interpolate(template, slots)
  }, [locale])
  useEffect(() => {
    document.documentElement.lang = locale
    try { localStorage.setItem("kronika.locale", locale) } catch {}
  }, [locale])
  useEffect(() => saveDisplayTimeZone(localStorage, displayZone), [displayZone])
  if (session === "pending") return null
  return session === "signed-in"
    ? <DisplayTimeProvider locale={locale} mode={displayZone} setMode={setDisplayZone}><App locale={locale} onLocale={setLocale} t={t} /></DisplayTimeProvider>
    : <Login expired={session === "expired"} locale={locale} onLocale={setLocale} t={t} />
}

function App({ locale, onLocale, t }: {
  readonly locale: Locale
  readonly onLocale: (locale: Locale) => void
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const opened = useRef(readAddress(window.location.search))
  const [theme, setTheme] = useState<Theme>(initialTheme)
  const [hour, setHour] = useState<number | null>(opened.current.at === null ? null : floorHour(opened.current.at))
  const wanted = useRef(opened.current.at)
  const [availableHours, setAvailableHours] = useState<readonly number[]>([])
  const [cursor, setCursor] = useState(0)
  const followsLatest = useRef(opened.current.at === null)
  const [timelineData, setTimelineData] = useState<HourData>(EMPTY_DATA)
  const [currentData, setCurrentData] = useState<HourData>(EMPTY_DATA)
  const data = useMemo(() => viewData(timelineData, currentData), [currentData, timelineData])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [source, setSource] = useState<Source>(sourceOf(opened.current.view))
  const [hostSection, setHostSection] = useState<HostSection>(hostSectionOf(opened.current.view))
  const [pgSection, setPgSection] = useState<PostgresSection>(pgSectionOf(opened.current.view))
  const [statementLens, setStatementLens] = useState<StatementLens>(statementLensOf(opened.current.pgLens))
  const [planLens, setPlanLens] = useState<PlanLens>(planLensOf(opened.current.pgLens))
  const [relationLens, setRelationLens] = useState<RelationLens>(relationLensOf(pgSectionOf(opened.current.view), opened.current.pgLens))
  const [relationLevel, setRelationLevel] = useState<RelationGroup>(opened.current.pgLevel)
  const [relationFilters, setRelationFilters] = useState<Readonly<Record<string, string>>>(() => relationFiltersOf(opened.current))
  const [relationSelectedKey, setRelationSelectedKey] = useState<string | null>(() => relationSelectedKeyOf(opened.current))
  const activeRelationLens = relationLensOf(pgSection, relationLens)
  const relationSection = pgSection === "tables" || pgSection === "indexes"
  const activeRelation = source === "postgresql" && relationSection
  const [lens, setLens] = useState<Lens>(opened.current.lens)
  const [find, setFind] = useState(opened.current.find)
  const [order, setOrder] = useState<TableOrder | null>(opened.current.sort)
  const [selectedKey, setSelectedKey] = useState<string | null>(opened.current.row)
  const [selectedFinding, setSelectedFinding] = useState<Finding | null>(null)
  const [eventScope, setEventScope] = useState<readonly Finding[] | null>(null)
  const [findingRow, setFindingRow] = useState<DataRow | null>(null)
  const [findingResolution, setFindingResolution] = useState<FindingResolution>("idle")
  const [findingPoints, setFindingPoints] = useState<readonly ChartPoint[]>([])
  const [systemFocus, setSystemFocus] = useState<Finding | null>(null)
  const context = useMemo(() => selectedFinding === null ? null : entityContext(selectedFinding, findingRow, t), [findingRow, selectedFinding, t])
  const clearEntityContext = useCallback(() => {
    setSelectedFinding(null)
    setEventScope(null)
    setFindingRow(null)
    setFindingResolution("idle")
    setFindingPoints([])
    setSystemFocus(null)
  }, [])
  const [helpOpen, setHelpOpen] = useState(false)
  useEffect(() => {
    document.documentElement.dataset.theme = theme
    try { localStorage.setItem("kronika.theme", theme) } catch {}
  }, [theme])
  const baseViewKey = source === "host" ? hostSection : source === "postgresql" ? `postgresql:${pgSection}` : "events"
  const viewKey = pgSection === "statements" && source === "postgresql"
    ? `${baseViewKey}:${statementLens}`
    : pgSection === "plans" && source === "postgresql" ? `${baseViewKey}:${planLens}` : baseViewKey
  const viewRequests = useMemo(() => {
    if (source === "postgresql" && pgSection === "statements") return [...TIMELINE_REQUESTS, statementRequest(statementLens)]
    if (source === "postgresql" && pgSection === "plans") return [
      ...TIMELINE_REQUESTS,
      planRequest(planLens),
      PLAN_INFO_REQUEST,
    ]
    if (activeRelation) {
      return [...TIMELINE_REQUESTS, relationRequest(relationSectionOf(pgSection), activeRelationLens, relationLevel)]
    }
    return VIEW_REQUESTS[baseViewKey] ?? []
  }, [activeRelation, activeRelationLens, baseViewKey, pgSection, planLens, relationLevel, source, statementLens])
  const [segments, setSegments] = useState<readonly SegmentBound[]>([])
  const drawn = useRef<number | null>(null)
  const selectedHour = useRef(hour)
  selectedHour.current = hour
  const refreshRequested = useRef(false)
  const [refreshVersion, setRefreshVersion] = useState(0)
  const [refreshing, setRefreshing] = useState(false)
  const [refreshFailed, setRefreshFailed] = useState(false)
  const [lastUpdated, setLastUpdated] = useState<number | null>(null)
  const refreshAwaitingSnapshot = useRef(false)
  const segmentsRef = useRef(segments)
  segmentsRef.current = segments
  const pendingRefresh = useRef<{ readonly timeline: TimelineData; readonly previousSegments: readonly SegmentBound[] } | null>(null)
  const finishRefresh = useCallback((succeeded: boolean) => {
    if (!refreshRequested.current) return
    const pending = pendingRefresh.current
    if (succeeded && pending !== null) {
      drawn.current = pending.timeline.hour
      setAvailableHours(pending.timeline.availableHours)
      setSegments(pending.timeline.segments)
      setTimelineData(hourOf(pending.timeline))
    } else if (pending !== null) {
      setSegments(pending.previousSegments)
    }
    pendingRefresh.current = null
    refreshRequested.current = false
    refreshAwaitingSnapshot.current = false
    setRefreshing(false)
    setRefreshFailed(!succeeded)
    if (succeeded) setLastUpdated(Date.now() * 1_000)
  }, [])
  const beginRefresh = useCallback(() => {
    if (refreshRequested.current || drawn.current === null || drawn.current !== selectedHour.current) return
    refreshRequested.current = true
    setRefreshing(true)
    setRefreshVersion((current) => current + 1)
  }, [])
  const requestRefresh = beginRefresh
  const chooseCursor = useCallback((next: number) => {
    followsLatest.current = false
    setCursor(next)
  }, [])
  const previousView = useRef(baseViewKey)
  useEffect(() => {
    if (previousView.current === baseViewKey) return
    previousView.current = baseViewKey
    setOrder(null)
  }, [baseViewKey])
  useEffect(() => {
    if (hour === null) return
    clearEntityContext()
  }, [clearEntityContext, hour])
  useEffect(() => {
    const refresh = refreshRequested.current && hour !== null && drawn.current === hour
    if (!refresh) finishRefresh(false)
    if (hour !== null && drawn.current === hour && !refresh) return
    const controller = new AbortController()
    setRefreshFailed(false)
    if (!refresh) {
      setLoading(true)
      setRefreshing(false)
      setError(null)
    }
    void loadTimeline(hour, controller.signal).then((timeline) => {
      const asked = wanted.current
      wanted.current = null
      const latest = latestTimelineTimestamp(timeline)
      if (refresh) {
        pendingRefresh.current = { timeline, previousSegments: segmentsRef.current }
        const next = refreshedCursor(cursor, followsLatest.current, timeline)
        if (next !== cursor) {
          setSegments(timeline.segments)
          setCursor(next)
          refreshAwaitingSnapshot.current = true
        } else {
          finishRefresh(true)
        }
      } else {
        drawn.current = timeline.hour
        setAvailableHours(timeline.availableHours)
        setHour(timeline.hour)
        setSegments(timeline.segments)
        setTimelineData(hourOf(timeline))
        setCurrentData(EMPTY_DATA)
        followsLatest.current = asked === null || floorHour(asked) !== timeline.hour
        setCursor(followsLatest.current ? latest : asked ?? latest)
        setLastUpdated(Date.now() * 1_000)
        setRefreshing(false)
      }
      setLoading(false)
    }).catch((reason: unknown) => {
      if (controller.signal.aborted) return
      if (refresh) {
        console.error("kronika: refresh failed", reason)
        finishRefresh(false)
        return
      }
      const fallback = hour ?? floorHour(Date.now() * 1_000)
      drawn.current = fallback
      setHour(fallback)
      setSegments([])
      setTimelineData(EMPTY_DATA)
      setCurrentData(EMPTY_DATA)
      setCursor(fallback)
      setError(reason instanceof Error ? reason.message : String(reason))
      setLoading(false)
    })
    return () => controller.abort()
  }, [finishRefresh, hour, refreshVersion])
  const cursorSegment = useMemo(() => segmentBoundAt(segments, cursor), [cursor, segments])
  const cursorSegmentId = cursorSegment?.id ?? null
  const [cursorState, setCursorState] = useState<"ready" | "loading" | "missing">("ready")
  const [densePageState, setDensePageState] = useState<"idle" | "loading" | "error">("idle")
  const snapshotGeneration = useRef(0)
  const densePage = useRef<{
    failed: string | undefined
    load: (cursor?: string) => void
  } | null>(null)
  const densePattern = viewRequests.some((request) => request.pageSize !== undefined) ? find.trim() : ""
  useEffect(() => {
    const generation = ++snapshotGeneration.current
    const completesRefresh = refreshAwaitingSnapshot.current
    densePage.current = null
    if (hour === null || cursorSegment === null) {
      if (!completesRefresh) setCurrentData(EMPTY_DATA)
      setCursorState("missing")
      setDensePageState("idle")
      if (completesRefresh) finishRefresh(false)
      return
    }
    const wanted = requestsForSegment(
      viewRequests.filter((request) => request.section !== "health").map((request) => request.pageSize !== undefined && request.section === context?.logicalName
        ? { ...request, typeIds: [context.typeId] } : request),
      cursorSegment,
    )
    if (wanted.length === 0) {
      if (!completesRefresh) setCurrentData(EMPTY_DATA)
      setCursorState("ready")
      setDensePageState("idle")
      if (completesRefresh) finishRefresh(true)
      return
    }
    setCursorState("loading")
    setDensePageState(wanted.some((request) => request.pageSize !== undefined) ? "loading" : "idle")
    const controller = new AbortController()
    const stale = () => controller.signal.aborted || generation !== snapshotGeneration.current
    const timer = setTimeout(() => {
      const request = wanted.find((request) => request.pageSize !== undefined)
      const ordinary = wanted.filter((request) => request.pageSize === undefined)
      if (request === undefined) {
        void loadSnapshot(cursorSegment.id, cursor, ordinary, controller.signal, order ?? undefined)
        .then((incoming) => {
          if (stale()) return
          setCurrentData(incoming)
          setCursorState("ready")
          if (completesRefresh) finishRefresh(true)
        })
        .catch((reason: unknown) => {
          if (stale()) return
          setCursorState("missing")
          if (completesRefresh) finishRefresh(false)
          console.error("kronika: snapshot at the cursor failed", reason)
        })
        return
      }
      const base = ordinary.length === 0
        ? Promise.resolve(EMPTY_DATA)
        : loadSnapshot(cursorSegment.id, cursor, ordinary, controller.signal, order ?? undefined)
          .catch((reason: unknown) => {
            if (completesRefresh) throw reason
            if (!controller.signal.aborted) console.error("kronika: snapshot companion failed", reason)
            return EMPTY_DATA
          })
      let inFlight = false
      const pageContext = context?.logicalName === request.section ? context : null
      const action = {
        failed: undefined as string | undefined,
        load: (pageCursor?: string) => {
          if (inFlight || stale()) return
          inFlight = true
          action.failed = undefined
          setDensePageState("loading")
          const relation = request.group === undefined ? undefined : relationFilters
          const fixed = request.group === undefined ? undefined : request.filters
          const options = {
            ...(pageCursor === undefined ? {} : { cursor: pageCursor }),
            ...(densePattern === "" ? {} : { search: [densePattern] }),
            ...(relation === undefined
              ? pageContext === null ? {} : { filters: Object.fromEntries(pageContext.identity), typeId: pageContext.typeId }
              : Object.keys(relation).length === 0 && fixed === undefined ? {} : { filters: { ...relation, ...fixed } }),
          }
          void Promise.all([
            pageCursor === undefined ? base : Promise.resolve(null),
            loadSnapshot(cursorSegment.id, cursor, [request], controller.signal, order ?? undefined, options),
          ]).then(([companion, incoming]) => {
            if (stale()) return
            setCurrentData((current) => pageCursor === undefined
              ? mergeSnapshotData(companion ?? EMPTY_DATA, incoming)
              : mergeSnapshotData(current, incoming, request.section))
            setDensePageState("idle")
            setCursorState("ready")
            if (completesRefresh && pageCursor === undefined) finishRefresh(true)
          }).catch((reason: unknown) => {
            if (stale()) return
            action.failed = pageCursor
            setDensePageState("error")
            setCursorState("ready")
            if (completesRefresh && pageCursor === undefined) finishRefresh(false)
            console.error("kronika: snapshot page failed", reason)
          }).finally(() => { inFlight = false })
        },
      }
      densePage.current = action
      action.load()
    }, 250)
    return () => { clearTimeout(timer); controller.abort() }
  }, [context, cursor, cursorSegmentId, densePattern, finishRefresh, hour, order, relationFilters, viewRequests])
  const refreshReady = !loading && cursorState === "ready" && densePageState !== "loading"
  useEffect(() => hour === null || !refreshReady || refreshing
    ? undefined : scheduleRefresh(hour, requestRefresh), [hour, refreshReady, refreshing, requestRefresh])
  const denseMetadata = currentData.snapshotRows[0]
  const loadMoreDense = useCallback(() => {
    const next = denseMetadata?.hasMore === true ? denseMetadata.nextCursor : null
    if (next != null) densePage.current?.load(next)
  }, [denseMetadata])
  const retryDense = useCallback(() => {
    const action = densePage.current
    if (action !== null) action.load(action.failed)
  }, [])

  useEffect(() => {
    const shortcuts = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return
      if (keyboardTargetOwnsArrows(event.target)) return
      if (event.key === "?") setHelpOpen((current) => !current)
      if (event.key === "Escape") setHelpOpen(false)
    }
    window.addEventListener("keydown", shortcuts)
    return () => window.removeEventListener("keydown", shortcuts)
  }, [])

  const shownAt = useMemo(() => shownMoment(data.sections, cursor), [cursor, data.sections])
  const contextRow = selectedFinding?.timestamp === cursor ? findingRow : null
  const allProcessRows = useMemo(() => snapshot(data.processes, cursor), [cursor, data.processes])
  const processRows = useMemo(() => contextualRows(allProcessRows, context?.logicalName === "os_process" ? context : null, contextRow), [allProcessRows, context, contextRow])
  const ticksPerSecond = useMemo(() => {
    const metadata = (data.sections.instance_metadata ?? [])[0]
    return metadata === undefined ? null : asNumber(value(metadata, "clock_ticks_per_sec"))
  }, [data.sections])
  const pgRows = useMemo(() => snapshot(data.activities, cursor), [cursor, data.activities])
  const linkedPids = useMemo(() => new Set(pgRows.flatMap((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid === null ? [] : [pid]
  })), [pgRows])
  const selectedProcess = processRows.find((row) => processKey(row) === selectedKey) ?? null
  useEffect(() => {
    if (selectedFinding?.logicalName === "os_process" && findingRow !== null) setSelectedKey(processKey(findingRow))
  }, [findingRow, selectedFinding])
  useEffect(() => {
    if (selectedFinding === null) {
      setFindingRow(null)
      setFindingResolution("idle")
      return
    }
    if (findingRow !== null && rowMatchesLocator(findingRow, selectedFinding)) {
      setFindingResolution("ready")
      return
    }
    const loaded = resolveLocator(data, selectedFinding)?.row ?? null
    if (loaded !== null) {
      setFindingRow(loaded)
      setFindingResolution("ready")
      return
    }
    const fields = findingProjection(selectedFinding)
    if (selectedFinding.typeId === "0" || fields.length === 0) {
      setFindingRow(null)
      setFindingResolution("unavailable")
      return
    }
    setFindingRow(null)
    setFindingResolution("loading")
    const controller = new AbortController()
    void loadSnapshot(
      selectedFinding.segmentId,
      selectedFinding.timestamp,
      [{ section: selectedFinding.logicalName, fields, typeId: selectedFinding.typeId }],
      controller.signal,
      undefined,
      { typeId: selectedFinding.typeId, rowOrdinal: selectedFinding.rowOrdinal, fullText: true },
    ).then((incoming) => {
      if (controller.signal.aborted) return
      const row = incoming.sections[selectedFinding.logicalName]?.[0] ?? null
      setFindingRow(row)
      setFindingResolution(row === null ? "unavailable" : "ready")
    }).catch(() => {
      if (!controller.signal.aborted) setFindingResolution("unavailable")
    })
    return () => controller.abort()
  }, [data, findingRow, selectedFinding])
  useEffect(() => {
    setFindingPoints([])
    if (selectedFinding === null || findingRow === null || hour === null) return
    const request = findingHistoryRequest(selectedFinding, findingRow)
    if (request === null) {
      setFindingPoints(findingHistory(selectedFinding, [findingRow], data))
      return
    }
    const controller = new AbortController()
    void loadSeries(hour, selectedFinding.logicalName, request.where, request.fields, controller.signal, selectedFinding.typeId, selectedFinding.timestamp)
      .then((rows) => setFindingPoints(findingHistory(selectedFinding, rows, data)))
      .catch(() => { if (!controller.signal.aborted) setFindingPoints([]) })
    return () => controller.abort()
  }, [data, findingRow, hour, selectedFinding])
  const pgFocus = selectedFinding !== null && selectedFinding.logicalName.startsWith("pg_") ? contextRow : null
  const joinedActivity = activityFor(selectedProcess, data.activities, selectedProcess?.timestamp ?? cursor)
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
      .catch(() => {})
    return () => controller.abort()
  }, [hour, selectedPid, selectedStart])
  const address = useMemo(() => writeAddress({
    at: cursor === 0 ? null : cursor,
    view: viewOf(source, hostSection, pgSection),
    lens,
    pgLens: activeRelation
      ? activeRelationLens : source === "postgresql" && pgSection === "plans" ? planLens : statementLens,
    pgLevel: relationLevel,
    datid: relationFilters.datid ?? null,
    schema: relationFilters.schemaname ?? null,
    relid: relationFilters.relid ?? null,
    indexrelid: relationFilters.indexrelid ?? null,
    sort: order,
    row: activeRelation
      ? relationSelectedKey
      : selectedKey,
    find,
  }), [activeRelation, activeRelationLens, cursor, find, hostSection, lens, order, pgSection, planLens, relationFilters, relationLevel, relationSelectedKey, selectedKey, source, statementLens])
  const steps = useRef<string | null>(null)
  useEffect(() => {
    if (window.location.pathname + window.location.search === address) return
    const dragging = steps.current !== null && stepOf(steps.current) === stepOf(address)
    steps.current = address
    window["history"][dragging ? "replaceState" : "pushState"]({}, "", address)
  }, [address])
  useEffect(() => {
    const back = () => {
      const opening = readAddress(window.location.search)
      setSource(sourceOf(opening.view))
      setHostSection(hostSectionOf(opening.view))
      setPgSection(pgSectionOf(opening.view))
      setLens(opening.lens)
      setStatementLens(statementLensOf(opening.pgLens))
      setPlanLens(planLensOf(opening.pgLens))
      const openingSection = pgSectionOf(opening.view)
      setRelationLens(relationLensOf(openingSection, opening.pgLens))
      setRelationLevel(opening.pgLevel)
      setRelationFilters(relationFiltersOf(opening))
      setRelationSelectedKey(relationSelectedKeyOf(opening))
      setOrder(opening.sort)
      setSelectedKey(opening.row)
      setFind(opening.find)
      clearEntityContext()
      if (opening.at !== null) {
        followsLatest.current = false
        wanted.current = opening.at
        setCursor(opening.at)
        setHour(floorHour(opening.at))
      }
    }
    window.addEventListener("popstate", back)
    return () => window.removeEventListener("popstate", back)
  }, [clearEntityContext])
  const navigateRelation = useCallback((navigation: RelationNavigation) => {
    const nextSection = navigation.section === "pg_stat_user_tables" ? "tables" : "indexes"
    const crossing = nextSection !== pgSection
    const nextLens = relationLensOf(nextSection, activeRelationLens)
    setPgSection(nextSection)
    setRelationLens(nextLens)
    setRelationLevel(navigation.group)
    setRelationFilters(navigation.filters)
    setRelationSelectedKey(navigation.selectedKey)
    if (crossing) setFind("")
    setOrder((current) => current !== null && !crossing && Object.hasOwn(relationRequest(navigation.section, nextLens, navigation.group).order ?? {}, current.column) ? current : null)
  }, [activeRelationLens, pgSection])
  const choosePgSection = useCallback((next: PostgresSection) => {
    const crossing = next !== pgSection
    setRelationSelectedKey(null)
    setPgSection(next)
    if (next !== "tables" && next !== "indexes") return
    setRelationLens((current) => relationLensOf(next, current))
    setRelationFilters((current) => relationFiltersForSection(current, next))
    if (crossing && relationSection) setFind("")
  }, [pgSection, relationSection])
  const chooseRelationLens = useCallback((next: RelationLens) => {
    if (next !== relationLens) setOrder(null)
    setRelationLens(next)
  }, [relationLens])
  const changeHour = useCallback((next: number) => {
    followsLatest.current = true
    setHour(floorHour(next))
  }, [])
  const selectProcess = useCallback((row: DataRow) => {
    setSelectedKey(processKey(row))
  }, [])
  const selectFinding = useCallback((finding: Finding, grouped: readonly Finding[] = [finding]) => {
    followsLatest.current = false
    setCursor(finding.timestamp)
    setFindingRow(null)
    setFindingResolution("loading")
    setFindingPoints([])
    if (grouped.length > 1) {
      setSelectedFinding(null)
      setFindingResolution("idle")
      setEventScope(grouped)
      setSource("events")
      return
    }
    setSelectedFinding(finding)
    setEventScope(null)
    setOrder(null)
    const resolved = resolveLocator(data, finding)
    if (resolved !== null) {
      setFindingRow(resolved.row)
      setFindingResolution("ready")
    }
    const route = findingRoute(finding)
    if (route === "processes") {
      setSource("host")
      setHostSection("processes")
      setLens(processLens(fieldNameForLocator(finding)))
      setSystemFocus(null)
      if (resolved !== null) setSelectedKey(processKey(resolved.row))
      return
    }
    if (route === "system") {
      setSource("host")
      setHostSection("system")
      setSystemFocus(finding)
      return
    }
    if (route !== "events") {
      setSource("postgresql")
      setPgSection(route)
      if (route === "statements") setStatementLens("load")
      if (route === "plans") setPlanLens("load")
      setSystemFocus(null)
      setFindingRow(resolved?.row ?? null)
      return
    }
    setSource("events")
  }, [data])
  const pgPresent = data.postgresqlConfigured === true || data.activities.length !== 0 || data.availableSections.some((name) => name.startsWith("pg_") && !name.startsWith("pg_log_"))
  const eventsPresent = data.findings.length !== 0
  const helpItems = source === "postgresql"
    ? HELP_POSTGRESQL
    : source === "events"
      ? HELP_EVENTS
      : hostSection === "processes" ? HELP_PROCESS : HELP_SYSTEM
  useEffect(() => {
    if (loading) return
    if (source === "postgresql" && !pgPresent) setSource("host")
    if (source === "events" && !eventsPresent) setSource("host")
  }, [eventsPresent, loading, pgPresent, source])

  const stretchPostgres = source === "postgresql" && (relationSection || pgSection === "statements" || pgSection === "plans")
  const cursorTime = cursor === 0 ? null : time.clock(cursor)
  const updatedTime = lastUpdated === null ? null : time.clock(lastUpdated)
  const zoneReference = cursor || hour || Date.now() * 1_000
  return <main className={`app-shell${stretchPostgres ? " pg-table-shell" : ""}`}>
    <header className="topbar">
      <span className="brand-mark"><Activity aria-hidden="true" size={15} strokeWidth={2} /></span>
      <h1>{t("app.title")}</h1>

      <nav aria-label={t("nav.sources")} className="source-tabs">
        <button aria-current={source === "host" ? "page" : undefined} className={source === "host" ? "source-active" : undefined} onClick={() => setSource("host")} type="button">{t("nav.host")}</button>
        <button aria-current={source === "postgresql" ? "page" : undefined} className={source === "postgresql" ? "source-active" : undefined} disabled={!pgPresent} onClick={() => setSource("postgresql")} title={pgPresent ? undefined : t("nav.no_data")} type="button">{t("nav.postgresql")}</button>
        {eventsPresent && <button aria-current={source === "events" ? "page" : undefined} className={source === "events" ? "source-active" : undefined} onClick={() => { setEventScope(null); setSelectedFinding(null); setSource("events") }} type="button">{t("nav.events")}</button>}
      </nav>

      {source === "host" && <div className="section-tabs" role="tablist">
        <button aria-selected={hostSection === "system"} onClick={() => setHostSection("system")} role="tab" type="button">{t("section.system")}</button>
        <button aria-selected={hostSection === "processes"} data-testid="process-tab" onClick={() => setHostSection("processes")} role="tab" type="button">{t("section.processes")}</button>
      </div>}

      <HourPicker availableHours={availableHours} changeHour={changeHour} hour={hour} locale={locale} t={t} />
      <div aria-live="polite" className="cursor-time">
        <TimeValue label={t("hour.cursor_label")} output={cursorTime} testId="cursor-time" />
        {updatedTime !== null && <TimeValue label={t("refresh.updated")} output={updatedTime} testId="updated-time" />}
        {refreshFailed && <span>{t("refresh.error")}</span>}
        {cursorState !== "ready" && <span className={cursorState === "loading" ? "cursor-behind" : "cursor-missing"} data-testid="cursor-behind">{t(cursorState === "loading" ? "status.updating" : "status.no_sample")}</span>}
      </div>

      <div className="top-actions">
        <button aria-label={t("refresh.action")} className="icon-button" disabled={refreshing || !refreshReady} onClick={requestRefresh} title={t("refresh.action")} type="button">↻</button>
        <select aria-label={t("timezone.switch")} className="timezone-select" data-testid="timezone-select" onChange={(event) => time.setMode(event.currentTarget.value as DisplayTimeZone)} value={time.mode}>
          <option value="browser">{t("timezone.browser", { zone: time.browserOffset(zoneReference) })}</option>
          <option value="utc">{t("timezone.utc")}</option>
        </select>
        <button aria-label={t("common.theme.switch")} className="icon-button" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} title={t(theme === "dark" ? "common.theme.light" : "common.theme.dark")} type="button">
          {theme === "dark" ? "☀" : "☾"}
        </button>
        <div aria-label={t("locale.switch")} className="locale-switch" role="group">
          {(["ru", "en"] as const).map((choice) => <button aria-pressed={locale === choice} data-testid={`locale-${choice}`} key={choice} onClick={() => onLocale(choice)} type="button">{t(`locale.${choice}`)}</button>)}
        </div>
        <button aria-label={t("auth.logout")} className="icon-button" onClick={logout} title={t("auth.logout")} type="button">×</button>
        <button aria-expanded={helpOpen} aria-label={t("help.open")} className="icon-button" data-testid="help-trigger" onClick={() => setHelpOpen((current) => !current)} type="button">?</button>
      </div>
    </header>

    <section className={`${cursorState === "loading" ? "workspace workspace-behind" : "workspace"}${stretchPostgres ? " pg-table-workspace" : ""}`}>
      <p aria-live="polite" className="live-note">
        {t(`nav.${source}`)}
        {source === "host" ? ` · ${t(`section.${hostSection}`)}` : ""}
        {source === "postgresql" ? ` · ${t(`pg.section.${pgSection}`)}` : ""}
      </p>
      {loading && <StateCard message={t("status.loading")} />}
      {!loading && error !== null && <StateCard message={t("status.error")} />}
      {!loading && error === null && hour !== null && source === "host" && hostSection === "system" && <SystemView context={context} contextRow={contextRow} cursor={cursor} data={data} focus={systemFocus} hour={hour} locale={locale} onContextClear={clearEntityContext} onCursor={chooseCursor} onFinding={selectFinding} t={t} />}
      {!loading && error === null && hour !== null && source === "host" && hostSection === "processes" && <>
        <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} onCursor={chooseCursor} onFinding={selectFinding} primaryLane={lens === "cpu" ? "cpu_busy" : lens === "memory" ? "memory" : lens === "disk" ? "io_stall" : "health"} shownAt={shownAt} t={t} />
        <div className="lensbar">
          <div aria-label={t("section.processes")} className="lens-tabs" role="group">
            {(["cpu", "memory", "disk", "generic"] as const).map((choice) => <button aria-pressed={lens === choice} data-testid={`lens-${choice}`} key={choice} onClick={() => { if (choice !== lens) setOrder(null); setLens(choice) }} type="button">{t(`lens.${choice}`)}</button>)}
          </div>
          <span>{processRows[0] === undefined ? t("status.no_data") : time.timestamp(processRows[0].timestamp)}</span>
        </div>
        <ProcessSummary lens={lens} linkedPids={linkedPids} locale={locale} rows={processRows} t={t} ticksPerSecond={ticksPerSecond} />
        <div className={selectedProcess === null ? "process-layout process-layout-table" : "process-layout"}>
          <ProcessTable contextLabel={context?.logicalName === "os_process" ? context.label : undefined} finding={selectedFinding?.logicalName === "os_process" ? selectedFinding : null} findingField={selectedFinding?.logicalName === "os_process" ? fieldNameForLocator(selectedFinding) : null} lens={lens} linkedPids={linkedPids} locale={locale} onContextClear={clearEntityContext} onOrder={setOrder} onPattern={setFind} onSelect={selectProcess} order={order} pattern={find} rows={processRows} selectedKey={selectedKey} t={t} ticksPerSecond={ticksPerSecond} />
          {selectedProcess !== null && <DetailDock activity={joinedActivity.row} activityTime={joinedActivity.snapshotTime} cursor={cursor} hour={hour} lens={lens} locale={locale} onClose={() => setSelectedKey(null)} onCursor={chooseCursor} process={selectedProcess} processHistory={processHistory} t={t} ticksPerSecond={ticksPerSecond} />}
        </div>
      </>}
      {!loading && error === null && hour !== null && source === "postgresql" && <PostgresView context={context} densePageState={densePageState} onContextClear={clearEntityContext} onLoadMore={loadMoreDense} onRetry={retryDense} onOrder={setOrder} onPattern={setFind} order={order ?? undefined} pattern={find} cursor={cursor} data={data} focus={pgFocus} focusFinding={selectedFinding} hour={hour} locale={locale} onCursor={chooseCursor} onFinding={selectFinding} onPlanLens={(next) => { setOrder(null); setPlanLens(next) }} onRelationLens={chooseRelationLens} onRelationNavigate={navigateRelation} onRelationSelectedKey={setRelationSelectedKey} onSection={choosePgSection} onStatementLens={(next) => { setOrder(null); setStatementLens(next) }} planLens={planLens} relationFilters={relationFilters} relationLens={activeRelationLens} relationLevel={relationLevel} relationSelectedKey={relationSelectedKey} section={pgSection} statementLens={statementLens} t={t} />}
      {!loading && error === null && hour !== null && source === "events" && <EventsView cursor={cursor} data={data} history={findingPoints} hour={hour} locale={locale} onCursor={chooseCursor} onFinding={selectFinding} onShowAll={() => { setEventScope(null); setSelectedFinding(null) }} resolution={findingResolution} resolved={findingRow} scope={eventScope} selected={selectedFinding} t={t} />}
    </section>

    {helpOpen && <HelpPanel items={helpItems} onClose={() => setHelpOpen(false)} t={t} />}
  </main>
}

function TimeValue({ label, output, testId }: { readonly label: string; readonly output: string | null; readonly testId: string }) {
  return <span data-testid={testId}><b>{label}</b>{output ?? "—"}</span>
}

function StateCard({ message }: { readonly message: string }) {
  return <div className="loading-card"><p className="eyebrow">KRONIKA</p><h2>{message}</h2></div>
}

function statementLensOf(lens: PgLens): StatementLens {
  return lens === "per_call" || lens === "io" || lens === "resources" || lens === "stability" ? lens : "load"
}

function planLensOf(lens: PgLens): PlanLens {
  return lens === "timing" || lens === "io" || lens === "identity" ? lens : "load"
}

function relationSectionOf(section: PostgresSection): RelationSection {
  return section === "indexes" ? "pg_stat_user_indexes" : "pg_stat_user_tables"
}

function relationLensOf(section: PostgresSection, lens: PgLens): RelationLens {
  const relation = relationSectionOf(section)
  return isRelationLens(relation, lens) ? lens : relation === "pg_stat_user_tables" ? "access" : "usage"
}

function relationFiltersOf(address: ReturnType<typeof readAddress>): Readonly<Record<string, string>> {
  return Object.fromEntries([
    ["datid", address.datid],
    ["schemaname", address.schema],
    ["relid", address.relid],
    ["indexrelid", address.indexrelid],
  ].flatMap(([name, stored]) => stored === null ? [] : [[name, stored]]))
}

function relationSelectedKeyOf(address: ReturnType<typeof readAddress>): string | null {
  const section = pgSectionOf(address.view)
  return section === "tables" || section === "indexes" ? address.row : null
}

function relationFiltersForSection(filters: Readonly<Record<string, string>>, section: "tables" | "indexes"): Readonly<Record<string, string>> {
  if (section === "indexes") return filters
  const { indexrelid: _, ...tableFilters } = filters
  return tableFilters
}

function initialTheme(): Theme {
  try {
    const saved = localStorage.getItem("kronika.theme")
    if (saved === "dark" || saved === "light") return saved
  } catch {}
  return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark"
}

function initialLocale(): Locale {
  try {
    const saved = localStorage.getItem("kronika.locale")
    if (saved === "ru" || saved === "en") return saved
  } catch {}
  for (const language of navigator.languages) {
    if (language.toLowerCase().startsWith("ru")) return "ru"
    if (language.toLowerCase().startsWith("en")) return "en"
  }
  return "en"
}

const root = document.getElementById("root")
if (root === null) throw new Error("Kronika UI root is missing")
void bootstrapSession().then(() => createRoot(root).render(<Kronika />))
