import { Activity, ChartLine, CircleHelp, LogOut, Moon, Plug, RotateCw, Sun } from "lucide-react"
import { translation } from "kronika:i18n"
import { ProcessesActivity } from "./activity"
import { useCallback, useEffect, useMemo, useReducer, useRef, useState, useSyncExternalStore } from "react"
import { createRoot } from "react-dom/client"

import {
  TIMELINE_REQUESTS,
  acceptResponse,
  loadTimeline,
  loadTimelineLanes,
  loadSeries,
  hourOf,
  loadSnapshot,
  loadSnapshotGroups,
  mergeSnapshotData,
  snapshotRequestGroups,
  fieldNameForLocator,
  viewData,
  withTimelineLanes,
  resolveLocator,
  PRODUCT_SECTION_GROUPS,
  POSTGRESQL_CONTEXT_REQUESTS,
  type DataRow,
  type SectionRequest,
  type SegmentBound,
  type Finding,
  type HourData,
  type SnapshotOptions,
  type SnapshotRequestGroup,
  type TimelineData,
} from "./api"
import type { TableOrder } from "./entity-table"
import { pgSectionOf, readAddress, sourceOf, stepOf, viewOf, writeAddress, type InspectorPanel, type PgLens, type Source } from "./address"
import { ActivityFacts } from "./detail-activity"
import { DetailDock, PROCESS_HISTORY_FIELDS } from "./detail"
import { loadDisplayTimeZone, saveDisplayTimeZone, type DisplayTimeZone } from "./display-time"
import { DisplayTimeProvider, DisplayTimeScope, useDisplayTime } from "./display-time-context"
import { contextualRows, entityContext, findingRoute, type EntityContext } from "./entity-context"
import { mergeObservationTimestamps, observationTimestamps } from "./cursor-timestamps"
import { EventsView } from "./events-view"
import { findingProjection } from "./finding-presentation"
import { HelpPanel, type Translate, LabelHelp } from "./help"
import { McpPanel } from "./mcp-connect"
import { MobileControls } from "./mobile-controls"
import { useHistoryRequest } from "./history-request"
import { HourSkeleton, type LoadProgress } from "./hour-skeleton"
import { HourPicker } from "./hour-picker"
import { Inspector, InspectorPortalProvider, InspectorRelatedPortal } from "./inspector"
import { keyboardTargetOwnsArrows } from "./keyboard"
import { rowMatchesLocator } from "./locator"
import { Login } from "./login"
import { parseSearch, type SearchSurface } from "./search"
import { findAfterSurfaceNavigation, searchSurfaceForLocation, searchSurfaceForSection } from "./search-navigation"
import { beginSearchRequest, IDLE_SEARCH_REQUEST, searchRequestForSurface, type SearchRequestState } from "./search-request"
import {
  activityFor,
  asNumber,
  floorHour,
  humanAge,
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
import { planRequest, statementRequest, type PlanLens, type StatementLens } from "./postgres-metrics"
import { isRelationLens, relationRequest, type RelationGroup, type RelationLens, type RelationNavigation, type RelationSection } from "./postgres-relations"
import { EMPTY_PROCESS_SUMMARY, LENS_FIELDS, ProcessSummary, ProcessTable, processSummaryReducer, processTableDefaultOrder } from "./process-table"
import { buildProcessForest } from "./process-tree"
import { latestTimelineTimestamp, refreshedCursor, scheduleRefresh } from "./refresh"
import type { ChartPoint } from "./series-chart"
import { apiFetch, bootstrapSession, getSessionSnapshot, logout, subscribeSession } from "./session"
import { hasPostgresTelemetry } from "./source-availability"
import type { RelatedNavigation } from "./statement-navigation"
import {
  CGROUP_SNAPSHOT_REQUESTS,
  SYSTEM_REQUESTS,
  SystemView,
  cgroupSnapshotPlan,
  clearCgroupSnapshotRows,
  recordedEnvironment,
} from "./system-view"
import { Timeline } from "./timeline"
import { TimezoneSelect } from "./timezone-select"

type Theme = "dark" | "light"

const EMPTY_DATA: HourData = {
  sections: {}, rateColumns: {}, snapshotRows: [], availableSections: [], syntheticDemo: false, postgresqlConfigured: false, postgresqlPresent: false, processes: [], activities: [], load: [], memory: [], pressure: [], health: [],
  points: [], lanePoints: [], laneContexts: [], findings: [], findingGroups: [],
}

interface CurrentSnapshot {
  readonly cursor: number | null
  readonly data: HourData
  readonly denseSection: string | null
  readonly target: string | null
}

const EMPTY_CURRENT_SNAPSHOT: CurrentSnapshot = { cursor: null, data: EMPTY_DATA, denseSection: null, target: null }

const VIEW_REQUESTS: Readonly<Record<string, readonly SectionRequest[]>> = {
  host: [...TIMELINE_REQUESTS, ...SYSTEM_REQUESTS],
  "postgresql:overview": [...TIMELINE_REQUESTS, ...POSTGRESQL_CONTEXT_REQUESTS],
  "postgresql:activity": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlActivity.map(section), ...POSTGRESQL_CONTEXT_REQUESTS],
  "postgresql:vacuum": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlVacuum.map(section), ...POSTGRESQL_CONTEXT_REQUESTS],
  "postgresql:locks": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlLocks.map(section), ...POSTGRESQL_CONTEXT_REQUESTS],
  "postgresql:databases": [...TIMELINE_REQUESTS, ...PRODUCT_SECTION_GROUPS.postgresqlDatabases.map(section), ...POSTGRESQL_CONTEXT_REQUESTS],
  events: TIMELINE_REQUESTS,
}

function processRequest(lens: Lens): SectionRequest {
  // Omit pageSize to request the complete snapshot for the tree.
  if (lens === "tree") return { section: "os_process" }
  const selected = processTableDefaultOrder(lens).column
  return {
    section: "os_process",
    pageSize: 200,
    defaultOrder: [selected],
    fallbackOrder: ["pid"],
    order: Object.fromEntries(LENS_FIELDS[lens].map((field) => [field.id, field.id === "command" ? ["cmdline", "comm"] : [field.field ?? field.id]])),
  }
}

function section(name: string): SectionRequest { return { section: name } }

