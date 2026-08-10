import { useMemo, useRef } from "react"

import type { DataRow, Finding } from "./api"
import { LabelHelp, type Translate } from "./help"
import { asNumber, formatUtc, value } from "./model"

const WIDTH = 920
const HEIGHT = 205
const LANE_HEIGHT = 36
const TOP = 8

interface SeriesPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number
}

export function Timeline({
  cursor,
  findings,
  health,
  hour,
  load,
  memory,
  onCursor,
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
  readonly pressure: readonly DataRow[]
  readonly t: Translate
}) {
  const plot = useRef<HTMLDivElement>(null)
  const end = hour + 3_600_000_000
  const healthPoints = useMemo(() => series(health, "os_health"), [health])
  const loadPoints = useMemo(() => series(load, "load1"), [load])
  const memoryPoints = useMemo(() => series(memory, "mem_available"), [memory])
  const pressurePoints = useMemo(() => [0, 1, 2].map((resource) => series(
    pressure.filter((row) => asNumber(value(row, "resource")) === resource),
    "some_avg10",
  )), [pressure])
  const markers = useMemo(() => groupedFindings(findings), [findings])
  const setFromClient = (clientX: number) => {
    const bounds = plot.current?.getBoundingClientRect()
    if (bounds === undefined) return
    const ratio = Math.max(0, Math.min(1, (clientX - bounds.left) / bounds.width))
    onCursor(Math.min(end - 1_000, Math.round(hour + ratio * (end - hour))))
  }
  const cursorX = scaleX(cursor, hour, end)
  return (
    <section className="timeline-shell" aria-label={t("hour.range", { start: formatUtc(hour).slice(11, 16), end: formatUtc(end).slice(11, 16) })}>
      <div className="timeline-labels" aria-hidden="false">
        <LaneLabel label="lane.health.label" help="lane.health.help" t={t} />
        <LaneLabel label="lane.load.label" help="lane.load.help" t={t} />
        <LaneLabel label="lane.memory.label" help="lane.memory.help" t={t} />
        <LaneLabel label="lane.pressure.label" help="lane.pressure.help" t={t} />
        <LaneLabel label="lane.locators.label" help="lane.locators.help" t={t} />
      </div>
      <div
        aria-label={t("hour.cursor", { time: formatUtc(cursor) })}
        aria-valuemax={end - 1_000}
        aria-valuemin={hour}
        aria-valuenow={cursor}
        aria-valuetext={formatUtc(cursor)}
        className="timeline-plot"
        data-testid="hour-timeline"
        onKeyDown={(event) => {
          if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return
          event.preventDefault()
          const direction = event.key === "ArrowLeft" ? -1 : 1
          onCursor(Math.max(hour, Math.min(end - 1_000, cursor + direction * 60_000_000)))
        }}
        onPointerDown={(event) => setFromClient(event.clientX)}
        ref={plot}
        role="slider"
        tabIndex={0}
      >
        <svg aria-hidden="true" preserveAspectRatio="none" viewBox={`0 0 ${WIDTH} ${HEIGHT}`}>
          {[0, 1, 2, 3, 4, 5, 6].map((tick) => {
            const x = tick / 6 * WIDTH
            return <line className="timeline-grid" key={tick} x1={x} x2={x} y1={0} y2={HEIGHT - 16} />
          })}
          {[0, 1, 2, 3, 4, 5].map((lane) => {
            const y = TOP + lane * LANE_HEIGHT
            return <line className="lane-line" key={lane} x1={0} x2={WIDTH} y1={y} y2={y} />
          })}
          <SeriesLine color="cyan" end={end} hour={hour} lane={0} points={healthPoints} />
          <SeriesLine color="amber" end={end} hour={hour} lane={1} points={loadPoints} />
          <SeriesLine color="violet" end={end} hour={hour} lane={2} points={memoryPoints} />
          <SeriesLine color="cyan" end={end} hour={hour} lane={3} points={pressurePoints[0] ?? []} />
          <SeriesLine color="violet" end={end} hour={hour} lane={3} points={pressurePoints[1] ?? []} />
          <SeriesLine color="amber" end={end} hour={hour} lane={3} points={pressurePoints[2] ?? []} />
          {markers.map((finding) => {
            const x = scaleX(finding.timestamp, hour, end)
            const sameTime = markers.filter((other) => other.timestamp === finding.timestamp)
            const slot = sameTime.findIndex((other) => other.kind === finding.kind)
            const y = TOP + 4 * LANE_HEIGHT + 11 + slot * 8
            return <g key={`${finding.timestamp}:${finding.kind}`}>
              <circle className={`marker marker-${finding.kind}`} cx={x} cy={y} r={4} />
              {finding.count > 1 && <text className="marker-count" x={x + 5} y={y + 3}>{finding.count}</text>}
            </g>
          })}
          <line className="cursor-line" x1={cursorX} x2={cursorX} y1={0} y2={HEIGHT - 16} />
          {[0, 1, 2, 3, 4, 5, 6].map((tick) => {
            const timestamp = hour + tick * 600_000_000
            return <text className="tick-text" key={tick} textAnchor={tick === 0 ? "start" : tick === 6 ? "end" : "middle"} x={tick / 6 * WIDTH} y={HEIGHT - 2}>{formatUtc(timestamp).slice(11, 16)}</text>
          })}
        </svg>
      </div>
    </section>
  )
}

function LaneLabel({ label, help, t }: { readonly label: string; readonly help: string; readonly t: Translate }) {
  return <div className="lane-label"><LabelHelp helpKey={help} labelKey={label} t={t} /></div>
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
  const values = points.map((point) => point.value)
  const low = Math.min(...values)
  const high = Math.max(...values)
  const span = high - low || 1
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
        const laneBottom = TOP + (lane + 1) * LANE_HEIGHT - 6
        const y = laneBottom - (point.value - low) / span * (LANE_HEIGHT - 12)
        return `${index === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`
      })
      .join(" ")
    return <path className={`series-line series-${color}`} d={path} key={segmentId} />
  })}</>
}

function series(rows: readonly DataRow[], field: string): readonly SeriesPoint[] {
  return rows.flatMap((row) => {
    const number = asNumber(value(row, field))
    return number === null ? [] : [{ segmentId: row.segmentId, timestamp: row.timestamp, value: number }]
  })
}

function groupedFindings(findings: readonly Finding[]): readonly { readonly timestamp: number; readonly kind: Finding["kind"]; readonly count: number }[] {
  const groups = new Map<string, { timestamp: number; kind: Finding["kind"]; count: number }>()
  for (const finding of findings) {
    const key = `${finding.timestamp}:${finding.kind}`
    const current = groups.get(key)
    groups.set(key, { timestamp: finding.timestamp, kind: finding.kind, count: (current?.count ?? 0) + 1 })
  }
  return [...groups.values()].sort((left, right) => left.timestamp - right.timestamp || left.kind.localeCompare(right.kind))
}

function scaleX(timestamp: number, hour: number, end: number): number {
  return Math.max(0, Math.min(WIDTH, (timestamp - hour) / (end - hour) * WIDTH))
}
