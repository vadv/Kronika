import { useMemo, useRef } from "react"

import type { DataRow, Finding } from "./api"
import { LabelHelp, type Translate } from "./help"
import { asNumber, formatUtc, value } from "./model"

const WIDTH = 920
const HEIGHT = 170
const LANE_HEIGHT = 36
const LANE_COUNT = 4
const TOP = 8
const PLOT_BOTTOM = TOP + LANE_COUNT * LANE_HEIGHT

interface SeriesPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number
}

export interface GroupedFinding {
  readonly count: number
  readonly finding: Finding
  readonly kind: Finding["kind"]
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
  readonly onFinding: (finding: Finding) => void
  readonly pressure: readonly DataRow[]
  readonly t: Translate
}) {
  const plot = useRef<HTMLDivElement>(null)
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
  const markers = useMemo(() => groupFindings(findings), [findings])
  const markerSlots = useMemo(() => findingSlots(markers), [markers])
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
        style={{ gridTemplateRows: `repeat(${LANE_COUNT}, ${LANE_HEIGHT}px)` }}
      >
        <LaneLabel label="lane.health.label" help="lane.health.help" t={t} />
        <LaneLabel label="lane.load.label" help="lane.load.help" t={t} />
        <LaneLabel label="lane.memory.label" help="lane.memory.help" t={t} />
        <LaneLabel label="lane.pressure.label" help="lane.pressure.help" t={t} />
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
              return <line className="timeline-grid" key={tick} x1={x} x2={x} y1={0} y2={PLOT_BOTTOM} />
            })}
            {[0, 1, 2, 3, 4].map((lane) => {
              const y = TOP + lane * LANE_HEIGHT
              return <line className="lane-line" key={lane} x1={0} x2={WIDTH} y1={y} y2={y} />
            })}
            <SeriesLine color="cyan" end={end} hour={hour} lane={0} points={healthPoints} />
            <SeriesLine color="amber" end={end} hour={hour} lane={1} points={loadPoints} />
            <SeriesLine color="violet" end={end} hour={hour} lane={2} points={memoryPoints} />
            <SeriesLine color="cyan" end={end} hour={hour} lane={3} points={pressurePoints[0] ?? []} />
            <SeriesLine color="violet" end={end} hour={hour} lane={3} points={pressurePoints[1] ?? []} />
            <SeriesLine color="amber" end={end} hour={hour} lane={3} points={pressurePoints[2] ?? []} />
            <line className="cursor-line" x1={cursorX} x2={cursorX} y1={0} y2={PLOT_BOTTOM} />
            {[0, 1, 2, 3, 4, 5, 6].map((tick) => {
              const timestamp = hour + tick * 600_000_000
              return <text className="tick-text" key={tick} textAnchor={tick === 0 ? "start" : tick === 6 ? "end" : "middle"} x={tick / 6 * WIDTH} y={HEIGHT - 2}>{formatUtc(timestamp).slice(11, 16)}</text>
            })}
          </svg>
        </div>
        {markers.map((marker, index) => {
          const slot = markerSlots.get(`${marker.timestamp}:${marker.kind}`) ?? { index: 0, total: 1 }
          const offset = (slot.index - (slot.total - 1) / 2) * 11
          const x = scaleX(marker.timestamp, hour, end)
          const y = seriesYAt(healthPoints, marker.finding.segmentId, marker.timestamp, 0) + offset
          return <FindingMarker
            key={`${marker.timestamp}:${marker.kind}:${index}`}
            marker={marker}
            onActivate={() => {
              onCursor(marker.timestamp)
              onFinding(marker.finding)
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
  return <button
    aria-label={`${t(`locator.${marker.kind}`)} · ${formatUtc(marker.timestamp)}${marker.count > 1 ? ` · ×${marker.count}` : ""}`}
    className={`marker-button marker-${marker.kind}`}
    data-marker-shape={findingShape(marker.kind)}
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
      height: "20px",
      justifyContent: "center",
      left: `${x / WIDTH * 100}%`,
      padding: 0,
      position: "absolute",
      top: `${y}px`,
      transform: "translate(-50%, -50%)",
      width: "20px",
      zIndex: 2,
    }}
    title={`${t(`locator.${marker.kind}`)} · ${formatUtc(marker.timestamp)}`}
    type="button"
  >
    <span aria-hidden="true" style={markerShapeStyle(marker.kind)} />
    {marker.count > 1 && <span aria-hidden="true" className="marker-count" style={{ color: "#b7c0cb", left: "15px", position: "absolute", top: "6px" }}>{marker.count}</span>}
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
  const bySegment = new Map<string, SeriesPoint[]>()
  for (const point of points) {
    const current = bySegment.get(point.segmentId) ?? []
    current.push(point)
    bySegment.set(point.segmentId, current)
  }
  return <>{[...bySegment.entries()].map(([segmentId, stored]) => {
    const path = stored
      .slice()
      .sort((left, right) => left.timestamp - right.timestamp)
      .map((point, index) => {
        const x = scaleX(point.timestamp, hour, end)
        const y = seriesY(point.value, lane, range.low, range.span)
        return `${index === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`
      })
      .join(" ")
    return <path className={`series-line series-${color}`} d={path} key={segmentId} />
  })}</>
}

function series(rows: readonly DataRow[], field: string): readonly SeriesPoint[] {
  return preferredSeries(rows, [field])
}

function preferredSeries(rows: readonly DataRow[], fields: readonly string[]): readonly SeriesPoint[] {
  return rows.flatMap((row) => {
    const number = fields.reduce<number | null>((selected, field) => selected ?? asNumber(value(row, field)), null)
    return number === null ? [] : [{ segmentId: row.segmentId, timestamp: row.timestamp, value: number }]
  })
}

export function groupFindings(findings: readonly Finding[]): readonly GroupedFinding[] {
  const groups = new Map<string, GroupedFinding>()
  for (const finding of findings) {
    const key = `${finding.timestamp}:${finding.kind}`
    const current = groups.get(key)
    groups.set(key, {
      count: (current?.count ?? 0) + 1,
      finding: current?.finding ?? finding,
      kind: finding.kind,
      timestamp: finding.timestamp,
    })
  }
  return [...groups.values()].sort((left, right) => left.timestamp - right.timestamp || left.kind.localeCompare(right.kind))
}

export function findingShape(kind: Finding["kind"]): FindingShape {
  if (kind === "known_bad") return "diamond"
  if (kind === "spike") return "triangle"
  return "circle"
}

function findingSlots(markers: readonly GroupedFinding[]): ReadonlyMap<string, { readonly index: number; readonly total: number }> {
  const byTime = new Map<number, GroupedFinding[]>()
  for (const marker of markers) {
    const current = byTime.get(marker.timestamp) ?? []
    current.push(marker)
    byTime.set(marker.timestamp, current)
  }
  const slots = new Map<string, { readonly index: number; readonly total: number }>()
  for (const [timestamp, stored] of byTime) {
    stored.forEach((marker, index) => slots.set(`${timestamp}:${marker.kind}`, { index, total: stored.length }))
  }
  return slots
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

function seriesYAt(points: readonly SeriesPoint[], segmentId: string, timestamp: number, lane: number): number {
  if (points.length === 0) return TOP + lane * LANE_HEIGHT + LANE_HEIGHT / 2
  const range = seriesRange(points)
  const stored = points
    .filter((point) => point.segmentId === segmentId)
    .slice()
    .sort((left, right) => left.timestamp - right.timestamp)
  if (stored.length === 0) return TOP + lane * LANE_HEIGHT + LANE_HEIGHT / 2
  const rightIndex = stored.findIndex((point) => point.timestamp >= timestamp)
  const right = rightIndex < 0 ? stored.at(-1) : stored[rightIndex]
  const left = rightIndex < 0 ? stored.at(-1) : rightIndex === 0 ? stored[0] : stored[rightIndex - 1]
  if (left === undefined || right === undefined) return TOP + lane * LANE_HEIGHT + LANE_HEIGHT / 2
  const duration = right.timestamp - left.timestamp
  const ratio = duration === 0 ? 0 : (timestamp - left.timestamp) / duration
  return seriesY(left.value + (right.value - left.value) * ratio, lane, range.low, range.span)
}

function seriesRange(points: readonly SeriesPoint[]): { readonly low: number; readonly span: number } {
  const values = points.map((point) => point.value)
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