const HELP_SYSTEM = [
  { label: "system.metric.health.label", help: "lane.health.os_health.help" },
  { label: "system.metric.cpu_used_cores.label", help: "system.metric.cpu_used_cores.help" },
  { label: "system.metric.cpu_capacity.label", help: "system.metric.cpu_capacity.help" },
  { label: "system.metric.cpu_actual_frequency.label", help: "system.metric.cpu_actual_frequency.help" },
  { label: "system.metric.cpu_scaling_frequency.label", help: "system.metric.cpu_scaling_frequency.help" },
  { label: "system.metric.cpu_user.label", help: "system.metric.cpu_user.help" },
  { label: "system.metric.cpu_system.label", help: "system.metric.cpu_system.help" },
  { label: "system.metric.cpu_iowait.label", help: "system.metric.cpu_iowait.help" },
  { label: "system.metric.cpu_steal.label", help: "system.metric.cpu_steal.help" },
  { label: "system.metric.load1.label", help: "system.metric.load1.help" },
  { label: "system.metric.mem_available.label", help: "system.metric.mem_available.help" },
  { label: "system.metric.mem_anon.label", help: "system.metric.mem_anon.help" },
  { label: "system.metric.mem_file_cache.label", help: "system.metric.mem_file_cache.help" },
  { label: "system.metric.mem_s_reclaimable.label", help: "system.metric.mem_s_reclaimable.help" },
  { label: "system.metric.mem_s_unreclaim.label", help: "system.metric.mem_s_unreclaim.help" },
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
  const [database, setDatabase] = useState<string | null>(null)
  useEffect(() => {
    const controller = new AbortController()
    apiFetch("/api/instance-label", { signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) return
        const body: { database?: string | null } = await response.json()
        if (typeof body.database === "string" && body.database !== "") setDatabase(body.database)
      })
      .catch(() => {})
    return () => controller.abort()
  }, [])
  useEffect(() => {
    document.title = database === null ? "Kronika" : `${database} — Kronika`
    return () => {
      document.title = "Kronika"
    }
  }, [database])
  const opened = useRef(readAddress(window.location.search))
  const [theme, setTheme] = useState<Theme>(initialTheme)
  const [hour, setHour] = useState<number | null>(opened.current.at === null ? null : floorHour(opened.current.at))
  const wanted = useRef(opened.current.at)
  const [availableHours, setAvailableHours] = useState<readonly number[]>([])
  const [cursor, setCursor] = useState(0)
  const followsLatest = useRef(opened.current.at === null)
  const [timelineData, setTimelineData] = useState<HourData>(EMPTY_DATA)
  const [backgroundTimeline, setBackgroundTimeline] = useState<TimelineData | null>(null)
  const backgroundTimelineRef = useRef(backgroundTimeline)
  backgroundTimelineRef.current = backgroundTimeline
  const [backgroundReadyHour, setBackgroundReadyHour] = useState<number | null>(null)
  const [currentSnapshot, setCurrentSnapshot] = useState<CurrentSnapshot>(EMPTY_CURRENT_SNAPSHOT)
  const pgPresent = hasPostgresTelemetry(timelineData)
  const [loading, setLoading] = useState(true)
  const [loadProgress, setLoadProgress] = useState<LoadProgress | undefined>(undefined)
  const loadThrottle = useRef(0)
  const [error, setError] = useState<string | null>(null)
  const [source, setSource] = useState<Source>(sourceOf(opened.current.view))
  const [systemMetric, setSystemMetric] = useState<string | null>(opened.current.metric)
  const visibleSource = source
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
  const [processSummary, dispatchProcessSummary] = useReducer(processSummaryReducer, EMPTY_PROCESS_SUMMARY)
  const [find, setFind] = useState(opened.current.find)
  const [searchRequest, setSearchRequest] = useState<SearchRequestState>(IDLE_SEARCH_REQUEST)
  const [order, setOrder] = useState<TableOrder | null>(opened.current.sort)
  const [selectedKey, setSelectedKey] = useState<string | null>(opened.current.row)
  const [inspectorPanel, setInspectorPanel] = useState<InspectorPanel>(opened.current.panel)
  const [mobileSearch, setMobileSearch] = useState(false)
  const [inspectorDetailRoot, setInspectorDetailRoot] = useState<HTMLElement | null>(null)
  const [inspectorChartRoot, setInspectorChartRoot] = useState<HTMLElement | null>(null)
  const [entityChartAvailable, setEntityChartAvailable] = useState(false)
  const [inspectorDetailTitle, setInspectorDetailTitle] = useState<string | null>(null)
  const inspectorDismiss = useRef<(() => void) | null>(null)
  const [timelineLane, setTimelineLane] = useState<string>(opened.current.lens === "memory" ? "memory" : opened.current.lens === "disk" ? "io_stall" : opened.current.lens === "cpu" ? "cpu_busy" : "health")
  const [selectedFinding, setSelectedFinding] = useState<Finding | null>(null)
  const [eventScope, setEventScope] = useState<readonly Finding[] | null>(null)
  const [findingRow, setFindingRow] = useState<DataRow | null>(null)
  const [systemFocus, setSystemFocus] = useState<Finding | null>(null)
  const context = useMemo(() => selectedFinding === null ? null : entityContext(selectedFinding, findingRow, t), [findingRow, selectedFinding, t])
  const clearEntityContext = useCallback(() => {
    setSelectedFinding(null)
    setEventScope(null)
    setFindingRow(null)
    setSystemFocus(null)
  }, [])
  const [helpOpen, setHelpOpen] = useState(false)
  const [mcpOpen, setMcpOpen] = useState(false)
  useEffect(() => {
    document.documentElement.dataset.theme = theme
    try { localStorage.setItem("kronika.theme", theme) } catch {}
  }, [theme])
  // Host sections read the same hour: switching resource tabs must not refetch.
  const baseViewKey = visibleSource === "host" ? "host" : visibleSource === "processes" ? "processes" : visibleSource === "postgresql" ? `postgresql:${pgSection}` : "events"
  const viewKey = pgSection === "statements" && visibleSource === "postgresql"
    ? `${baseViewKey}:${statementLens}`
    : pgSection === "plans" && visibleSource === "postgresql" ? `${baseViewKey}:${planLens}` : baseViewKey
  const viewRequests = useMemo(() => {
    if (visibleSource === "processes") return [
      ...TIMELINE_REQUESTS,
      processRequest(lens),
      { section: "pg_stat_activity" },
      { section: "instance_metadata" },
      ...(lens === "tree" ? [{ section: "os_meminfo" }] : []),
    ]
    if (visibleSource === "postgresql" && pgSection === "statements") return [...TIMELINE_REQUESTS, statementRequest(statementLens), ...POSTGRESQL_CONTEXT_REQUESTS]
    if (visibleSource === "postgresql" && pgSection === "plans") return [
      ...TIMELINE_REQUESTS,
      planRequest(planLens),
      ...POSTGRESQL_CONTEXT_REQUESTS,
    ]
    if (activeRelation && visibleSource === "postgresql") {
      return [...TIMELINE_REQUESTS, relationRequest(relationSectionOf(pgSection), activeRelationLens, relationLevel), ...POSTGRESQL_CONTEXT_REQUESTS]
    }
    return VIEW_REQUESTS[baseViewKey] ?? []
  }, [activeRelation, activeRelationLens, baseViewKey, lens, pgSection, planLens, relationLevel, statementLens, visibleSource])
  const [segments, setSegments] = useState<readonly SegmentBound[]>([])
  const densePattern = viewRequests.some((request) => request.pageSize !== undefined) ? find.trim() : ""
  const denseCandidate = viewRequests.find((request) => request.pageSize !== undefined)
  const denseSurface = denseCandidate === undefined ? null : searchSurfaceForSection(denseCandidate.section)
  const denseSearchValid = densePattern === "" || denseSurface === null || parseSearch(densePattern, denseSurface, {
    grouped: denseCandidate !== undefined && "group" in denseCandidate && denseCandidate.group !== undefined && denseCandidate.group !== "object",
  }).ok
  const requestedSnapshots = viewRequests.filter((request) => request.section !== "health" && (request.pageSize === undefined || denseSearchValid)).map((request) => request.pageSize !== undefined && request.section === context?.logicalName
    ? { ...request, typeIds: [context.typeId] } : request)
  const snapshotGroups = snapshotRequestGroups(segments, cursor, requestedSnapshots)
  const denseLoad = snapshotGroups.flatMap((group) => group.requests
    .filter((request) => request.pageSize !== undefined)
    .map((request) => ({ anchor: group.anchor, request })))[0]
  const denseRequest = denseLoad?.request
  const ordinaryGroups = snapshotGroups
    .map((group) => ({
      anchor: group.anchor,
      requests: group.requests.filter((request) => request.pageSize === undefined),
    }))
    .filter((group) => group.requests.length !== 0)
  const pageContext = denseRequest !== undefined && context?.logicalName === denseRequest.section ? context : null
  const denseOptions = denseRequest === undefined
    ? undefined
    : initialPageOptions(denseRequest, pageContext, densePattern, relationFilters)
  const requestOrder = visibleSource === "processes" ? order ?? processTableDefaultOrder(lens) : order
  const cgroupTargetGroups = visibleSource === "host" && timelineData.availableSections.some((name) => name.startsWith("os_cgroup"))
    ? snapshotRequestGroups(segments, cursor, CGROUP_SNAPSHOT_REQUESTS)
    : []
  const snapshotTarget = snapshotGroups.length === 0
    ? null
    : snapshotTargetKey(snapshotGroups, cursor, cgroupTargetGroups, requestOrder, denseOptions)
  const retainsDenseRows = denseRequest !== undefined
    && currentSnapshot.cursor === cursor
    && currentSnapshot.denseSection === denseRequest.section
  const currentData = currentSnapshot.target === snapshotTarget || retainsDenseRows ? currentSnapshot.data : EMPTY_DATA
  const data = useMemo(() => viewData(timelineData, currentData), [currentData, timelineData])
  const environment = useMemo(() => recordedEnvironment(data, cursor), [cursor, data])
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
  const pendingRefresh = useRef<{
    readonly timeline: TimelineData
    readonly previousCursor: number
    readonly previousSegments: readonly SegmentBound[]
  } | null>(null)
  const finishRefresh = useCallback((succeeded: boolean) => {
    if (!refreshRequested.current) return
    const pending = pendingRefresh.current
    if (succeeded && pending !== null) {
      drawn.current = pending.timeline.hour
      setAvailableHours(pending.timeline.availableHours)
      setSegments(pending.timeline.segments)
      setTimelineData((current) => withTimelineLanes(hourOf(pending.timeline), {
        contexts: current.laneContexts,
        points: current.lanePoints,
      }))
      setBackgroundTimeline(pending.timeline)
    } else if (pending !== null) {
      setSegments(pending.previousSegments)
      setCursor(pending.previousCursor)
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
      setBackgroundTimeline(null)
      setBackgroundReadyHour(null)
      setLoading(true)
      setRefreshing(false)
      setError(null)
      setLoadProgress({ received: 0, startedAt: Date.now() * 1_000, lastSeconds: readLastLoadSeconds() })
    }
    const loadStartedAt = Date.now() * 1_000
    void loadTimeline(hour, controller.signal, refresh ? undefined : (received) => {
      const now = Date.now()
      if (now - loadThrottle.current < 250) return
      loadThrottle.current = now
      setLoadProgress((current) => current === undefined ? current : { ...current, received })
    }).then((timeline) => {
      const asked = wanted.current
      wanted.current = null
      const latest = latestTimelineTimestamp(timeline)
      if (refresh) {
        pendingRefresh.current = { timeline, previousCursor: cursor, previousSegments: segmentsRef.current }
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
        setBackgroundTimeline(timeline)
        setCurrentSnapshot(EMPTY_CURRENT_SNAPSHOT)
        followsLatest.current = asked === null || floorHour(asked) !== timeline.hour
        setCursor(followsLatest.current ? latest : asked ?? latest)
        setLastUpdated(Date.now() * 1_000)
        setRefreshing(false)
        writeLastLoadSeconds((Date.now() * 1_000 - loadStartedAt) / 1_000_000)
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
      setBackgroundTimeline(null)
      setBackgroundReadyHour(null)
      setCurrentSnapshot(EMPTY_CURRENT_SNAPSHOT)
      setCursor(fallback)
      setError(reason instanceof Error ? reason.message : String(reason))
      setLoading(false)
    })
    return () => controller.abort()
  }, [finishRefresh, hour, refreshVersion])
  const [cursorState, setCursorState] = useState<"ready" | "loading" | "missing">("ready")
  const [densePageState, setDensePageState] = useState<"idle" | "loading" | "error">("idle")
  const snapshotGeneration = useRef(0)
  const cgroupSnapshotKey = useRef<string | null>(null)
  const densePage = useRef<{
    failed: string | undefined
    load: (cursor?: string) => void
  } | null>(null)
  useEffect(() => {
    const generation = ++snapshotGeneration.current
    const tracksInitialSearch = denseRequest !== undefined
      && denseSurface !== null
      && denseSearchValid
      && snapshotTarget !== null
      && (densePattern !== "" || (searchRequest.phase === "pending" && searchRequest.surface === denseSurface))
    const retainedSearchRows = denseRequest !== undefined
      && retainsDenseRows
      && (denseRequest.section === "os_process"
        ? currentSnapshot.data.processes.length !== 0
        : (currentSnapshot.data.sections[denseRequest.section]?.length ?? 0) !== 0)
    if (tracksInitialSearch && denseSurface !== null) {
      setSearchRequest(beginSearchRequest(denseSurface, retainedSearchRows))
    } else if (denseSurface === null || !denseSearchValid) {
      setSearchRequest(IDLE_SEARCH_REQUEST)
    }
    cgroupSnapshotKey.current = null
    setCurrentSnapshot((current) => current.target === snapshotTarget
      ? { ...current, data: clearCgroupSnapshotRows(current.data) }
      : current)
    const completesRefresh = refreshAwaitingSnapshot.current
    densePage.current = null
    if (hour === null) {
      if (!completesRefresh) setCurrentSnapshot(EMPTY_CURRENT_SNAPSHOT)
      setCursorState("missing")
      setDensePageState("idle")
      if (completesRefresh) finishRefresh(false)
      return
    }
    if (backgroundTimeline === null || backgroundTimeline.hour !== hour) {
      setCursorState("loading")
      setDensePageState("idle")
      return
    }
    if (snapshotTarget === null) {
      if (!completesRefresh) setCurrentSnapshot(EMPTY_CURRENT_SNAPSHOT)
      setCursorState("ready")
      setDensePageState("idle")
      setBackgroundReadyHour(hour)
      if (completesRefresh) finishRefresh(true)
      return
    }
    setCursorState("loading")
    setDensePageState(denseRequest === undefined ? "idle" : "loading")
    const controller = new AbortController()
    const stale = () => controller.signal.aborted || generation !== snapshotGeneration.current
    const loadOrdinarySnapshot = async (ordinary: readonly SnapshotRequestGroup[]): Promise<HourData> => {
      const primary = await loadSnapshotGroups(ordinary, cursor, controller.signal, requestOrder ?? undefined)
      if (stale() || cgroupTargetGroups.length === 0) return primary
      const plans = cgroupTargetGroups.map((group) => ({
        anchor: group.anchor,
        plan: cgroupSnapshotPlan(group.anchor.id, cursor, primary, group.requests),
      }))
      const planKey = JSON.stringify(plans.map(({ anchor, plan }) => [anchor.id, plan.key]))
      cgroupSnapshotKey.current = planKey
      const exact = await Promise.all(plans.flatMap(({ anchor, plan }) => plan.loads.map(({ filters, request }) => loadSnapshot(
        anchor.id,
        cursor,
        [request],
        controller.signal,
        undefined,
        { filters },
      ).catch((reason: unknown) => {
        if (!stale() && cgroupSnapshotKey.current === planKey) {
          console.error(`kronika: filtered ${request.section} snapshot failed`, reason)
        }
        return EMPTY_DATA
      }))))
      if (stale() || cgroupSnapshotKey.current !== planKey) return primary
      return exact.reduce((current, incoming) => mergeSnapshotData(current, incoming), primary)
    }
    const timer = setTimeout(() => {
      if (denseLoad === undefined) {
        void loadOrdinarySnapshot(ordinaryGroups)
        .then((incoming) => {
          if (stale()) return
          setCurrentSnapshot({ cursor, data: incoming, denseSection: null, target: snapshotTarget })
          setCursorState("ready")
          setBackgroundReadyHour(hour)
          setLastUpdated(Date.now() * 1_000)
          if (completesRefresh) finishRefresh(true)
        })
        .catch((reason: unknown) => {
          if (stale()) return
          setCursorState("missing")
          setBackgroundReadyHour(hour)
          if (completesRefresh) finishRefresh(false)
          console.error("kronika: snapshot at the cursor failed", reason)
        })
        return
      }
      const pagedRequest = denseLoad.request
      const base = ordinaryGroups.length === 0
        ? Promise.resolve(EMPTY_DATA)
        : loadOrdinarySnapshot(ordinaryGroups)
          .catch((reason: unknown) => {
            if (completesRefresh && visibleSource !== "processes") throw reason
            if (!controller.signal.aborted) console.error("kronika: snapshot companion failed", reason)
            return EMPTY_DATA
          })
      let inFlight = false
      const action = {
        failed: undefined as string | undefined,
        load: (pageCursor?: string) => {
          if (inFlight || stale()) return
          inFlight = true
          action.failed = undefined
          setDensePageState("loading")
          const tracksPageSearch = denseSurface !== null && (pageCursor === undefined ? tracksInitialSearch : densePattern !== "")
          if (tracksPageSearch && denseSurface !== null) {
            setSearchRequest(beginSearchRequest(denseSurface, pageCursor === undefined ? retainedSearchRows : true))
          }
          const options = {
            ...denseOptions,
            ...(pageCursor === undefined ? {} : { cursor: pageCursor }),
          }
          const page = loadSnapshot(denseLoad.anchor.id, cursor, [pagedRequest], controller.signal, requestOrder ?? undefined, options)
          const loaded = (incoming: HourData, companion: HourData | null) => {
            if (stale()) return
            setCurrentSnapshot((current) => ({
              cursor,
              data: pageCursor === undefined
                ? mergeSnapshotData(companion ?? EMPTY_DATA, incoming)
                : mergeSnapshotData(current.target === snapshotTarget ? current.data : EMPTY_DATA, incoming, pagedRequest.section),
              denseSection: pagedRequest.section,
              target: snapshotTarget,
            }))
            setDensePageState("idle")
            setCursorState("ready")
            setBackgroundReadyHour(hour)
            if (tracksPageSearch) setSearchRequest(IDLE_SEARCH_REQUEST)
            setLastUpdated(Date.now() * 1_000)
            if (completesRefresh && pageCursor === undefined) finishRefresh(true)
          }
          const failed = (reason: unknown) => {
            if (stale()) return
            action.failed = pageCursor
            setDensePageState("error")
            setCursorState("ready")
            setBackgroundReadyHour(hour)
            if (tracksPageSearch && denseSurface !== null) {
              setSearchRequest({ phase: "error", retained: pageCursor !== undefined || retainedSearchRows, surface: denseSurface })
            }
            if (completesRefresh && pageCursor === undefined) finishRefresh(false)
            console.error("kronika: snapshot page failed", reason)
          }
          if (pageCursor === undefined && visibleSource === "processes") {
            void page.then((incoming) => {
              loaded(incoming, null)
              void base.then((companion) => {
                if (stale() || companion === EMPTY_DATA) return
                setCurrentSnapshot((current) => current.target !== snapshotTarget ? current : {
                  ...current,
                  data: mergeSnapshotData(current.data, companion),
                })
              }).catch((reason: unknown) => {
                if (!stale()) console.error("kronika: snapshot companion failed", reason)
              })
            }).catch(failed).finally(() => { inFlight = false })
            return
          }
          void Promise.all([
            pageCursor === undefined ? base : Promise.resolve(null),
            page,
          ]).then(([companion, incoming]) => loaded(incoming, companion)).catch(failed)
            .finally(() => { inFlight = false })
        },
      }
      densePage.current = action
      action.load()
    }, backgroundReadyHour === hour ? 250 : 0)
    return () => { clearTimeout(timer); controller.abort() }
  }, [backgroundTimeline, finishRefresh, hour, snapshotTarget])
  useEffect(() => {
    if (backgroundTimeline === null || backgroundTimeline.segments.length === 0
      || backgroundReadyHour !== backgroundTimeline.hour
      || hour !== backgroundTimeline.hour) return
    const controller = new AbortController()
    void loadTimelineLanes(backgroundTimeline, controller.signal).then((lanes) => {
      if (controller.signal.aborted || backgroundTimelineRef.current !== backgroundTimeline) return
      setTimelineData((current) => withTimelineLanes(current, lanes))
    }).catch((reason: unknown) => {
      if (!controller.signal.aborted) console.error("kronika: timeline lanes failed", reason)
    })
    return () => controller.abort()
  }, [backgroundReadyHour, backgroundTimeline, hour])
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
      if (event.key === "?") {
        setHelpOpen((current) => !current)
        setMcpOpen(false)
      }
      if (event.key === "Escape") {
        setHelpOpen(false)
        setMcpOpen(false)
      }
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
  const memTotalKb = useMemo(
    () => asNumber(value(snapshot(data.sections.os_meminfo ?? [], cursor)[0] ?? null, "mem_total")),
    [cursor, data.sections.os_meminfo],
  )
  // The forest ignores entity context, so it must not rebuild when context changes.
  const processForest = useMemo(
    () => lens === "tree" ? buildProcessForest(allProcessRows, memTotalKb, ticksPerSecond) : null,
    [allProcessRows, lens, memTotalKb, ticksPerSecond],
  )
  const processTableRows = processForest ?? processRows
  const pgRows = useMemo(() => snapshot(data.activities, cursor), [cursor, data.activities])
  const linkedPids = useMemo(() => new Set(pgRows.flatMap((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid === null ? [] : [pid]
  })), [pgRows])
  const selectedProcess = useMemo(
    () => processTableRows.find((row) => processKey(row) === selectedKey) ?? null,
    [processTableRows, selectedKey],
  )
  useEffect(() => {
    if (selectedFinding?.logicalName === "os_process" && findingRow !== null) {
      setSelectedKey(processKey(findingRow))
      setInspectorPanel("detail")
    }
  }, [findingRow, selectedFinding])
  useEffect(() => {
    if (selectedFinding === null) {
      setFindingRow(null)
      return
    }
    if (findingRow !== null && rowMatchesLocator(findingRow, selectedFinding)) return
    const loaded = resolveLocator(data, selectedFinding)?.row ?? null
    if (loaded !== null) {
      setFindingRow(loaded)
      return
    }
    const fields = findingProjection(selectedFinding)
    if (selectedFinding.typeId === "0" || fields.length === 0) {
      setFindingRow(null)
      return
    }
    setFindingRow(null)
    const controller = new AbortController()
    acceptResponse(loadSnapshot(
      selectedFinding.segmentId,
      selectedFinding.timestamp,
      [{ section: selectedFinding.logicalName, fields, typeId: selectedFinding.typeId }],
      controller.signal,
      undefined,
      { typeId: selectedFinding.typeId, rowOrdinal: selectedFinding.rowOrdinal, fullText: true },
    ), controller.signal, (incoming) => {
      setFindingRow(incoming.sections[selectedFinding.logicalName]?.[0] ?? null)
    })
    return () => controller.abort()
  }, [data, findingRow, selectedFinding])
  const pgFocus = selectedFinding !== null && selectedFinding.logicalName.startsWith("pg_") ? contextRow : null
  const joinedActivity = activityFor(selectedProcess, data.activities, selectedProcess?.timestamp ?? cursor)
  const selectedPid = selectedProcess === null ? null : rawText(value(selectedProcess, "pid"))
  const processHistoryKey = hour === null || selectedPid === null ? null : JSON.stringify([hour, selectedPid])
  const processHistory = useHistoryRequest(processHistoryKey, refreshVersion,
    processHistoryKey === null || hour === null || selectedPid === null
      ? null
      : (signal) => loadSeries(hour, "os_process", { pid: selectedPid }, PROCESS_HISTORY_FIELDS, signal))
  const sharedNavigationTimestamps = useMemo(
    () => observationTimestamps(data.lanePoints, data.health),
    [data.health, data.lanePoints],
  )
  const navigationTimestamps = useMemo(() => {
    if (visibleSource === "processes") {
      const summary = processSummary.hour === hour ? processSummary.history : []
      return mergeObservationTimestamps(sharedNavigationTimestamps, summary, data.processes, processHistory.value ?? [])
    }
    if (visibleSource === "postgresql" && pgSection === "activity") {
      return mergeObservationTimestamps(sharedNavigationTimestamps, data.activities)
    }
    if (visibleSource === "postgresql" && pgSection === "vacuum") {
      return mergeObservationTimestamps(sharedNavigationTimestamps, data.sections.pg_stat_progress_vacuum ?? [])
    }
    return sharedNavigationTimestamps
  }, [data.activities, data.processes, data.sections.pg_stat_progress_vacuum, hour, pgSection, processHistory.value, processSummary.history, processSummary.hour, sharedNavigationTimestamps, visibleSource])
  const activeSearchSurface = searchSurfaceForLocation(source, pgSection)
  const visibleSearchRequest = searchRequestForSurface(searchRequest, activeSearchSurface)
  const applyFind = useCallback((next: string) => {
    if (next === find) return
    setFind(next)
    if (activeSearchSurface === null || denseSurface !== activeSearchSurface) return
    setSearchRequest(beginSearchRequest(activeSearchSurface, searchRowsAvailable(data, activeSearchSurface)))
  }, [activeSearchSurface, data, denseSurface, find])
  const navigateSearchSurface = useCallback((targetSurface: ReturnType<typeof searchSurfaceForLocation>, targetFind?: string) => {
    const next = findAfterSurfaceNavigation(activeSearchSurface, targetSurface, find, targetFind)
    if (next !== find) setFind(next)
    if (targetFind !== undefined && targetSurface !== null && next !== find) {
      setSearchRequest(beginSearchRequest(targetSurface, targetSurface === activeSearchSurface && searchRowsAvailable(data, targetSurface)))
      return
    }
    if (targetSurface !== activeSearchSurface) setSearchRequest(IDLE_SEARCH_REQUEST)
  }, [activeSearchSurface, data, find])
  const address = useMemo(() => writeAddress({
    at: cursor === 0 ? null : cursor,
    view: viewOf(source, pgSection),
    lens,
    pgLens: activeRelation
      ? activeRelationLens : source === "postgresql" && pgSection === "plans" ? planLens : statementLens,
    pgLevel: relationLevel,
    datid: relationFilters.datid ?? null,
    schema: relationFilters.schemaname ?? null,
    relid: relationFilters.relid ?? null,
    indexrelid: relationFilters.indexrelid ?? null,
    tablespaceOid: relationFilters.tablespace_oid ?? null,
    sort: order,
    row: activeRelation
      ? relationSelectedKey
      : source === "host" || source === "processes" || source === "postgresql" || source === "events" ? selectedKey : null,
    panel: inspectorPanel,
    find,
    metric: source === "host" ? systemMetric : null,
  }), [activeRelation, activeRelationLens, cursor, find, inspectorPanel, lens, order, pgSection, planLens, relationFilters, relationLevel, relationSelectedKey, selectedKey, source, statementLens, systemMetric])
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
      setSystemMetric(opening.metric)
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
      setInspectorPanel(opening.panel)
      setFind(opening.find)
      setSearchRequest(IDLE_SEARCH_REQUEST)
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
    navigateSearchSurface(searchSurfaceForSection(nextSection))
    setPgSection(nextSection)
    setRelationLens(nextLens)
    setRelationLevel(navigation.group)
    setRelationFilters(navigation.filters)
    setRelationSelectedKey(navigation.selectedKey)
    setOrder((current) => current !== null && !crossing && Object.hasOwn(relationRequest(navigation.section, nextLens, navigation.group).order ?? {}, current.column) ? current : null)
  }, [activeRelationLens, navigateSearchSurface, pgSection])
  const choosePgSection = useCallback((next: PostgresSection) => {
    const crossing = next !== pgSection
    if (crossing) {
      setSelectedKey(null)
      setInspectorPanel(null)
    }
    setRelationSelectedKey(null)
    navigateSearchSurface(searchSurfaceForSection(next))
    setPgSection(next)
    if (next !== "tables" && next !== "indexes") return
    setRelationLens((current) => relationLensOf(next, current))
    setRelationFilters((current) => relationFiltersForSection(current, next))
  }, [navigateSearchSurface, pgSection])
  const openRelated = useCallback((target: RelatedNavigation) => {
    // A related drill replaces any finding context.
    clearEntityContext()
    navigateSearchSurface(searchSurfaceForSection(target.section), target.expression)
    setSource("postgresql")
    setPgSection(target.section)
    if (target.section === "statements") setStatementLens("load")
    else setPlanLens("load")
    setSelectedKey(null)
    setInspectorPanel(null)
    setOrder(null)
  }, [clearEntityContext, navigateSearchSurface])
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
    setInspectorPanel("detail")
  }, [])
  const selectFinding = useCallback((finding: Finding, grouped: readonly Finding[] = [finding]) => {
    followsLatest.current = false
    setCursor(finding.timestamp)
    setFindingRow(null)
    if (grouped.length > 1) {
      setSelectedFinding(null)
      setEventScope(grouped)
      setInspectorPanel(null)
      navigateSearchSurface("events")
      setSource("events")
      return
    }
    setSelectedFinding(finding)
    setEventScope(null)
    setOrder(null)
    const resolved = resolveLocator(data, finding)
    if (resolved !== null) setFindingRow(resolved.row)
    const route = findingRoute(finding)
    if (route === "processes") {
      navigateSearchSurface("os_process")
      setSource("processes")
      setLens(processLens(fieldNameForLocator(finding)))
      setSystemFocus(null)
      if (resolved !== null) {
        setSelectedKey(processKey(resolved.row))
        setInspectorPanel("detail")
      }
      return
    }
    if (route === "system") {
      navigateSearchSurface(null)
      setSource("host")
      setSystemFocus(finding)
      setInspectorPanel(null)
      return
    }
    if (route !== "events") {
      navigateSearchSurface(searchSurfaceForSection(route))
      setSource("postgresql")
      setPgSection(route)
      if (route === "statements") setStatementLens("load")
      if (route === "plans") setPlanLens("load")
      setSystemFocus(null)
      setFindingRow(resolved?.row ?? null)
      setInspectorPanel("detail")
      return
    }
    navigateSearchSurface("events")
    setSource("events")
    setInspectorPanel("detail")
  }, [data, navigateSearchSurface])
  const eventsPresent = data.findings.length !== 0
  const helpItems = visibleSource === "postgresql"
    ? HELP_POSTGRESQL
    : visibleSource === "events"
      ? HELP_EVENTS
      : visibleSource === "processes" ? HELP_PROCESS : HELP_SYSTEM
  const stretchPostgres = visibleSource === "postgresql" && pgSection !== "overview"
  const cursorTime = cursor === 0 ? null : time.clock(cursor)
  const updatedClock = lastUpdated === null ? null : time.clock(lastUpdated)
  const detailAvailable = selectedProcess !== null || inspectorDetailTitle !== null
  const inspectorOpen = inspectorPanel === "chart" || (inspectorPanel !== null && detailAvailable)
  const closeInspector = () => {
    inspectorDismiss.current?.()
    setInspectorPanel(null)
    if (visibleSource === "processes") setSelectedKey(null)
    if (visibleSource === "postgresql") {
      setSelectedKey(null)
      setRelationSelectedKey(null)
    }
    if (visibleSource === "events") setSelectedFinding(null)
  }
  const openChart = () => setInspectorPanel("chart")
  const openPortalDetail = useCallback(() => setInspectorPanel("detail"), [])
  const selectDetailKey = useCallback((key: string | null) => {
    setSelectedKey(key)
    if (key !== null) setInspectorPanel("detail")
  }, [])
  const selectRelationDetail = useCallback((key: string | null) => {
    setRelationSelectedKey(key)
    if (key !== null) setInspectorPanel("detail")
  }, [])
  const timelinePrimary = visibleSource === "processes"
    ? lens === "cpu" ? "cpu_busy" : lens === "memory" ? "memory" : lens === "disk" ? "io_stall" : "health"
    : visibleSource === "postgresql" ? pgSection === "statements" || pgSection === "plans" ? "pg_running" : pgSection === "activity" || pgSection === "locks" || pgSection === "vacuum" ? "pg_waiting" : "health"
      : "health"
  return <DisplayTimeScope hour={hour}><main className={`app-shell flex h-dvh min-h-0 flex-col overflow-hidden${stretchPostgres ? " pg-table-shell" : ""}${inspectorOpen ? " inspector-open" : ""}${inspectorOpen && inspectorPanel === "chart" && !(entityChartAvailable && detailAvailable) ? " inspector-chart-open" : ""}${mobileSearch ? " mobile-search-open" : ""}`}>
    {data.syntheticDemo === true && <p className="pointer-events-none fixed bottom-2 left-2 z-[70] m-0 rounded border border-line3 bg-s1/95 px-2 py-1 font-sans text-[11px] font-medium tracking-[0.04em] text-fg3 shadow-sm" data-testid="demo-notice">{t("demo.synthetic")}</p>}
    <header className="topbar [.pg-table-shell>&]:flex-none">
      <span className="flex flex-none items-center text-accent2"><Activity aria-hidden="true" size={15} strokeWidth={2} /></span>
      <h1>{database === null ? t("app.title") : `${t("app.title")} — ${database}`}</h1>

      <nav aria-label={t("nav.sources")} className="source-tabs max-[760px]:overflow-x-auto">
        <button aria-current={visibleSource === "host" ? "page" : undefined} className={visibleSource === "host" ? "source-active" : undefined} onClick={() => { navigateSearchSurface(null); setSystemFocus(null); setSelectedKey(null); setInspectorPanel(null); setSource("host") }} type="button">{t("nav.host")}</button>
        <button aria-current={visibleSource === "processes" ? "page" : undefined} className={visibleSource === "processes" ? "source-active" : undefined} data-testid="process-tab" onClick={() => { navigateSearchSurface("os_process"); setSelectedKey(null); setInspectorPanel(null); setSource("processes") }} type="button">{t("nav.processes")}</button>
        <button aria-current={visibleSource === "postgresql" ? "page" : undefined} className={visibleSource === "postgresql" ? "source-active" : undefined} onClick={() => { navigateSearchSurface(searchSurfaceForSection(pgSection)); setSelectedKey(null); setInspectorPanel(null); setSource("postgresql") }} title={pgPresent ? undefined : t("nav.no_data")} type="button">{t("nav.postgresql")}</button>
        <button aria-current={visibleSource === "events" ? "page" : undefined} className={visibleSource === "events" ? "source-active" : undefined} onClick={() => { navigateSearchSurface("events"); setEventScope(null); setSelectedFinding(null); setInspectorPanel(null); setSource("events") }} title={eventsPresent ? undefined : t("nav.no_data")} type="button">{t("nav.events")}</button>
      </nav>

      <HourPicker availableHours={availableHours} changeHour={changeHour} hour={hour} locale={locale} t={t} />
      <div aria-live="polite" className="cursor-time max-[760px]:order-9">
        <TimeValue label={t("hour.cursor_label")} output={cursorTime} testId="cursor-time" />
        {lastUpdated !== null && updatedClock !== null && <UpdatedAge at={lastUpdated} clock={updatedClock} locale={locale} t={t} />}
        {cursorState === "loading" && <span className="ml-2.5 flex flex-none items-center gap-1.5 font-sans text-xs text-fg3" data-testid="cursor-behind" role="status"><span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none" />{t("status.updating")}</span>}
        {cursorState === "missing" && <span className="cursor-missing ml-2 font-sans text-xs text-warn" data-testid="cursor-behind">{t("status.no_sample")}</span>}
        {refreshFailed && <span>{t("refresh.error")}</span>}
      </div>

      <div className="top-actions">
        <button aria-label={t("inspector.open_chart")} aria-pressed={inspectorPanel === "chart"} className="icon-button text-fg2 aria-pressed:bg-s4 aria-pressed:text-accent3" data-testid="charts-toggle" onClick={openChart} title={t("inspector.open_chart")} type="button"><ChartLine aria-hidden="true" size={14} /></button>
        <button aria-label={t("refresh.action")} className="icon-button" disabled={refreshing || !refreshReady} onClick={requestRefresh} title={t("refresh.action")} type="button"><RotateCw aria-hidden="true" size={14} /></button>
        <TimezoneSelect mode={time.mode} setMode={time.setMode} t={t} />
        <button aria-label={t("common.theme.switch")} className="icon-button" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} title={t(theme === "dark" ? "common.theme.light" : "common.theme.dark")} type="button">
          {theme === "dark" ? <Sun aria-hidden="true" size={14} /> : <Moon aria-hidden="true" size={14} />}
        </button>
        <div aria-label={t("locale.switch")} className="locale-switch" role="group">
          {(["ru", "en"] as const).map((choice) => <button aria-pressed={locale === choice} data-testid={`locale-${choice}`} key={choice} onClick={() => onLocale(choice)} type="button">{t(`locale.${choice}`)}</button>)}
        </div>
        <button aria-label={t("auth.logout")} className="icon-button" onClick={logout} title={t("auth.logout")} type="button"><LogOut aria-hidden="true" size={14} /></button>
        <button aria-expanded={mcpOpen} aria-label={t("mcp.open")} className="icon-button" data-testid="mcp-trigger" onClick={() => { setMcpOpen((current) => !current); setHelpOpen(false) }} title={t("mcp.open")} type="button"><Plug aria-hidden="true" size={14} /></button>
        <button aria-expanded={helpOpen} aria-label={t("help.open")} className="icon-button" data-testid="help-trigger" onClick={() => { setHelpOpen((current) => !current); setMcpOpen(false) }} title={t("help.open")} type="button"><CircleHelp aria-hidden="true" size={14} /></button>
      </div>
    </header>

    <InspectorPortalProvider chartTarget={inspectorChartRoot} dismissRef={inspectorDismiss} onChartAvailable={setEntityChartAvailable} onOpen={openPortalDetail} onTitle={setInspectorDetailTitle} target={inspectorDetailRoot}>
    <div className="workspace-frame flex min-h-0 min-w-0 flex-1 overflow-hidden">
    <section className={`workspace min-h-0 min-w-0 flex-1 px-2.5 pb-2 pt-2 max-[760px]:px-2${stretchPostgres ? " pg-table-workspace flex flex-col overflow-hidden [&>.timeline-shell]:flex-none [&>.pg-tabs]:flex-none [&>.lensbar]:flex-none [&>[data-testid=table-paging]]:flex-none" : " overflow-auto"}${visibleSource === "processes" ? " process-workspace flex flex-col overflow-hidden [&>.timeline-shell]:flex-none [&>.lensbar]:flex-none" : ""}`}>
      <p aria-live="polite" className="absolute m-0 h-px w-px overflow-hidden whitespace-nowrap [clip-path:inset(50%)]">
        {t(`nav.${visibleSource}`)}
        {visibleSource === "postgresql" ? ` · ${t(`pg.section.${pgSection}`)}` : ""}
      </p>
      {!loading && error === null && hour !== null && <MobileControls filtered={find !== ""} onOpenChart={openChart} onSearch={setMobileSearch} searchOpen={mobileSearch} t={t} />}
      {loading && <HourSkeleton locale={locale} progress={loadProgress} t={t} />}
      {!loading && error !== null && <StateCard message={t("status.error")} />}
      {!loading && error === null && hour !== null && visibleSource === "host" && <SystemView context={context} contextRow={contextRow} cursor={cursor} data={data} focus={systemFocus} historyRevision={refreshVersion} hour={hour} locale={locale} metric={systemMetric} navigationTimestamps={navigationTimestamps} onContextClear={clearEntityContext} onCursor={chooseCursor} onFinding={selectFinding} onMetric={setSystemMetric} onOpenChart={openChart} onSelectedKey={selectDetailKey} onSelectedLane={setTimelineLane} selectedKey={selectedKey} selectedLane={timelineLane} t={t} tablesLoading={cursorState === "loading"} />}
      {!loading && error === null && hour !== null && visibleSource === "processes" && <>
        <Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={chooseCursor} onFinding={selectFinding} onOpenChart={openChart} onSelectedLane={setTimelineLane} primaryLane={timelinePrimary} selectedLane={timelineLane} shownAt={shownAt} t={t} />
        <div className="lensbar !mt-0 border-t-0">
          <div aria-label={t("nav.processes")} className="lens-tabs max-[760px]:w-full max-[760px]:[&>button]:min-w-0 max-[760px]:[&>button]:flex-1 max-[760px]:[&>button]:px-1" role="group">
            {(["tree", "cpu", "memory", "disk", "generic"] as const).map((choice) => <button aria-pressed={lens === choice} data-testid={`lens-${choice}`} key={choice} onClick={() => { if (choice !== lens) setOrder(null); setLens(choice) }} type="button">{t(`lens.${choice}`)}</button>)}
          </div>
          <ProcessSummary cursor={cursor} dispatch={dispatchProcessSummary} enabled={backgroundReadyHour === hour} hour={hour} lens={lens} locale={locale} state={processSummary} t={t} />
          <span className="snapshot-time">{processTableRows[0] === undefined ? t("status.no_data") : time.timestamp(processTableRows[0].timestamp, hour)}</span>
        </div>
        <ProcessesActivity cursor={cursor} hour={hour} locale={locale} onCursor={chooseCursor} onPattern={applyFind} t={t} ticksPerSecond={ticksPerSecond} />
        <div className="process-main grid min-h-0 min-w-0 flex-1 grid-cols-[minmax(0,1fr)] grid-rows-[minmax(0,1fr)] overflow-hidden">
          <ProcessTable contextLabel={lens !== "tree" && context?.logicalName === "os_process" ? context.label : undefined} densePageState={lens === "tree" ? "idle" : densePageState} finding={selectedFinding?.logicalName === "os_process" ? selectedFinding : null} findingField={selectedFinding?.logicalName === "os_process" ? fieldNameForLocator(selectedFinding) : null} lens={lens} linkedPids={linkedPids} locale={locale} metadata={lens === "tree" ? undefined : denseMetadata} onContextClear={clearEntityContext} onLoadMore={loadMoreDense} onOrder={setOrder} onPattern={applyFind} onRetry={retryDense} onSelect={selectProcess} order={requestOrder} pattern={find} rows={processTableRows} searchRequest={visibleSearchRequest} selectedKey={selectedKey} t={t} ticksPerSecond={ticksPerSecond} />
        </div>
      </>}
      {!loading && error === null && hour !== null && visibleSource === "postgresql" && <PostgresView context={context} densePageState={densePageState} searchRequest={visibleSearchRequest} tablesLoading={cursorState === "loading"} onContextClear={clearEntityContext} onLoadMore={loadMoreDense} onRetry={retryDense} onRelated={openRelated} onOrder={setOrder} onPattern={applyFind} onSelectedKey={selectDetailKey} order={order ?? undefined} pattern={find} cursor={cursor} data={data} focus={pgFocus} focusFinding={selectedFinding} historyRevision={refreshVersion} hour={hour} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={chooseCursor} onFinding={selectFinding} onOpenChart={openChart} onPlanLens={(next) => { setOrder(null); setPlanLens(next) }} onRelationLens={chooseRelationLens} onRelationNavigate={navigateRelation} onRelationSelectedKey={selectRelationDetail} onSection={choosePgSection} onSelectedLane={setTimelineLane} onStatementLens={(next) => { setOrder(null); setStatementLens(next) }} planLens={planLens} relationFilters={relationFilters} relationLens={activeRelationLens} relationLevel={relationLevel} relationSelectedKey={relationSelectedKey} section={pgSection} segments={segments} selectedKey={selectedKey} selectedLane={timelineLane} statementLens={statementLens} t={t} />}
      {!loading && error === null && hour !== null && visibleSource === "events" && <EventsView cursor={cursor} data={data} loading={cursorState === "loading"} hour={hour} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={chooseCursor} onFinding={selectFinding} onOpenChart={openChart} onPattern={applyFind} onSelectedLane={setTimelineLane} onShowAll={() => { setEventScope(null); setSelectedFinding(null); setInspectorPanel(null) }} pattern={find} revision={refreshVersion} scope={eventScope} selected={selectedFinding} selectedLane={timelineLane} t={t} />}
    </section>

    {inspectorOpen && hour !== null && <Inspector
      chart={<Timeline cursor={cursor} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={chooseCursor} onFinding={selectFinding} onSelectedLane={setTimelineLane} presentation="inspector" primaryLane={timelinePrimary} selectedLane={timelineLane} shownAt={shownAt} t={t} />}
      detail={selectedProcess === null
        ? <div className="inspector-detail-slot" ref={setInspectorDetailRoot} />
        : <>
          <DetailDock activity={joinedActivity.row} cursor={cursor} hour={hour} lens={lens} locale={locale} onCursor={chooseCursor} process={selectedProcess} processHistory={processHistory.value?.length ? processHistory.value : [selectedProcess]} processHistoryStatus={processHistory.status} t={t} ticksPerSecond={ticksPerSecond} />
          {joinedActivity.row !== null && <InspectorRelatedPortal id="pg_stat_activity" identity={`process:${selectedPid ?? ""}`} label={t("pg.section.activity")}>
            <ActivityFacts activity={joinedActivity.row} activityTime={joinedActivity.snapshotTime} locale={locale} onRelated={openRelated} t={t} />
          </InspectorRelatedPortal>}
        </>}
      detailAvailable={detailAvailable}
      entityChart={<div className="inspector-chart-slot" ref={setInspectorChartRoot} />}
      entityChartAvailable={entityChartAvailable}
      onClose={closeInspector}
      onPanel={setInspectorPanel}
      panel={inspectorPanel ?? "chart"}
      t={t}
      title={selectedProcess === null ? inspectorDetailTitle ?? t("inspector.timeline") : `PID ${selectedPid ?? "—"}`}
    />}
    </div>
    </InspectorPortalProvider>

    {helpOpen && <HelpPanel items={helpItems} onClose={() => setHelpOpen(false)} t={t} />}
    {mcpOpen && <McpPanel database={database} onClose={() => setMcpOpen(false)} t={t} />}
  </main></DisplayTimeScope>
}

