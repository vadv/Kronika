import { useEffect, useMemo, useRef, useState } from "react"

import type { DataRow, Finding } from "./api"
import { LabelHelp, type Translate } from "./help"
import { asNumber, formatUtc, value } from "./model"
import { TimeTicks } from "./time-ticks"

const WIDTH = 920
const HEIGHT = 170
const LANE_HEIGHT = 36
const TOP = 8
const MARKER_CLUSTER_PX = 38

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
}

export type FindingShape = "circle" | "diamond" | "triangle"

export function Timeline({
  cursor,
  findings,
  health,
  hour,
  load,
  memory,
  onCursor,
  onFinding,
  pressure,
  t,
}: {
  readonly cursor: number
  readonly findings: readonly Finding[]
  readonly health: readonly DataRow[]
  readonly hour: number
  readonly load: readonly DataRow[]
  readonly memory: readonly DataRow[]
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding, grouped?: readonly Finding[]) => void
  readonly pressure: readonly DataRow[]
  readonly t: Translate
}) {
  const plot = useRef<HTMLDivElement>(null)
  const [plotWidth, setPlotWidth] = useState(WIDTH)
  const end = hour + 3_600_000_000
  const healthPoints = useMemo(() => series(health, "os_health"), [health])
  const loadPoints = useMemo(() => series(load, "load1"), [load])
  const memoryPoints = useMemo(
    () => preferredSeries(memory, ["mem_available_percent", "mem_available"]),
    [memory],
  )
  const pressurePoints = useMemo(() => [0, 1, 2].map((resource) => series(
    pressure.filter((row) => asNumber(value(row, "resource")) === resource),
    "some_avg10",
  )), [pressure])
  // A labelled lane with nothing in it is a line of chrome that says nothing.
  const lanes = useMemo(() => [
    { key: "health", series: [{ color: "cyan" as const, points: healthPoints }] },
    { key: "load", series: [{ color: "amber" as const, points: loadPoints }] },
    { key: "memory", series: [{ color: "violet" as const, points: memoryPoints }] },
    {
      key: "pressure",
      series: [
        { color: "cyan" as const, points: pressurePoints[0] ?? [] },
        { color: "violet" as const, points: pressurePoints[1] ?? [] },
        { color: "amber" as const, points: pressurePoints[2] ?? [] },
      ],
    },
  ].filter((lane) => lane.series.some((series) => series.points.some((point) => point.value !== null))),
  [healthPoints, loadPoints, memoryPoints, pressurePoints])
  const healthLane = lanes.findIndex((lane) => lane.key === "health")
  const plotBottom = TOP + Math.max(1, lanes.length) * LANE_HEIGHT
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
  const setFromClient = (clientX: number) => {
    const bounds = plot.current?.getBoundingClientRect()
    if (bounds === undefined) return
    const ratio = Math.max(0, Math.min(1, (clientX - bounds.left) / bounds.width))
    onCursor(Math.min(end - 1_000, Math.round(hour + ratio * (end - hour))))
  }
  const cursorX = scaleX(cursor, hour, end)
  return (
    <section
      aria-label={t("hour.range", { start: formatUtc(hour).slice(11, 16), end: formatUtc(end).slice(11, 16) })}
      className="timeline-shell"
      style={{ minHeight: `${HEIGHT + 18}px` }}
    >
      <div
        aria-hidden="false"
        className="timeline-labels"
        style={{ gridTemplateRows: `repeat(${Math.max(1, lanes.length)}, ${LANE_HEIGHT}px)` }}
      >
        {lanes.map((lane) => <LaneLabel help={`lane.${lane.key}.help`} key={lane.key} label={`lane.${lane.key}.label`} t={t} />)}
      </div>
      <div className="timeline-plot" ref={plot} style={{ height: `${HEIGHT}px`, position: "relative" }}>
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
            const direction = event.key === "ArrowLeft" ? -1 : 1
            onCursor(Math.max(hour, Math.min(end - 1_000, cursor + direction * 60_000_000)))
          }}
          onPointerDown={(event) => setFromClient(event.clientX)}
          role="slider"
          style={{ height: `${HEIGHT}px` }}
          tabIndex={0}
        >
          <svg aria-hidden="true" preserveAspectRatio="none" style={{ height: `${HEIGHT}px` }} viewBox={`0 0 ${WIDTH} ${HEIGHT}`}>
            {[0, 1, 2, 3, 4, 5, 6].map((tick) => {
              const x = tick / 6 * WIDTH
              return <line className="timeline-grid" key={tick} x1={x} x2={x} y1={0} y2={plotBottom} />
            })}
            {lanes.map((_lane, index) => {
              const y = TOP + index * LANE_HEIGHT
              return <line className="lane-line" key={y} x1={0} x2={WIDTH} y1={y} y2={y} />
            })}
            {lanes.flatMap((lane, index) => lane.series.map((series, ordinal) => (
              <SeriesLine color={series.color} end={end} hour={hour} key={`${lane.key}:${ordinal}`} lane={index} points={series.points} />
            )))}
            <line className="cursor-line" x1={cursorX} x2={cursorX} y1={0} y2={plotBottom} />
          </svg>
          <TimeTicks className="timeline-time-ticks" hour={hour} />
        </div>
        {markers.map((marker, index) => {
          const x = scaleX(marker.timestamp, hour, end)
          const displayTimestamp = hour + x / WIDTH * (end - hour)
          const y = seriesYAt(healthPoints, marker.finding.segmentId, displayTimestamp, Math.max(0, healthLane))
          return <FindingMarker
            key={`${marker.timestamp}:${marker.kind}:${index}`}
            marker={marker}
            onActivate={() => {
              onCursor(marker.timestamp)
              onFinding(marker.finding, marker.findings)
            }}
            t={t}
            x={x}
            y={y}
          />
        })}
      </div>
    </section>
  )
}

