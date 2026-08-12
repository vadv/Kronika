import { useEffect, useMemo, useRef, useState } from "react"

import type { DataRow, Finding, LanePoint } from "./api"
import { niceCeiling, numericRuns, svgPath, type NumericPoint } from "./chart"
import { LabelHelp, type Translate } from "./help"
import { moveCursor } from "./keyboard"
import { asNumber, compact, formatUtc, humanBytes, type Locale, value } from "./model"
import { TimeTicks } from "./time-ticks"

const WIDTH = 920
const TICK_ROW = 18
const PRIMARY_HEIGHT = 132
const OVERVIEW_HEIGHT = 27
const MARKER_RAIL = 14
const NEUTRAL_RAIL = 14
const TOP = MARKER_RAIL + NEUTRAL_RAIL + 8
const MARKER_CLUSTER_PX = 38
const MARKER_RAIL_Y = MARKER_RAIL / 2
const NEUTRAL_RAIL_Y = MARKER_RAIL + NEUTRAL_RAIL / 2
const SHARE: readonly [number, number] = [0, 100]

interface SeriesPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export interface GroupedFinding {
  readonly count: number
  readonly finding: Finding
  readonly findings: readonly Finding[]
  readonly kind: Finding["kind"]
  readonly kinds: readonly Finding["kind"][]
  readonly startTimestamp: number
  readonly endTimestamp: number
  readonly timestamp: number
  readonly placement: "event" | "neutral" | "track"
  readonly track: string | null
}

interface TimelineSeries {
  readonly color: "cyan" | "amber" | "violet"
  readonly points: readonly SeriesPoint[]
}

interface TimelineLane {
  readonly domain?: readonly [number, number] | undefined
  readonly key: string
  readonly minimumSpan?: number | undefined
  readonly series: readonly TimelineSeries[]
  readonly threshold?: number | undefined
}

interface DisplayedLane extends TimelineLane {
  readonly height: number
  readonly primary: boolean
  readonly top: number
}

export type FindingShape = "circle" | "diamond" | "triangle"