function TimeValue({ label, output, testId }: { readonly label: string; readonly output: string | null; readonly testId: string }) {
  return <span data-testid={testId}><b>{label}</b>{output ?? "—"}</span>
}

function UpdatedAge({ at, clock, locale, t }: { readonly at: number; readonly clock: string; readonly locale: Locale; readonly t: Translate }) {
  const [now, setNow] = useState(() => Date.now() * 1_000)
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now() * 1_000), 1_000)
    return () => clearInterval(timer)
  }, [])
  const age = humanAge((now - at) / 1_000_000, locale)
  return <span className="flex flex-none items-baseline text-xs text-fg4" data-testid="updated-time">
    <LabelHelp
      helpKey="refresh.updated"
      helpText={`${t("refresh.updated")} ${t("refresh.ago", { age })} · ${clock}`}
      iconOnly
      labelKey="refresh.updated"
      t={t}
      testId="updated-help"
    />
  </span>
}

const LOAD_SECONDS_KEY = "kronika.hourload-seconds"

function readLastLoadSeconds(): number | null {
  try {
    const stored = localStorage.getItem(LOAD_SECONDS_KEY)
    if (stored === null) return null
    const seconds = Number(stored)
    return Number.isFinite(seconds) && seconds >= 1 ? seconds : null
  } catch {
    return null
  }
}

