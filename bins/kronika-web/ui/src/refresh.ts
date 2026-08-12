import type { TimelineData } from "./api"
import { floorHour } from "./model"

export const REFRESH_INTERVAL_MS = 15_000

interface VisibilityTarget {
  readonly hidden: boolean
  addEventListener(type: "visibilitychange", listener: () => void): void
  removeEventListener(type: "visibilitychange", listener: () => void): void
}

interface IntervalTarget {
  setInterval(handler: () => void, milliseconds: number): number
  clearInterval(id: number): void
}

export function isCurrentHour(hour: number, now = Date.now() * 1_000): boolean {
  return hour === floorHour(now)
}

export function latestTimelineTimestamp(timeline: TimelineData): number {
  const end = timeline.hour + 3_600_000_000
  let latest = timeline.hour
  const take = (timestamp: number) => {
    if (timestamp >= timeline.hour && timestamp < end) latest = Math.max(latest, timestamp)
  }
  for (const segment of timeline.segments) take(Math.min(segment.maxTs, end - 1))
  for (const row of timeline.health) take(row.timestamp)
  for (const point of timeline.points) take(point.timestamp)
  for (const point of timeline.lanePoints) take(point.timestamp)
  for (const finding of timeline.findings) take(finding.timestamp)
  return latest
}

export function refreshedCursor(current: number, followsLatest: boolean, timeline: TimelineData): number {
  return followsLatest ? latestTimelineTimestamp(timeline) : current
}

export function scheduleRefresh(
  hour: number,
  refresh: () => void,
  visibility: VisibilityTarget = document,
  intervals: IntervalTarget = window,
  now: () => number = () => Date.now() * 1_000,
): () => void {
  let timer: number | null = null
  const stop = () => {
    if (timer === null) return
    intervals.clearInterval(timer)
    timer = null
  }
  const start = () => {
    if (!visibility.hidden && timer === null && isCurrentHour(hour, now())) {
      timer = intervals.setInterval(() => {
        if (isCurrentHour(hour, now())) refresh()
        else stop()
      }, REFRESH_INTERVAL_MS)
    }
  }
  const changed = () => {
    if (visibility.hidden) stop()
    else if (isCurrentHour(hour, now())) {
      refresh()
      start()
    }
  }
  start()
  visibility.addEventListener("visibilitychange", changed)
  return () => {
    stop()
    visibility.removeEventListener("visibilitychange", changed)
  }
}
