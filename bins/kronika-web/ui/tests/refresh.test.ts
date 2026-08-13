import assert from "node:assert/strict"
import test from "node:test"

import { importFile } from "./import-module.mjs"

const { emptyHourStatusKey, isCurrentHour, latestTimelineTimestamp, REFRESH_INTERVAL_MS, refreshedCursor, scheduleRefresh } = await importFile("../src/refresh.ts")

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

test("only the current calendar hour schedules the shared fifteen-second refresh", async () => {
  assert.equal(isCurrentHour(HOUR, HOUR + 1), true)
  assert.equal(isCurrentHour(HOUR - 3_600_000_000, HOUR + 1), false)
  const visibility = fakeVisibility(false)
  const timers = fakeTimers()
  let now = HOUR + 1
  let refreshes = 0
  const dispose = scheduleRefresh(HOUR, async () => { refreshes += 1 }, visibility, timers, () => now)

  assert.deepEqual(timers.pendingDelays(), [REFRESH_INTERVAL_MS])
  timers.advance(REFRESH_INTERVAL_MS)
  await tick()
  assert.equal(refreshes, 1)
  now = HOUR + 3_600_000_000
  timers.advance(REFRESH_INTERVAL_MS)
  await tick()
  assert.equal(refreshes, 1)
  assert.equal(timers.pending(), 0)
  dispose()
  assert.equal(timers.pending(), 0)

  scheduleRefresh(HOUR - 3_600_000_000, async () => { refreshes += 1 }, visibility, timers, () => HOUR + 1)
  assert.equal(timers.pending(), 0)
})

test("an empty open hour stays provisional while a completed hour is definitive", () => {
  assert.equal(emptyHourStatusKey(HOUR, HOUR + 1), "status.no_data_current")
  assert.equal(emptyHourStatusKey(HOUR - 3_600_000_000, HOUR + 1), "status.no_data_completed")
})

test("a hidden page stops polling and refreshes once when it becomes visible", async () => {
  const visibility = fakeVisibility(true)
  const timers = fakeTimers()
  let refreshes = 0
  const dispose = scheduleRefresh(HOUR, async () => { refreshes += 1 }, visibility, timers, () => HOUR + 1)
  assert.equal(timers.pending(), 0)

  visibility.setHidden(false)
  await tick()
  assert.equal(refreshes, 1)
  assert.equal(timers.pending(), 0)
  visibility.setHidden(true)
  assert.equal(timers.pending(), 0)
  dispose()
  assert.equal(visibility.listeners.size, 0)
})

test("the latest cursor includes the newest stored source without leaving the hour", () => {
  assert.equal(latestTimelineTimestamp(timeline()), HOUR + 50)
  assert.equal(latestTimelineTimestamp({ ...timeline(), segments: [{ id: "a", minTs: HOUR, maxTs: HOUR + 4_000_000_000, sections: [] }] } as never), HOUR + 3_600_000_000 - 1)
  assert.equal(refreshedCursor(HOUR + 12, false, timeline()), HOUR + 12)
  assert.equal(refreshedCursor(HOUR + 12, true, timeline()), HOUR + 50)
})

test("a slow refresh cannot overlap or enter a fifteen-second cancellation loop", async () => {
  const visibility = fakeVisibility(false)
  const timers = fakeTimers()
  const first = deferred()
  let requests = 0
  const harness = refreshHarness(() => {
    requests += 1
    return first.promise
  }, visibility, timers, () => HOUR + 1)

  timers.advance(REFRESH_INTERVAL_MS)
  await tick()
  assert.equal(requests, 1)
  timers.advance(REFRESH_INTERVAL_MS * 3)
  await tick()
  assert.equal(requests, 1)
  assert.equal(timers.pending(), 0)
  harness.request()
  assert.equal(requests, 1)

  first.resolve()
  await tick()
  assert.deepEqual(timers.pendingDelays(), [REFRESH_INTERVAL_MS])
  timers.advance(REFRESH_INTERVAL_MS - 1)
  assert.equal(requests, 1)
  timers.advance(1)
  await tick()
  assert.equal(requests, 2)
  harness.dispose()
})

test("refresh keeps a committed cursor stable and reloads latest exactly once when it advances", () => {
  assert.equal(refreshedCursor(HOUR + 12, false, timeline()), HOUR + 12)
  const advanced = refreshedCursor(HOUR + 12, true, timeline())
  assert.equal(advanced, HOUR + 50)
  assert.equal(refreshedCursor(advanced, true, timeline()), advanced)
  assert.equal(refreshedCursor(HOUR + 60, true, timeline()), HOUR + 60)
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

function fakeTimers() {
  let next = 0
  let now = 0
  const callbacks = new Map<number, () => void>()
  const deadlines = new Map<number, number>()
  return {
    setTimeout(callback: () => void, milliseconds: number) {
      next += 1
      callbacks.set(next, callback)
      deadlines.set(next, now + milliseconds)
      return next
    },
    clearTimeout(id: number) {
      callbacks.delete(id)
      deadlines.delete(id)
    },
    advance(milliseconds: number) {
      now += milliseconds
      for (const [id, deadline] of [...deadlines]) {
        if (deadline > now) continue
        const callback = callbacks.get(id)
        callbacks.delete(id)
        deadlines.delete(id)
        callback?.()
      }
    },
    pending() { return deadlines.size },
    pendingDelays() { return [...deadlines.values()].map((deadline) => deadline - now) },
  }
}

function refreshHarness(action: () => Promise<void>, visibility: ReturnType<typeof fakeVisibility>, timers: ReturnType<typeof fakeTimers>, now: () => number) {
  let busy = false
  let dispose = () => {}
  const render = () => {
    dispose()
    dispose = busy ? () => {} : scheduleRefresh(HOUR, request, visibility, timers, now)
  }
  const request = () => {
    if (busy) return
    busy = true
    render()
    void action().finally(() => { busy = false; render() })
  }
  render()
  return { request, dispose: () => dispose() }
}

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => { resolve = done })
  return { promise, resolve }
}

async function tick() {
  for (let pending = 0; pending < 8; pending += 1) await Promise.resolve()
}
