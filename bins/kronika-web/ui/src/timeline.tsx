import { useEffect, useMemo, useRef, useState } from "react"

import { fieldNameForLocator, type DataRow, type Finding, type LanePoint } from "./api"
import { buildMetricSamples } from "./chart"
import { useDisplayTime } from "./display-time-context"
import { findingOrder, findingSummary } from "./finding-presentation"
import { LabelHelp, type Translate } from "./help"
import { keyboardTargetOwnsArrows, moveCursor, orderedRecordedTimes } from "./keyboard"
import { asNumber, compact, humanPercent, type Locale, value } from "./model"
import { emptyHourStatusKey } from "./refresh"
import { uncollectedStart } from "./series-chart"
import { UPlotChart, type ChartDecoration, type RecordedSeries } from "./uplot-chart"

export const MARKER_CLUSTER_PX = 88

interface SeriesPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export interface GroupedFinding {
  readonly composition: readonly { readonly count: number; readonly kind: Finding["kind"] }[]
  readonly findings: readonly Finding[]
}

interface TimelineSeries {
  readonly color: "cyan" | "amber" | "violet"
  readonly field: string
  readonly points: readonly SeriesPoint[]
}

interface TimelineLane {
  readonly key: string
  readonly series: readonly TimelineSeries[]
  readonly threshold?: number | undefined
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
  const time = useDisplayTime()
  const end = hour + 3_600_000_000
  const healthTrack = useMemo(() => healthTimelineSeries(health), [health])
  const lanes = useMemo<readonly TimelineLane[]>(() => {
    const of = (name: string) => lanePoints
      .filter((point) => point.lane === name)
      .map((point) => ({ segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }))
    const one = (color: TimelineSeries["color"], field: string, points: readonly SeriesPoint[]): readonly [TimelineSeries] => [{ color, field, points }]
    return [
      { key: "health", series: healthTrack.series, threshold: healthTrack.threshold },
      { key: "cpu_busy", series: one("cyan", "cpu_busy", of("cpu_busy")) },
      { key: "cpu_stall", series: one("amber", "cpu_stall", of("cpu_stall")) },
      { key: "memory", series: one("violet", "memory", of("memory")) },
      { key: "io_stall", series: one("cyan", "io_stall", of("io_stall")) },
      { key: "pg_running", series: one("cyan", "pg_running", of("pg_running")) },
      { key: "pg_waiting", series: one("amber", "pg_waiting", of("pg_waiting")) },
      { key: "oldest_xact", series: one("violet", "pg_oldest_xact", of("pg_oldest_xact")) },
    ].filter((lane) => lane.key === "health"
      ? lane.series.some((line) => line.points.length !== 0)
      : lane.series.some((line) => line.points.some((point) => point.value !== null)))
  }, [healthTrack, lanePoints])
  const [selectedLane, setSelectedLane] = useState(primaryLane)
  const previousPrimary = useRef(primaryLane)
  useEffect(() => {
    if (previousPrimary.current === primaryLane) return
    previousPrimary.current = primaryLane
    if (lanes.some((lane) => lane.key === primaryLane)) setSelectedLane(primaryLane)
  }, [lanes, primaryLane])
  useEffect(() => {
    if (lanes.some((lane) => lane.key === selectedLane)) return
    setSelectedLane(lanes.find((lane) => lane.key === primaryLane)?.key ?? lanes[0]?.key ?? "health")
  }, [lanes, primaryLane, selectedLane])
  const selected = lanes.find((lane) => lane.key === selectedLane) ?? lanes[0]
  const primaryTimes = useMemo(() => selectedTimelineTimes(lanes, selectedLane), [lanes, selectedLane])
  const [plotWidth, setPlotWidth] = useState(920)
  const markers = useMemo(() => groupFindings(findings, hour, end, plotWidth), [end, findings, hour, plotWidth])
  useEffect(() => {
    const move = (event: KeyboardEvent) => {
      if (event.defaultPrevented || (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
        || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey
        || keyboardTargetOwnsArrows(event.target) || primaryTimes.length === 0) return
      event.preventDefault()
      const timestamp = moveCursor(cursor, primaryTimes, event.key)
      if (timestamp !== cursor) onCursor(timestamp)
    }
    window.addEventListener("keydown", move)
    return () => window.removeEventListener("keydown", move)
  }, [cursor, onCursor, primaryTimes])
  const recorded = useMemo(() => selected === undefined ? [] : toRecordedSeries(selected, locale, t), [locale, selected, t])
  const healthAt = selected?.key === "health" ? healthEvaluationAtOrBefore(selected.series, cursor) : null
  const current = (selected?.series ?? []).map((line) => {
    const key = selected?.key ?? "health"
    const number = key === "health" ? healthAt === null ? null : exactValue(line.points, healthAt) : exactValue(line.points, cursor)
    return `${key === "health" ? `${t(`lane.health.${line.field}`)} ` : ""}${number === null ? "—" : format(number, key, locale)}`
  }).join(" · ")
  const decorations = useMemo(
    () => timelineDecorations(lanes, selected?.series ?? [], hour, end),
    [end, hour, lanes, selected],
  )
  const threshold = useMemo(() => selected?.threshold === undefined ? undefined : { below: selected.threshold, seriesId: "overall_health" }, [selected])
  const markerLayer = <>{markers.map((marker, index) => {
    const first = marker.findings[0]
    if (first === undefined) return null
    return <FindingMarker
      key={`${first.timestamp}:${first.kind}:${index}`}
      marker={marker}
      onActivate={() => {
        if (first.timestamp !== cursor) onCursor(first.timestamp)
        onFinding(first, marker.findings)
      }}
      share={shareOf(first.timestamp, hour, end)}
      t={t}
      time={time.timestamp}
    />
  })}</>
  if (selected === undefined) {
    return findings.length === 0
      ? <section className="timeline-empty" data-testid="timeline-empty">{t(emptyHourStatusKey(hour))}</section>
      : <section className="timeline-empty" data-testid="timeline-empty">{t("status.no_data")}</section>
  }
  return <section aria-label={t("hour.range", { range: time.hourRange(hour).primary })} className="timeline-shell">
    <div className="timeline-labels">
      {lanes.map((lane) => <LaneLabel
        help={`lane.${lane.key}.help`}
        key={lane.key}
        label={`lane.${lane.key}.label`}
        onSelect={() => setSelectedLane(lane.key)}
        primary={lane.key === selected.key}
        reading={laneReading(lane, cursor, locale, t)}
        t={t}
      />)}
    </div>
    <UPlotChart
      className="timeline-chart"
      cursor={cursor}
      decorations={decorations}
      hour={hour}
      locale={locale}
      markerLayer={markerLayer}
      onCursor={onCursor}
      onPlotWidth={setPlotWidth}
      reading={current}
      referenceTimestamp={shownAt ?? undefined}
      series={recorded}
      t={t}
      testId="hour-timeline"
      threshold={threshold}
    />
  </section>
}

export function timelineRecordedTimes(series: readonly { readonly points: readonly { readonly timestamp: number }[] }[]): readonly number[] {
  return orderedRecordedTimes(series.flatMap((line) => line.points.map((point) => point.timestamp)))
}

export function selectedTimelineTimes(
  lanes: readonly { readonly key: string; readonly series: readonly { readonly points: readonly { readonly timestamp: number }[] }[] }[],
  selectedLane: string,
): readonly number[] {
  return timelineRecordedTimes((lanes.find((lane) => lane.key === selectedLane) ?? lanes[0])?.series ?? [])
}

export function sampleWindow(lanes: readonly { readonly series: readonly { readonly points: readonly SeriesPoint[] }[] }[]): { readonly start: number; readonly end: number } | null {
  const timestamps = lanes.flatMap((lane) => lane.series)
    .flatMap((line) => line.points)
    .flatMap((point) => typeof point.value === "number" && Number.isFinite(point.value) ? [point.timestamp] : [])
  if (timestamps.length === 0) return null
  return { start: Math.min(...timestamps), end: Math.max(...timestamps) }
}

export function timelineDecorations(
  lanes: readonly { readonly series: readonly { readonly points: readonly SeriesPoint[] }[] }[],
  selected: readonly { readonly points: readonly SeriesPoint[] }[],
  hour: number,
  end: number,
  now = Date.now() * 1_000,
): readonly ChartDecoration[] {
  const available = sampleWindow(lanes)
  const output: ChartDecoration[] = []
  if (available !== null && available.start > hour) output.push({ from: hour, to: available.start, tone: "unavailable" })
  if (available !== null && available.end < end) output.push({ from: available.end, to: end, tone: "unavailable" })
  const future = uncollectedStart(selected.flatMap((line) => line.points), hour, now)
  if (future !== null && future < end) output.push({ from: future, to: end, tone: "future" })
  return output
}

function toRecordedSeries(lane: TimelineLane, locale: Locale, t: Translate): readonly RecordedSeries[] {
  const percent = ["health", "cpu_busy", "cpu_stall", "memory", "io_stall"].includes(lane.key)
  const unit = percent ? "%" : lane.key === "oldest_xact" ? (locale === "ru" ? "с" : "s") : (locale === "ru" ? "количество" : "count")
  return lane.series.map((line) => ({
    color: line.color,
    helpKey: `lane.${lane.key}.help`,
    id: line.field,
    label: lane.key === "health" ? t(`lane.health.${line.field}`) : t(`lane.${lane.key}.label`),
    labelKey: lane.key === "health" ? `lane.health.${line.field}` : `lane.${lane.key}.label`,
    points: line.points,
    scale: percent ? "percent" as const : "nonnegative" as const,
    tick: (number: number, place: Locale) => format(number, lane.key, place),
    unit,
    value: (number: number, place: Locale) => format(number, lane.key, place),
  }))
}

function LaneLabel({ label, help, onSelect, primary, reading, t }: { readonly label: string; readonly help: string; readonly onSelect: () => void; readonly primary: boolean; readonly reading: string; readonly t: Translate }) {
  return <div className={`lane-label lane-overview${primary ? " lane-primary" : ""}`}>
    <button aria-pressed={primary} className="lane-select" onClick={onSelect} type="button">
      <span className="lane-name">{t(label)}</span>
      <span className="lane-reading">{reading}</span>
    </button>
    <LabelHelp helpKey={help} iconOnly labelKey={label} t={t} />
  </div>
}

export function exactValue(points: readonly SeriesPoint[], cursor: number): number | null {
  const point = points.find((candidate) => candidate.timestamp === cursor)
  return point?.value ?? null
}

export function healthEvaluationAtOrBefore(
  series: readonly { readonly points: readonly { readonly timestamp: number }[] }[],
  cursor: number,
): number | null {
  let chosen: number | null = null
  for (const line of series) for (const point of line.points) {
    if (point.timestamp <= cursor && (chosen === null || point.timestamp > chosen)) chosen = point.timestamp
  }
  return chosen
}

function format(number: number, key: string, locale: Locale): string {
  if (key === "oldest_xact") return `${compact(number, locale)} ${locale === "ru" ? "с" : "s"}`
  if (key === "pg_running" || key === "pg_waiting") return compact(number, locale)
  return humanPercent(number, locale)
}

function laneReading(lane: TimelineLane, cursor: number, locale: Locale, t: Translate): string {
  const healthAt = lane.key === "health" ? healthEvaluationAtOrBefore(lane.series, cursor) : null
  return lane.series.map((line) => {
    const number = lane.key === "health" ? healthAt === null ? null : exactValue(line.points, healthAt) : exactValue(line.points, cursor)
    const output = number === null ? "—" : format(number, lane.key, locale)
    return lane.key === "health" ? `${t(`lane.health.${line.field}`)} ${output}` : output
  }).join(" · ")
}

export function FindingMarker({ marker, onActivate, share, t, time = String }: { readonly marker: GroupedFinding; readonly onActivate: () => void; readonly t: Translate; readonly share: number; readonly time?: (timestamp: number) => string }) {
  const activate = (event: { preventDefault(): void; stopPropagation(): void }) => {
    event.preventDefault()
    event.stopPropagation()
    onActivate()
  }
  const first = marker.findings[0]
  const last = marker.findings.at(-1)
  if (first === undefined || last === undefined) return null
  const count = marker.findings.length
  const kindSummary = findingSummary(marker.findings, t)
  const timeSummary = first.timestamp === last.timestamp ? time(first.timestamp) : `${time(first.timestamp)}–${time(last.timestamp)}`
  return <button
    aria-label={`${kindSummary} · ${timeSummary} · ×${count}`}
    className={`marker-button${count === 1 ? ` marker-${first.kind}` : " marker-aggregate"}`}
    data-marker-composition={marker.composition.map(({ count, kind }) => `${kind}:${count}`).join(" ")}
    data-marker-count={count}
    data-marker-kinds={marker.composition.map(({ kind }) => kind).join(" ")}
    onClick={activate}
    onKeyDown={(event) => {
      event.stopPropagation()
      if (event.key === "Enter" || event.key === " ") activate(event)
    }}
    onPointerDown={(event) => event.stopPropagation()}
    style={{ left: `clamp(${MARKER_CLUSTER_PX / 2}px, ${share * 100}%, calc(100% - ${MARKER_CLUSTER_PX / 2}px))` }}
    type="button"
  >
    {count === 1
      ? <FindingGlyph kind={first.kind} />
      : <span aria-hidden="true" className="marker-cluster-badge">
        <span className="marker-composition">{marker.composition.map(({ count, kind }) => <span className="marker-kind-count" key={kind}><FindingGlyph kind={kind} /><small>{count}</small></span>)}</span>
        <strong className="marker-count">{count}</strong>
      </span>}
  </button>
}

function series(rows: readonly DataRow[], field: string): readonly SeriesPoint[] {
  return preferredSeries(rows, [field])
}

function preferredSeries(rows: readonly DataRow[], fields: readonly string[]): readonly SeriesPoint[] {
  return buildMetricSamples(rows, (row) => {
    const field = fields.find((candidate) => Object.hasOwn(row.values, candidate))
    return field === undefined ? undefined : asNumber(value(row, field))
  })
}

export function healthTimelineSeries(rows: readonly DataRow[]): { readonly series: readonly TimelineSeries[]; readonly threshold?: number } {
  const candidates: readonly TimelineSeries[] = [
    { color: "cyan", field: "overall_health", points: series(rows, "overall_health") },
    { color: "amber", field: "os_health", points: series(rows, "os_health") },
    { color: "violet", field: "postgres_health", points: series(rows, "postgres_health") },
  ]
  const shown = candidates.filter((candidate) => candidate.points.length !== 0)
  return { series: shown, ...(shown.some((candidate) => candidate.field === "overall_health") ? { threshold: 50 } : {}) }
}

export function groupFindings(findings: readonly Finding[], hour: number, end: number, pixelWidth: number, clusterWidth = MARKER_CLUSTER_PX): readonly GroupedFinding[] {
  const duration = Math.max(1, end - hour)
  const width = Math.max(1, pixelWidth)
  const ordered = findings.slice().sort(findingOrder)
  const stored: Finding[][] = []
  let active: Finding[] = []
  let anchor = 0
  for (const finding of ordered) {
    const edge = Math.min(width / 2, clusterWidth / 2)
    const x = Math.max(edge, Math.min(width - edge, (finding.timestamp - hour) / duration * width))
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
  return stored.map((group) => ({
    composition: FINDING_KINDS.flatMap((kind) => {
      const count = group.filter((finding) => finding.kind === kind).length
      return count === 0 ? [] : [{ count, kind }]
    }),
    findings: group,
  }))
}

const FINDING_KINDS = ["event", "known_bad", "spike"] as const satisfies readonly Finding["kind"][]

export function findingTrack(finding: Finding): string | null {
  if (finding.kind === "event") return null
  const field = fieldNameForLocator(finding)
  if (finding.logicalName === "health" && field === "overall_health") return "health"
  if (finding.logicalName === "os_meminfo" && field === "mem_available") return "memory"
  return null
}

export function findingShape(kind: Finding["kind"]): FindingShape {
  if (kind === "known_bad") return "diamond"
  if (kind === "spike") return "triangle"
  return "circle"
}

function FindingGlyph({ kind }: { readonly kind: Finding["kind"] }) {
  if (kind === "known_bad") return <svg aria-hidden="true" data-marker-shape="diamond" height="11" viewBox="0 0 12 12" width="11"><path d="M6 1 11 6 6 11 1 6Z" fill="var(--bad)" stroke="var(--bad-edge)" /></svg>
  if (kind === "spike") return <svg aria-hidden="true" data-marker-shape="triangle" height="11" viewBox="0 0 12 12" width="12"><path d="M6 1 11 10.5H1Z" fill="none" stroke="var(--warn)" strokeWidth="1.5" /></svg>
  return <svg aria-hidden="true" data-marker-shape="circle" height="10" viewBox="0 0 12 12" width="10"><circle cx="6" cy="6" fill="var(--event)" r="4.5" stroke="var(--event-edge)" /></svg>
}

export function healthThreshold(field: string): number | null {
  return field === "overall_health" ? 50 : null
}

function shareOf(timestamp: number, hour: number, end: number): number {
  return Math.max(0, Math.min(1, (timestamp - hour) / (end - hour)))
}