function writeLastLoadSeconds(seconds: number): void {
  try {
    localStorage.setItem(LOAD_SECONDS_KEY, String(Math.round(seconds * 10) / 10))
  } catch {}
}

function StateCard({ message }: { readonly message: string }) {
  return <div className="grid min-h-[calc(100dvh-40px)] place-items-center p-4">
    <div className="w-full max-w-[620px] rounded-[var(--radius-md)] border border-line2 bg-s1 px-3 py-3" data-testid="state-card" role="alert">
      <div className="flex items-center gap-2 border-l-2 border-accent pl-2.5"><span className="text-xs uppercase tracking-[.1em] text-fg4">KRONIKA</span><h2 className="text-sm">{message}</h2></div>
    </div>
  </div>
}

function initialPageOptions(
  request: SectionRequest,
  context: EntityContext | null,
  pattern: string,
  relationFilters: Readonly<Record<string, string>>,
): SnapshotOptions {
  const filters = request.group === undefined
      ? context === null ? undefined : sortedFilters(Object.fromEntries(context.identity))
      : Object.keys(relationFilters).length === 0 && request.filters === undefined
        ? undefined
        : sortedFilters({ ...relationFilters, ...request.filters })
  return {
    ...(filters === undefined ? {} : { filters }),
    ...(request.group === undefined && context !== null ? { typeId: context.typeId } : {}),
    ...(pattern === "" ? {} : { search: pattern }),
  }
}