export function Timeline({
  cursor,
  findings,
  health,
  hour,
  lanePoints,
  locale,
  onCursor,
  onFinding,
  primaryLane = "health",
  shownAt,
  t,
}: {
  readonly cursor: number
  readonly findings: readonly Finding[]
  readonly health: readonly DataRow[]
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding, grouped?: readonly Finding[]) => void
  readonly primaryLane?: string | undefined
  readonly shownAt?: number | null
  readonly t: Translate
}) {
  const plot = useRef<HTMLDivElement>(null)
  const [plotWidth, setPlotWidth] = useState(WIDTH)
  const [hover, setHover] = useState<number | null>(null)
  const [selectedLane, setSelectedLane] = useState(primaryLane)
  const end = hour + 3_600_000_000
  const healthTrack = useMemo(() => {
    const overall = series(health, "overall_health")
    if (overall.some((point) => point.value !== null)) {
      return { points: overall, threshold: healthThreshold("overall_health") ?? undefined }
    }
    return { points: series(health, "os_health"), threshold: undefined }
  }, [health])
  const lanes = useMemo<readonly TimelineLane[]>(() => {
    const of = (name: string) => lanePoints
      .filter((point) => point.lane === name)
      .map((point) => ({ segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }))
    const one = (color: TimelineSeries["color"], points: readonly SeriesPoint[]): readonly [TimelineSeries] => [{ color, points }]
    return [
      { domain: SHARE, key: "health", series: one("cyan", healthTrack.points), threshold: healthTrack.threshold },
      { domain: SHARE, key: "cpu_busy", series: one("cyan", of("cpu_busy")) },
      { domain: SHARE, key: "cpu_stall", series: one("amber", of("cpu_stall")) },
      { domain: SHARE, key: "memory", series: one("violet", of("memory")) },
      { domain: SHARE, key: "io_stall", series: one("cyan", of("io_stall")) },
      { domain: undefined, key: "pg_running", minimumSpan: 5, series: one("cyan", of("pg_running")) },
      { domain: undefined, key: "pg_waiting", minimumSpan: 5, series: one("amber", of("pg_waiting")) },
      { domain: undefined, key: "oldest_xact", series: one("violet", of("pg_oldest_xact")) },
    ].filter((lane) => lane.series.some((series) => series.points.some((point) => point.value !== null)))
  }, [healthTrack, lanePoints])
  useEffect(() => {
    if (lanes.some((lane) => lane.key === primaryLane)) setSelectedLane(primaryLane)
  }, [lanes, primaryLane])
  useEffect(() => {
    if (lanes.some((lane) => lane.key === selectedLane)) return
    setSelectedLane(lanes[0]?.key ?? "health")
  }, [lanes, selectedLane])
  const displayed = useMemo<readonly DisplayedLane[]>(() => {
    const primary = lanes.find((lane) => lane.key === selectedLane) ?? lanes[0]
    if (primary === undefined) return []
    const overview = lanes.filter((lane) => lane.key !== primary.key).slice(0, overviewLaneCount(plotWidth))
    let top = TOP
    return [primary, ...overview].map((lane, index) => {
      const height = index === 0 ? PRIMARY_HEIGHT : OVERVIEW_HEIGHT
      const displayedLane = { ...lane, height, primary: index === 0, top }
      top += height
      return displayedLane
    })
  }, [lanes, plotWidth, selectedLane])
  const plotBottom = displayed.at(-1) === undefined
    ? TOP + PRIMARY_HEIGHT
    : (displayed.at(-1)?.top ?? TOP) + (displayed.at(-1)?.height ?? PRIMARY_HEIGHT)
  const height = plotBottom + TICK_ROW
  const window = sampleWindow(lanes)
  const markers = useMemo(
    () => groupFindings(findings, hour, end, plotWidth),
    [end, findings, hour, plotWidth],
  )
  useEffect(() => {
    const element = plot.current
    if (element === null) return
    const update = () => setPlotWidth(Math.max(1, element.clientWidth))
    update()
    if (typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(update)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  if (lanes.length === 0 && findings.length === 0) {
    return <section className="timeline-empty" data-testid="timeline-empty">{t("status.no_data")}</section>
  }
  const timestampFromClient = (clientX: number): number | null => {
    const bounds = plot.current?.getBoundingClientRect()
    if (bounds === undefined) return null
    const ratio = Math.max(0, Math.min(1, (clientX - bounds.left) / bounds.width))
    return Math.min(end - 1_000, Math.round(hour + ratio * (end - hour)))
  }
  const commitFromClient = (clientX: number) => {
    const timestamp = timestampFromClient(clientX)
    if (timestamp !== null) onCursor(timestamp)
  }
  const cursorX = shareOf(cursor, hour, end) * plotWidth
  const shownX = shownAt === undefined || shownAt === null ? null : shareOf(shownAt, hour, end) * plotWidth
  const hoverX = hover === null ? null : shareOf(hover, hour, end) * plotWidth
  return (
    <section
      aria-label={t("hour.range", { start: formatUtc(hour).slice(11, 16), end: formatUtc(end).slice(11, 16) })}
      className="timeline-shell"
      style={{ minHeight: `${height + 8}px` }}
    >
      <div
        aria-hidden="false"
        className="timeline-labels"
        style={{ gridTemplateRows: displayed.map((lane) => `${lane.height}px`).join(" ") }}
      >
        {displayed.map((lane) => <LaneLabel
          help={`lane.${lane.key}.help`}
          key={lane.key}
          label={`lane.${lane.key}.label`}
          onSelect={lane.primary ? undefined : () => setSelectedLane(lane.key)}
          primary={lane.primary}
          reading={lane.series.map((series) => valueAt(series.points, cursor)).map((number) => number === null ? "—" : format(number, lane.key, locale)).join(" · ")}
          t={t}
        />)}
      </div>
      <div className="timeline-plot" ref={plot} style={{ height: `${height}px`, position: "relative" }}>
        <div
          aria-label={t("hour.cursor", { time: formatUtc(cursor) })}
          aria-valuemax={end - 1_000}
          aria-valuemin={hour}
          aria-valuenow={cursor}
          aria-valuetext={formatUtc(cursor)}
          data-testid="hour-timeline"
          onKeyDown={(event) => {
            if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return
            event.preventDefault()
            onCursor(moveCursor(cursor, hour, event.key))
          }}
          onPointerDown={(event) => {
            event.currentTarget.setPointerCapture(event.pointerId)
            const timestamp = timestampFromClient(event.clientX)
            setHover(timestamp)
            if (timestamp !== null) onCursor(timestamp)
          }}
          onPointerMove={(event) => {
            setHover(timestampFromClient(event.clientX))
            if (event.buttons === 1) commitFromClient(event.clientX)
          }}
          onPointerLeave={() => setHover(null)}
          onPointerUp={(event) => event.currentTarget.releasePointerCapture(event.pointerId)}
          role="slider"
          style={{ height: `${height}px` }}
          tabIndex={0}
        >
          <svg aria-hidden="true" preserveAspectRatio="none" style={{ height: `${height}px` }} viewBox={`0 0 ${plotWidth} ${height}`}>
            {window !== null && window.start > hour && <rect
              className="data-unavailable"
              height={plotBottom - TOP}
              width={shareOf(window.start, hour, end) * plotWidth}
              x={0}
              y={TOP}
            />}
            {window !== null && window.end < end && <rect
              className="data-unavailable"
              height={plotBottom - TOP}
              width={(1 - shareOf(window.end, hour, end)) * plotWidth}
              x={shareOf(window.end, hour, end) * plotWidth}
              y={TOP}
            />}
            <line className="finding-rail event-rail" x1={0} x2={plotWidth} y1={MARKER_RAIL_Y} y2={MARKER_RAIL_Y} />
            <line className="finding-rail neutral-rail" x1={0} x2={plotWidth} y1={NEUTRAL_RAIL_Y} y2={NEUTRAL_RAIL_Y} />
            {[0, 1, 2, 3, 4, 5, 6].map((tick) => {
              const x = tick / 6 * plotWidth
              return <line className="timeline-grid" key={tick} x1={x} x2={x} y1={0} y2={plotBottom} />
            })}
            {displayed.map((lane) => {
              return <line className="lane-line" key={lane.key} x1={0} x2={plotWidth} y1={lane.top} y2={lane.top} />
            })}
            <line className="lane-line" x1={0} x2={plotWidth} y1={plotBottom} y2={plotBottom} />
            {displayed.map((lane) => {
              const range = laneRange(lane)
              const thresholdY = lane.threshold === undefined ? null : seriesY(lane.threshold, lane.top, lane.height, range.low, range.span)
              const floorY = seriesY(range.low, lane.top, lane.height, range.low, range.span)
              return <g key={lane.key}>
                {thresholdY !== null && <rect className="threshold-band" height={Math.max(0, floorY - thresholdY)} width={plotWidth} x={0} y={thresholdY} />}
                {lane.primary && [0.25, 0.5, 0.75].map((part) => {
                  const y = seriesY(range.low + range.span * part, lane.top, lane.height, range.low, range.span)
                  return <line className="primary-grid" key={part} x1={0} x2={plotWidth} y1={y} y2={y} />
                })}
                {lane.series.map((series, ordinal) => (
                  <SeriesLine color={series.color} end={end} height={lane.height} hour={hour} key={`${lane.key}:${ordinal}`} points={series.points} primary={lane.primary} range={range} top={lane.top} width={plotWidth} />
                ))}
              </g>
            })}
            {shownX !== null && Math.abs(shownX - cursorX) > 1
              && <line className="shown-line" x1={shownX} x2={shownX} y1={0} y2={plotBottom}><title>{t("hour.shown", { time: formatUtc(shownAt ?? 0) })}</title></line>}
            <line className="cursor-line" x1={cursorX} x2={cursorX} y1={0} y2={plotBottom} />
            {hoverX !== null && <line className="hover-line" x1={hoverX} x2={hoverX} y1={0} y2={plotBottom} />}
          </svg>
          {displayed.filter((lane) => lane.primary).map((lane) => {
            const range = laneRange(lane)
            return <div className="primary-scale" key={lane.key} style={{ height: `${lane.height - 12}px`, top: `${lane.top + 6}px` }}>
              <span>{format(range.low + range.span, lane.key, locale)}</span>
              <span>{format(range.low, lane.key, locale)}</span>
            </div>
          })}
          {hover !== null && <div className="timeline-hover" style={{ left: `${Math.max(6, Math.min(94, shareOf(hover, hour, end) * 100))}%` }}>
            <time>{formatUtc(hover)}</time>
            {displayed.map((lane) => {
              const reading = lane.series.map((series) => valueAt(series.points, hover)).map((number) => number === null ? "—" : format(number, lane.key, locale)).join(" · ")
              return <span key={lane.key}>{t(`lane.${lane.key}.label`)} <strong>{reading}</strong></span>
            })}
          </div>}
          <TimeTicks className="timeline-time-ticks" hour={hour} />
        </div>
        {markers.map((marker, index) => {
          const share = shareOf(marker.timestamp, hour, end)
          const lane = marker.track === null ? undefined : displayed.find((candidate) => candidate.key === marker.track)
          const y = marker.placement === "event"
            ? MARKER_RAIL_Y
            : lane === undefined ? NEUTRAL_RAIL_Y : findingY(marker.finding, lane)
          return <FindingMarker
            key={`${marker.timestamp}:${marker.kind}:${index}`}
            marker={marker}
            onActivate={() => {
              onCursor(marker.timestamp)
              onFinding(marker.finding, marker.findings)
            }}
            t={t}
            share={share}
            y={y ?? NEUTRAL_RAIL_Y}
          />
        })}
      </div>
    </section>
  )
}

function LaneLabel({ label, help, onSelect, primary, reading, t }: { readonly label: string; readonly help: string; readonly onSelect?: (() => void) | undefined; readonly primary: boolean; readonly reading: string; readonly t: Translate }) {
  const content = <>
    <LabelHelp helpKey={help} labelKey={label} t={t} />
    <span className="lane-reading">{reading}</span>
  </>
  return primary
    ? <div className="lane-label lane-primary">{content}</div>
    : <button className="lane-label lane-overview" onClick={onSelect} type="button">{content}</button>
}

export function valueAt(points: readonly SeriesPoint[], cursor: number): number | null {
  let chosen: SeriesPoint | null = null
  for (const point of points) {
    if (point.timestamp <= cursor && (chosen === null || point.timestamp > chosen.timestamp)) chosen = point
  }
  return chosen?.value ?? null
}

function format(number: number, key: string, locale: Locale): string {
  if (key === "oldest_xact") return `${compact(number, locale)} s`
  if (key === "backends" || key === "pg_running" || key === "pg_waiting") return compact(number, locale)
  return `${compact(number, locale)}%`
}

function FindingMarker({
  marker,
  onActivate,
  share,
  t,
  y,
}: {
  readonly marker: GroupedFinding
  readonly onActivate: () => void
  readonly t: Translate
  readonly share: number
  readonly y: number
}) {
  const activate = (event: { preventDefault(): void; stopPropagation(): void }) => {
    event.preventDefault()
    event.stopPropagation()
    onActivate()
  }
  const kindSummary = marker.kinds.map((kind) => {
    const count = marker.findings.filter((finding) => finding.kind === kind).length
    return `${t(`locator.${kind}`)} ×${count}`
  }).join(" · ")
  const timeSummary = marker.startTimestamp === marker.endTimestamp
    ? formatUtc(marker.startTimestamp)
    : `${formatUtc(marker.startTimestamp)}–${formatUtc(marker.endTimestamp)}`
  return <button
    aria-label={`${kindSummary} · ${timeSummary} · ×${marker.count}`}
    className={`marker-button${marker.count === 1 ? ` marker-${marker.kind}` : " marker-aggregate"}`}
    data-marker-count={marker.count}
    data-marker-kinds={marker.kinds.join(" ")}
    onClick={activate}
    onKeyDown={(event) => {
      event.stopPropagation()
      if (event.key === "Enter" || event.key === " ") activate(event)
    }}
    onPointerDown={(event) => event.stopPropagation()}
    style={{
      alignItems: "center",
      background: "transparent",
      border: 0,
      cursor: "pointer",
      display: "flex",
      height: "30px",
      justifyContent: "center",
      left: `${share * 100}%`,
      padding: 0,
      position: "absolute",
      top: `${y}px`,
      transform: "translate(-50%, -50%)",
      width: "34px",
      zIndex: 2,
    }}
    title={`${kindSummary} · ${timeSummary} · ×${marker.count}`}
    type="button"
  >
    <span aria-hidden="true" className="marker-shape-stack">
      {marker.kinds.map((kind) => <FindingGlyph key={kind} kind={kind} />)}
    </span>
    {marker.count > 1 && <span aria-hidden="true" className="marker-count">{marker.count}</span>}
  </button>
}

function SeriesLine({
  color,
  end,
  height,
  hour,
  points,
  primary,
  range,
  top,
  width,
}: {
  readonly color: "cyan" | "amber" | "violet"
  readonly end: number
  readonly height: number
  readonly hour: number
  readonly points: readonly SeriesPoint[]
  readonly primary: boolean
  readonly range: { readonly low: number; readonly span: number }
  readonly top: number
  readonly width: number
}) {
  if (points.length === 0) return null
  return <>{[...timelineRuns(points).entries()].map(([runId, stored]) => {
    const path = svgPath(stored.slice().sort((left, right) => left.timestamp - right.timestamp), (point) => [
      shareOf(point.timestamp, hour, end) * width,
      seriesY(point.value, top, height, range.low, range.span),
    ])
    const area = primary && stored.length > 1
      ? `${path} L${(shareOf(stored.at(-1)?.timestamp ?? hour, hour, end) * width).toFixed(2)} ${(top + height - 6).toFixed(2)} L${(shareOf(stored[0]?.timestamp ?? hour, hour, end) * width).toFixed(2)} ${(top + height - 6).toFixed(2)} Z`
      : null
    const singleton = stored.length === 1 ? stored[0] : undefined
    return <g key={runId}>
      {area !== null && <path className={`series-area area-${color}`} d={area} />}
      {singleton === undefined
        ? <path className={`series-line series-${color}${primary ? " series-primary" : " series-overview"}`} d={path} />
        : <circle
          className={`series-dot series-${color}`}
          cx={shareOf(singleton.timestamp, hour, end) * width}
          cy={seriesY(singleton.value, top, height, range.low, range.span)}
          r={primary ? 2 : 1.4}
        />}
    </g>
  })}</>
}

function series(rows: readonly DataRow[], field: string): readonly SeriesPoint[] {
  return preferredSeries(rows, [field])
}

function preferredSeries(rows: readonly DataRow[], fields: readonly string[]): readonly SeriesPoint[] {
  return rows.map((row) => {
    const number = fields.reduce<number | null>((selected, field) => selected ?? asNumber(value(row, field)), null)
    return { segmentId: row.segmentId, timestamp: row.timestamp, value: number }
  })
}

export function timelineRuns(points: readonly SeriesPoint[]): ReadonlyMap<string, readonly NumericPoint<SeriesPoint>[]> {
  return numericRuns(points, textOrder)
}

export function sampleWindow(lanes: readonly { readonly series: readonly { readonly points: readonly SeriesPoint[] }[] }[]): { readonly start: number; readonly end: number } | null {
  const timestamps = lanes.flatMap((lane) => lane.series)
    .flatMap((series) => series.points)
    .flatMap((point) => point.value === null || !Number.isFinite(point.value) ? [] : [point.timestamp])
  if (timestamps.length === 0) return null
  return { start: Math.min(...timestamps), end: Math.max(...timestamps) }
}

export function groupFindings(
  findings: readonly Finding[],
  hour: number,
  end: number,
  pixelWidth: number,
  clusterWidth = MARKER_CLUSTER_PX,
): readonly GroupedFinding[] {
  const duration = Math.max(1, end - hour)
  const width = Math.max(1, pixelWidth)
  const buckets = new Map<string, { readonly placement: GroupedFinding["placement"]; readonly track: string | null; readonly findings: Finding[] }>()
  for (const finding of findings.slice().sort(findingOrder)) {
    const track = findingTrack(finding)
    const placement = finding.kind === "event" ? "event" : track === null ? "neutral" : "track"
    const key = placement === "track" ? `track:${track}` : placement
    const bucket = buckets.get(key) ?? { placement, track, findings: [] }
    bucket.findings.push(finding)
    buckets.set(key, bucket)
  }
  const grouped: GroupedFinding[] = []
  for (const bucket of buckets.values()) {
    const stored: Finding[][] = []
    let active: Finding[] = []
    let anchor = 0
    for (const finding of bucket.findings) {
      const x = Math.max(0, Math.min(width, (finding.timestamp - hour) / duration * width))
      if (active.length === 0 || x - anchor <= clusterWidth) {
        if (active.length === 0) anchor = x
        active.push(finding)
      } else {
        stored.push(active)
        active = [finding]
        anchor = x
      }
    }
    if (active.length !== 0) stored.push(active)
    for (const group of stored) {
      const first = group[0]
      const last = group.at(-1)
      if (first === undefined || last === undefined) throw new Error("empty marker cluster")
      const kinds = FINDING_KINDS.filter((kind) => group.some((finding) => finding.kind === kind))
      grouped.push({
        count: group.length,
        finding: first,
        findings: group,
        kind: first.kind,
        kinds,
        placement: bucket.placement,
        startTimestamp: first.timestamp,
        endTimestamp: last.timestamp,
        timestamp: first.timestamp,
        track: bucket.track,
      })
    }
  }
  return grouped.sort((left, right) => left.timestamp - right.timestamp
    || placementOrder(left.placement) - placementOrder(right.placement)
    || textOrder(left.track ?? "", right.track ?? "")
    || findingOrder(left.finding, right.finding))
}

const FINDING_KINDS = ["event", "known_bad", "spike"] as const satisfies readonly Finding["kind"][]

function findingOrder(left: Finding, right: Finding): number {
  return left.timestamp - right.timestamp
    || FINDING_KINDS.indexOf(left.kind) - FINDING_KINDS.indexOf(right.kind)
    || textOrder(left.segmentId, right.segmentId)
    || textOrder(left.typeId, right.typeId)
    || textOrder(left.rowOrdinal, right.rowOrdinal)
    || left.fieldOrdinal - right.fieldOrdinal
    || textOrder(left.logicalName, right.logicalName)
}

function textOrder(left: string, right: string): number { return left < right ? -1 : left > right ? 1 : 0 }

function placementOrder(placement: GroupedFinding["placement"]): number {
  return placement === "event" ? 0 : placement === "track" ? 1 : 2
}

export function findingTrack(finding: Finding): string | null {
  if (finding.kind === "event") return null
  if (finding.logicalName === "health" && finding.typeId === "0" && finding.fieldOrdinal === 1) return "health"
  if (finding.logicalName === "os_meminfo" && finding.fieldOrdinal === 2) return "memory"
  return null
}

export function findingShape(kind: Finding["kind"]): FindingShape {
  if (kind === "known_bad") return "diamond"
  if (kind === "spike") return "triangle"
  return "circle"
}

function FindingGlyph({ kind }: { readonly kind: Finding["kind"] }) {
  if (kind === "known_bad") return <svg data-marker-shape="diamond" height="11" viewBox="0 0 12 12" width="11"><path d="M6 1 11 6 6 11 1 6Z" fill="var(--bad)" stroke="var(--bad-edge)" /></svg>
  if (kind === "spike") return <svg data-marker-shape="triangle" height="11" viewBox="0 0 12 12" width="12"><path d="M6 1 11 10.5H1Z" fill="none" stroke="var(--warn)" strokeWidth="1.5" /></svg>
  return <svg data-marker-shape="circle" height="10" viewBox="0 0 12 12" width="10"><circle cx="6" cy="6" fill="var(--event)" r="4.5" stroke="var(--event-edge)" /></svg>
}

export function seriesYAt(points: readonly SeriesPoint[], segmentId: string, timestamp: number, lane = 0): number | null {
  const range = laneRange({ series: [{ points }] })
  const point = points.find((candidate) => candidate.segmentId === segmentId
    && candidate.timestamp === timestamp && candidate.value !== null && Number.isFinite(candidate.value))
  if (point?.value === null || point?.value === undefined) return null
  const top = lane === 0 ? TOP : TOP + PRIMARY_HEIGHT + (lane - 1) * OVERVIEW_HEIGHT
  const height = lane === 0 ? PRIMARY_HEIGHT : OVERVIEW_HEIGHT
  return seriesY(point.value, top, height, range.low, range.span)
}

function findingY(finding: Finding, lane: DisplayedLane): number | null {
  const range = laneRange(lane)
  for (const series of lane.series) {
    const point = series.points.find((candidate) => candidate.segmentId === finding.segmentId
      && candidate.timestamp === finding.timestamp && candidate.value !== null && Number.isFinite(candidate.value))
    if (point?.value !== null && point?.value !== undefined) {
      return seriesY(point.value, lane.top, lane.height, range.low, range.span)
    }
  }
  return null
}

export function laneRange(
  lane: { readonly domain?: readonly [number, number] | undefined; readonly minimumSpan?: number | undefined; readonly series: readonly { readonly points: readonly SeriesPoint[] }[] },
): { readonly low: number; readonly span: number } {
  if (lane.domain !== undefined) return { low: lane.domain[0], span: lane.domain[1] - lane.domain[0] }
  const values = lane.series.flatMap((series) => series.points)
    .flatMap((point) => point.value === null || !Number.isFinite(point.value) ? [] : [point.value])
  const minimum = lane.minimumSpan ?? 1
  if (values.length === 0) return { low: 0, span: minimum }
  return { low: 0, span: Math.max(minimum, niceCeiling(Math.max(...values, 0))) }
}

export function healthThreshold(field: string): number | null {
  return field === "overall_health" ? 50 : null
}

export function overviewLaneCount(width: number): 2 | 3 | 4 {
  if (width < 720) return 2
  if (width < 1_040) return 3
  return 4
}

function seriesY(number: number, top: number, height: number, low: number, span: number): number {
  const laneBottom = top + height - 6
  return laneBottom - (number - low) / span * (height - 12)
}

function shareOf(timestamp: number, hour: number, end: number): number {
  return Math.max(0, Math.min(1, (timestamp - hour) / (end - hour)))
}
