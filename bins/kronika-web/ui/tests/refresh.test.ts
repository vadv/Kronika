import assert from "node:assert/strict"
import test from "node:test"

import { importFile } from "./import-module.mjs"

const { isCurrentHour, latestTimelineTimestamp, REFRESH_INTERVAL_MS, refreshedCursor, scheduleRefresh } = await importFile("../src/refresh.ts")

const HOUR = 1_800_000_000_000_000

function timeline() {
  return {
    hour: HOUR,
    availableHours: [HOUR],
    availableSections: [],
    findingGroups: [],
    findings: [{ timestamp: HOUR + 40, segmentId: "a", logicalName: "health", typeId: "0", rowOrdinal: "0", fieldOrdinal: 0, kind: "spike" as const, category: null }],
    health: [{ timestamp: HOUR + 20 }],
    lanePoints: [{ timestamp: HOUR + 30 }],
    lanes: {},
    points: [{ timestamp: HOUR + 25 }],
    segments: [{ id: "a", minTs: HOUR + 10, maxTs: HOUR + 50, sections: [] }],
  } as never
}

test("only the current calendar hour schedules the shared fifteen-second refresh", () => {
  assert.equal(isCurrentHour(HOUR, HOUR + 1), true)
  assert.equal(isCurrentHour(HOUR - 3_600_000_000, HOUR + 1), false)
  const visibility = fakeVisibility(false)
  const intervals = fakeIntervals()
  let now = HOUR + 1
  let refreshes = 0
  const dispose = scheduleRefresh(HOUR, () => { refreshes += 1 }, visibility, intervals, () => now)

  assert.deepEqual([...intervals.delays.values()], [REFRESH_INTERVAL_MS])
  intervals.fire()
  assert.equal(refreshes, 1)
  now = HOUR + 3_600_000_000
  intervals.fire()
  assert.equal(refreshes, 1)
  assert.equal(intervals.delays.size, 0)
  dispose()
  assert.equal(intervals.delays.size, 0)

  scheduleRefresh(HOUR - 3_600_000_000, () => { refreshes += 1 }, visibility, intervals, () => HOUR + 1)
  assert.equal(intervals.delays.size, 0)
})

test("a hidden page stops polling and refreshes once when it becomes visible", () => {
  const visibility = fakeVisibility(true)
  const intervals = fakeIntervals()
  let refreshes = 0
  const dispose = scheduleRefresh(HOUR, () => { refreshes += 1 }, visibility, intervals, () => HOUR + 1)
  assert.equal(intervals.delays.size, 0)

  visibility.setHidden(false)
  assert.equal(refreshes, 1)
  assert.deepEqual([...intervals.delays.values()], [REFRESH_INTERVAL_MS])
  visibility.setHidden(true)
  assert.equal(intervals.delays.size, 0)
  dispose()
  assert.equal(visibility.listeners.size, 0)
})

test("the latest cursor includes the newest stored source without leaving the hour", () => {
  assert.equal(latestTimelineTimestamp(timeline()), HOUR + 50)
  assert.equal(latestTimelineTimestamp({ ...timeline(), segments: [{ id: "a", minTs: HOUR, maxTs: HOUR + 4_000_000_000, sections: [] }] } as never), HOUR + 3_600_000_000 - 1)
  assert.equal(refreshedCursor(HOUR + 12, false, timeline()), HOUR + 12)
  assert.equal(refreshedCursor(HOUR + 12, true, timeline()), HOUR + 50)
})

function fakeVisibility(initial: boolean) {
  const listeners = new Set<() => void>()
  return {
    hidden: initial,
    listeners,
    addEventListener(_type: "visibilitychange", listener: () => void) { listeners.add(listener) },
    removeEventListener(_type: "visibilitychange", listener: () => void) { listeners.delete(listener) },
    setHidden(hidden: boolean) {
      this.hidden = hidden
      for (const listener of listeners) listener()
    },
  }
}

function fakeIntervals() {
  let next = 0
  const callbacks = new Map<number, () => void>()
  const delays = new Map<number, number>()
  return {
    callbacks,
    delays,
    setInterval(callback: () => void, milliseconds: number) {
      next += 1
      callbacks.set(next, callback)
      delays.set(next, milliseconds)
      return next
    },
    clearInterval(id: number) {
      callbacks.delete(id)
      delays.delete(id)
    },
    fire() {
      for (const callback of callbacks.values()) callback()
    },
  }
}