function snapshotTargetKey(
  groups: readonly SnapshotRequestGroup[],
  cursor: number,
  cgroupGroups: readonly SnapshotRequestGroup[],
  order: TableOrder | null,
  options: SnapshotOptions | undefined,
): string {
  const keyed = (group: SnapshotRequestGroup) => {
    const sections = new Set(group.requests.map(({ section }) => section))
    const layouts = group.anchor.sections
      .filter(({ logicalName }) => sections.has(logicalName))
      .map(({ logicalName, typeId }) => [logicalName, typeId] as const)
      .sort(([leftName, leftType], [rightName, rightType]) => leftName.localeCompare(rightName) || leftType.localeCompare(rightType))
    return [group.anchor.id, layouts, group.requests] as const
  }
  return JSON.stringify([
    cursor,
    groups.map(keyed),
    cgroupGroups.map(keyed),
    order === null ? null : [order.column, order.descending],
    options ?? null,
  ])
}

function sortedFilters(filters: Readonly<Record<string, string>>): Readonly<Record<string, string>> {
  return Object.fromEntries(Object.entries(filters).sort(([left], [right]) => left.localeCompare(right)))
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
    ["tablespace_oid", address.tablespaceOid],
  ].flatMap(([name, stored]) => stored === null ? [] : [[name, stored]]))
}

function relationSelectedKeyOf(address: ReturnType<typeof readAddress>): string | null {
  const section = pgSectionOf(address.view)
  return section === "tables" || section === "indexes" ? address.row : null
}

function searchRowsAvailable(data: HourData, surface: SearchSurface): boolean {
  if (surface === "os_process") return data.processes.length !== 0
  return (data.sections[surface]?.length ?? 0) !== 0
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