function LaneLabel({ label, help, t }: { readonly label: string; readonly help: string; readonly t: Translate }) {
  return <div className="lane-label"><LabelHelp helpKey={help} labelKey={label} t={t} /></div>
}

function FindingMarker({
  marker,
  onActivate,
  t,
  x,
  y,
}: {
  readonly marker: GroupedFinding
  readonly onActivate: () => void
  readonly t: Translate
  readonly x: number
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
      left: `${x / WIDTH * 100}%`,
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
      {marker.kinds.map((kind) => <span data-marker-shape={findingShape(kind)} key={kind} style={markerShapeStyle(kind)} />)}
    </span>
    {marker.count > 1 && <span aria-hidden="true" className="marker-count">{marker.count}</span>}
  </button>
}

function SeriesLine({
  color,
  end,
  hour,
  lane,
  points,
}: {
  readonly color: "cyan" | "amber" | "violet"
  readonly end: number
  readonly hour: number
  readonly lane: number
  readonly points: readonly SeriesPoint[]
}) {
  if (points.length === 0) return null
  const range = seriesRange(points)
  return <>{[...timelineRuns(points).entries()].map(([runId, stored]) => {
    const path = stored
      .slice()
      .sort((left, right) => left.timestamp - right.timestamp)
      .map((point, index) => {
        const x = scaleX(point.timestamp, hour, end)
        const y = seriesY(point.value, lane, range.low, range.span)
        return `${index === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`
      })
      .join(" ")
    return <path className={`series-line series-${color}`} d={path} key={runId} />
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

export function timelineRuns(points: readonly SeriesPoint[]): ReadonlyMap<string, readonly (SeriesPoint & { readonly value: number })[]> {
  const bySegment = new Map<string, SeriesPoint[]>()
  for (const point of points) {
    const stored = bySegment.get(point.segmentId) ?? []
    stored.push(point)
    bySegment.set(point.segmentId, stored)
  }
  const runs = new Map<string, readonly (SeriesPoint & { readonly value: number })[]>()
  for (const [segmentId, stored] of bySegment) {
    let run: (SeriesPoint & { readonly value: number })[] = []
    let index = 0
    const flush = () => {
      if (run.length !== 0) runs.set(`${segmentId}:${index}`, run)
      run = []
      index += 1
    }
    for (const point of stored.slice().sort((left, right) => left.timestamp - right.timestamp)) {
      if (point.value === null || !Number.isFinite(point.value)) {
        flush()
      } else {
        run.push(point as SeriesPoint & { readonly value: number })
      }
    }
    flush()
  }
  return runs
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
  const ordered = findings.slice().sort(findingOrder)
  const stored: Finding[][] = []
  let active: Finding[] = []
  let anchor = 0
  for (const finding of ordered) {
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
  return stored.map((group) => {
    const first = group[0]
    const last = group.at(-1)
    if (first === undefined || last === undefined) throw new Error("empty marker cluster")
    const kinds = FINDING_KINDS.filter((kind) => group.some((finding) => finding.kind === kind))
    return {
      count: group.length,
      finding: first,
      findings: group,
      kind: first.kind,
      kinds,
      startTimestamp: first.timestamp,
      endTimestamp: last.timestamp,
      timestamp: first.timestamp,
    }
  })
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

export function findingShape(kind: Finding["kind"]): FindingShape {
  if (kind === "known_bad") return "diamond"
  if (kind === "spike") return "triangle"
  return "circle"
}

function markerShapeStyle(kind: Finding["kind"]): React.CSSProperties {
  const common: React.CSSProperties = { display: "block", flex: "0 0 auto", height: "9px", width: "9px" }
  switch (findingShape(kind)) {
    case "diamond":
      return { ...common, background: "#f43f5e", border: "1px solid #fecdd3", transform: "rotate(45deg)" }
    case "triangle":
      return { ...common, background: "#f59e0b", clipPath: "polygon(50% 0, 100% 100%, 0 100%)", height: "10px", width: "11px" }
    case "circle":
      return { ...common, background: "#a78bfa", border: "1px solid #ddd6fe", borderRadius: "50%" }
  }
}

export function seriesYAt(points: readonly SeriesPoint[], segmentId: string, timestamp: number, lane: number): number {
  if (points.length === 0) return TOP + lane * LANE_HEIGHT + LANE_HEIGHT / 2
  const range = seriesRange(points)
  const numeric = points.filter((point): point is SeriesPoint & { readonly value: number } => point.value !== null && Number.isFinite(point.value))
  if (numeric.length === 0) return TOP + lane * LANE_HEIGHT + LANE_HEIGHT / 2
  const runs = [...timelineRuns(points).values()]
  const matching = runs.find((run) => run[0]?.segmentId === segmentId && contains(run, timestamp))
    ?? runs.find((run) => contains(run, timestamp))
  if (matching === undefined) {
    const nearest = numeric.reduce((selected, point) => {
      const distance = Math.abs(point.timestamp - timestamp)
      const selectedDistance = Math.abs(selected.timestamp - timestamp)
      return distance < selectedDistance || (distance === selectedDistance && point.timestamp < selected.timestamp) ? point : selected
    })
    return seriesY(nearest.value, lane, range.low, range.span)
  }
  const stored = matching.slice().sort((left, right) => left.timestamp - right.timestamp)
  const rightIndex = stored.findIndex((point) => point.timestamp >= timestamp)
  const right = stored[rightIndex]
  const left = rightIndex <= 0 ? stored[0] : stored[rightIndex - 1]
  if (left === undefined || right === undefined) return TOP + lane * LANE_HEIGHT + LANE_HEIGHT / 2
  const duration = right.timestamp - left.timestamp
  const ratio = duration === 0 ? 0 : (timestamp - left.timestamp) / duration
  return seriesY(left.value + (right.value - left.value) * ratio, lane, range.low, range.span)
}

function contains(run: readonly SeriesPoint[], timestamp: number): boolean {
  const first = run[0]
  const last = run.at(-1)
  return first !== undefined && last !== undefined && first.timestamp <= timestamp && last.timestamp >= timestamp
}

function seriesRange(points: readonly SeriesPoint[]): { readonly low: number; readonly span: number } {
  const values = points.flatMap((point) => point.value === null || !Number.isFinite(point.value) ? [] : [point.value])
  if (values.length === 0) return { low: 0, span: 1 }
  const low = Math.min(...values)
  return { low, span: Math.max(...values) - low || 1 }
}

function seriesY(number: number, lane: number, low: number, span: number): number {
  const laneBottom = TOP + (lane + 1) * LANE_HEIGHT - 6
  return laneBottom - (number - low) / span * (LANE_HEIGHT - 12)
}

function scaleX(timestamp: number, hour: number, end: number): number {
  return Math.max(0, Math.min(WIDTH, (timestamp - hour) / (end - hour) * WIDTH))
}
