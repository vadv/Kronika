import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { mkdtemp, readFile, rm } from "node:fs/promises"
import { createServer } from "node:http"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { gunzipSync } from "node:zlib"

const HOUR_US = 3_600_000_000
const HOUR = Date.UTC(2026, 7, 13, 5) * 1_000
const AT = HOUR + 1_800_000_000
const AUGUST_HOUR = Date.UTC(2026, 7, 10, 3) * 1_000
const DECEMBER_HOUR = Date.UTC(2026, 11, 31, 23) * 1_000
const JANUARY_HOUR = Date.UTC(2027, 0, 15, 9) * 1_000
const FEBRUARY_HOUR = Date.UTC(2027, 1, 1, 2) * 1_000
const DST_EDT_HOUR = Date.UTC(2026, 10, 1, 5) * 1_000
const DST_EST_HOUR = Date.UTC(2026, 10, 1, 6) * 1_000
const AVAILABLE_HOURS = [AUGUST_HOUR, HOUR + HOUR_US, DECEMBER_HOUR, JANUARY_HOUR, FEBRUARY_HOUR]
const SEGMENT = "7300"
const ARTIFACT = process.env.KRONIKA_UI_ARTIFACT ?? new URL("../kronika-ui.html.gz", import.meta.url)
const BEFORE_AT = AT - 5_000_000
const AFTER_AT = AT + 7_000_000
const QUARTER = HOUR + 900_000_000
const QUARTER_PREVIOUS = QUARTER - 5_000_000
const QUARTER_NEXT = QUARTER + 5_000_000
const SESSION_COOKIE = `kronika_session=v1.2000000000.${"A".repeat(43)}`
const SLOW_PATTERN = 'SELECT "bulkoperations_bulktask"."id" FROM "bulkoperations_bulktask" WHERE "bulkoperations_bulktask"."status" = ? AND "bulkoperations_bulktask"."tenant_partition_with_a_deliberately_long_identifier" = ?'
const SLOW_QUERY = `${SLOW_PATTERN.replaceAll("?", "'pending'")} ORDER BY "bulkoperations_bulktask"."created_at" DESC LIMIT 250`


const ZONE_VALUE = `document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value")`
const ZONE_LABEL = `document.querySelector('[data-testid="timezone-value"]')?.textContent`

async function switchZone(cdp, zone) {
  await cdp.evaluate(`document.querySelector('[data-testid="timezone-select"]').click()`)
  await cdp.waitFor(`document.querySelector('[data-testid="timezone-option-${zone}"]') !== null`, `the ${zone} timezone option`)
  await cdp.evaluate(`document.querySelector('[data-testid="timezone-option-${zone}"]').click()`)
}

test("display timezone and human chart precision stay global", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const hour = Number(url.searchParams.get("from") ?? HOUR)
      const records = timelineRecords(hour).map((record) => record.record === "point" && record.series === "os_health" && record.ts === String(AT)
        ? { ...record, value: 41.729068244136855 }
        : record)
      return ndjson(response, records)
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      return ndjson(response, url.searchParams.getAll("section").includes("pg_stat_statements") ? statementRecords(true) : snapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("focused browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const result = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, result)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "Europe/Moscow" })
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.statements` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length >= 1`, "the focused Statements path", 15_000)
    await settleLayout(cdp)

    await cdp.waitFor(`document.querySelector('[data-testid="activity-toggle"]')?.getAttribute("aria-expanded") === "false"`, "the collapsed activity ledger")
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/heatmap")).length, 0)
    await cdp.evaluate(`document.querySelector('[data-testid="activity-toggle"]').click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"]').length === 2`, "the ranked activity ledger", 15_000)
    assert.ok(requests.filter(({ path }) => path.startsWith("/api/heatmap")).length >= 1)
    const ledger = await cdp.evaluate(`(() => ({
      top: document.querySelector('[data-testid="activity-top-count"]')?.textContent ?? "",
      totals: document.querySelector('[data-testid="activity-row-totals"]') !== null,
      others: document.querySelector('[data-testid="activity-row-others"]')?.textContent ?? "",
      cells: document.querySelectorAll('[data-testid="activity-row"] rect').length,
      help: document.querySelectorAll('[data-testid="activity-pg_stat_statements"] .help-dot').length,
    }))()`)
    assert.equal(ledger.totals, true)
    assert.match(ledger.top, /2/)
    assert.match(ledger.others, /1/)
    assert.equal(ledger.cells, 6)
    assert.ok(ledger.help >= 3)
    await cdp.evaluate(`document.querySelector('[data-testid="activity-cut-wal_bytes"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-cut-wal_bytes"]')?.getAttribute("aria-pressed") === "true"`, "the WAL cut")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"]').length === 2`, "the reranked ledger", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="activity-maximize"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-overlay"]') !== null`, "the full-screen ledger")
    await cdp.evaluate(`document.querySelector('[data-testid="activity-top-50"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-top-50"]')?.getAttribute("aria-pressed") === "true"`, "the deeper rank")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-overlay"] [data-testid="activity-row"]').length === 2`, "the reloaded full-screen ledger", 15_000)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-overlay"]') === null`, "the restored ledger")
    await cdp.evaluate(`document.querySelector('[data-testid="activity-cut-exec_time"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-cut-exec_time"]')?.getAttribute("aria-pressed") === "true"`, "the default cut back")
    await cdp.evaluate(`document.querySelector('[data-testid="activity-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-toggle"]')?.getAttribute("aria-expanded") === "false"`, "the ledger collapsed again")
    const browserMode = await cdp.evaluate(`(() => ({
      at: new URL(location.href).searchParams.get("at"),
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent ?? "",
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent ?? "",
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth || document.querySelector(".topbar").scrollWidth > document.querySelector(".topbar").clientWidth,
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      updated: document.querySelector('[data-testid="updated-time"]')?.textContent ?? "",
      zone: document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value"),
      zoneLabel: document.querySelector('[data-testid="timezone-value"]')?.textContent ?? "",
      zoneSelectors: document.querySelectorAll('[data-testid="timezone-select"]').length,
    }))()`)
    assert.equal(browserMode.at, String(AT))
    assert.equal(browserMode.zone, "browser")
    assert.equal(browserMode.zoneLabel, "Browser time")
    assert.equal(browserMode.zoneSelectors, 1)
    assert.match(browserMode.cursor, /08:30:00/)
    assert.match(browserMode.hour, /08:00–09:00/)
    assert.match(browserMode.status, /08:30:00/)
    assert.doesNotMatch(browserMode.status, /\b\d{2}[./]\d{2}[./]2026\b/)
    assert.match(browserMode.updated, /^(?:Updated)?\d+ [smh] ago$|^(?:Updated)?\d+ min ago$/)
    for (const output of [browserMode.cursor, browserMode.hour, browserMode.status, browserMode.updated]) {
      assert.doesNotMatch(output, /GMT|UTC|\.\d{3}(?!\d)/)
    }
    assert.equal(browserMode.overflow, false)

    const hover = async () => {
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .u-over') !== null`, "the focused health timeline")
      await cdp.evaluate(`(() => {
        const plot = document.querySelector('[data-testid="hour-timeline"] .u-over')
        const bounds = plot.getBoundingClientRect()
        const clientX = bounds.left + (${AT + 3_000_000} - ${HOUR}) / ${HOUR_US} * bounds.width
        const clientY = bounds.top + bounds.height / 2
        plot.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, clientX, clientY }))
        plot.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX, clientY }))
      })()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .chart-tooltip') !== null`, "the focused chart tooltip")
    }
    await hover()
    const tooltip = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] .chart-tooltip').textContent`)
    assert.match(tooltip, /08:30:07/)
    assert.match(tooltip, /48%/)
    assert.doesNotMatch(tooltip, /41\.729068|GMT|UTC|\.000/)
    const apiBeforeSwitch = requests.filter(({ path }) => path.startsWith("/api/")).length
    await switchZone(cdp, "utc")
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value") === "utc" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("05:30:00") === true`, "the UTC display mode")
    await hover()
    const utcMode = await cdp.evaluate(`(() => ({
      at: new URL(location.href).searchParams.get("at"),
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent ?? "",
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent ?? "",
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      tooltip: document.querySelector('[data-testid="hour-timeline"] .chart-tooltip')?.textContent ?? "",
      updated: document.querySelector('[data-testid="updated-time"]')?.textContent ?? "",
      zone: document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value"),
      zoneLabel: document.querySelector('[data-testid="timezone-value"]')?.textContent ?? "",
    }))()`)
    assert.equal(utcMode.at, String(AT))
    assert.equal(utcMode.zone, "utc")
    assert.equal(utcMode.zoneLabel, "UTC")
    assert.match(utcMode.cursor, /05:30:00/)
    assert.match(utcMode.hour, /05:00–06:00/)
    assert.match(utcMode.status, /05:30:00/)
    assert.doesNotMatch(utcMode.status, /\b\d{2}[./]\d{2}[./]2026\b/)
    assert.match(utcMode.tooltip, /05:30:07/)
    assert.match(utcMode.updated, /^(?:Updated)?\d+ [smh] ago$|^(?:Updated)?\d+ min ago$/)
    for (const output of [utcMode.cursor, utcMode.hour, utcMode.status, utcMode.tooltip, utcMode.updated]) {
      assert.doesNotMatch(output, /GMT|UTC|\.\d{3}(?!\d)/)
    }
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/")).length, apiBeforeSwitch)
    await switchZone(cdp, "browser")
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-value"]')?.textContent === "Browser time" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("08:30:00") === true`, "the Browser display restore")
    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value") === "browser" && document.querySelector('[data-testid="timezone-value"]')?.textContent === "Browser time" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("08:30:00") === true`, "the persisted Browser display", 15_000)

    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-tab"]')?.getAttribute("aria-current") === "page"`, "the Processes history destination")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("view")`), null)
    await cdp.evaluate(`(() => {
      const navigator = document.querySelector('[data-testid="hour-timeline"] input.chart-navigator')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(navigator, "2")
      navigator.dispatchEvent(new Event("input", { bubbles: true }))
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "the exact replacement cursor")
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}" && new URL(location.href).searchParams.get("view") === "pg.statements"`, "the exact back selection")
    await cdp.evaluate(`history.forward()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}" && new URL(location.href).searchParams.get("view") === null`, "the exact forward selection")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value")`), "browser")
    assert.equal(await cdp.evaluate(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`), true)
    assert.deepEqual(result.errors, [])
    assert.deepEqual(result.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test.skip("legacy fullscreen uPlot is replaced by the shared Inspector", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const startFinding = {
    record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
    field_ordinal: 11, row_ordinal: "998", ts: String(HOUR + 1),
  }
  const nearEndFinding = {
    record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
    field_ordinal: 11, row_ordinal: "997", ts: String(HOUR + Math.floor(HOUR_US * 0.895)),
  }
  const endFinding = {
    record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
    field_ordinal: 11, row_ordinal: "999", ts: String(HOUR + HOUR_US - 1),
  }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, [
      ...timelineRecords().filter(({ record }) => record !== "finding"),
      startFinding,
      nearEndFinding,
      endFinding,
    ])
    if (url.pathname.startsWith("/api/")) return ndjson(response, [])
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("expanded-chart browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    const viewports = [
      { height: 768, label: "desktop", mobile: false, width: 1366 },
      { height: 768, label: "iPad landscape", mobile: true, width: 1194 },
      { height: 844, label: "phone", mobile: true, width: 390 },
    ]
    for (const viewport of viewports) {
      await cdp.send("Emulation.setDeviceMetricsOverride", {
        deviceScaleFactor: 1,
        height: viewport.height,
        mobile: viewport.mobile,
        width: viewport.width,
      })
      await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}` })
      await cdp.waitFor(`document.querySelectorAll('[data-testid="hour-timeline"] .marker-button').length >= 2`, `${viewport.label} timeline markers`, 15_000)
      await settleLayout(cdp)
      const before = await cdp.evaluate(`(() => {
        document.documentElement.style.overflow = "visible"
        document.body.style.overflow = "auto"
        document.body.style.minHeight = "200vh"
        window.scrollTo(0, 120)
        return { body: document.body.style.overflow, root: document.documentElement.style.overflow, scrollY }
      })()`)
      assert.equal(before.body, "auto", viewport.label)
      assert.equal(before.root, "visible", viewport.label)
      assert.ok(Math.abs(before.scrollY - 120) <= 1, `${viewport.label}: ${JSON.stringify(before)}`)
      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] .chart-expand').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"][role="dialog"].uplot-expanded') !== null`, `${viewport.label} expanded timeline`)
      await settleLayout(cdp)
      const geometry = await cdp.evaluate(`(() => {
        const dialog = document.querySelector('[data-testid="hour-timeline"][role="dialog"]')
        const header = dialog.querySelector("figcaption")
        const title = header.querySelector(".chart-series-labels")
        const current = header.querySelector(".chart-current")
        const control = header.querySelector(".chart-expand")
        const markers = [...dialog.querySelectorAll('[data-testid="chart-marker-track"] .marker-button')]
        const lastMarker = markers.at(-1)
        const bounds = (node) => {
          const rect = node.getBoundingClientRect()
          return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width }
        }
        const intersects = (left, right) => Math.max(left.left, right.left) < Math.min(left.right, right.right)
          && Math.max(left.top, right.top) < Math.min(left.bottom, right.bottom)
        const headerRect = bounds(header)
        const titleRect = bounds(title)
        const currentRect = bounds(current)
        const controlRect = bounds(control)
        const dialogRect = bounds(dialog)
        const markerRect = bounds(lastMarker)
        const markerRects = markers.map(bounds)
        const markersOverlap = markerRects.some((left, index) => markerRects.slice(index + 1).some((right) => intersects(left, right)))
        const hit = [[markerRect.left + 2, (markerRect.top + markerRect.bottom) / 2],
          [(markerRect.left + markerRect.right) / 2, (markerRect.top + markerRect.bottom) / 2],
          [markerRect.right - 2, (markerRect.top + markerRect.bottom) / 2]].every(([x, y]) =>
          document.elementsFromPoint(x, y).some((node) => node === lastMarker || lastMarker.contains(node)))
        return {
          actionCount: dialog.querySelectorAll(".chart-expand, .chart-close").length,
          activeIsControl: document.activeElement === control,
          bodyOverflow: getComputedStyle(document.body).overflow,
          control: controlRect,
          current: currentRect,
          currentControlOverlap: intersects(currentRect, controlRect),
          dialog: dialogRect,
          header: headerRect,
          hit,
          horizontalOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
          lastMarker: markerRect,
          lastMarkerCount: Number(lastMarker.dataset.markerCount),
          markerCount: markers.reduce((count, marker) => count + Number(marker.dataset.markerCount), 0),
          markerControlOverlap: intersects(markerRect, controlRect),
          markersOverlap,
          rootAnchor: getComputedStyle(document.documentElement).overflowAnchor,
          rootOverflow: getComputedStyle(document.documentElement).overflow,
          scrollY,
          title: titleRect,
          titleControlOverlap: intersects(titleRect, controlRect),
          titleCurrentOverlap: intersects(titleRect, currentRect),
          viewport: { height: innerHeight, width: innerWidth },
        }
      })()`)
      assert.equal(geometry.actionCount, 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.activeIsControl, true, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.rootAnchor, "none", `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.rootOverflow, "hidden", `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.bodyOverflow, "hidden", `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.scrollY, before.scrollY, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.horizontalOverflow, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.currentControlOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.titleControlOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.titleCurrentOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.markerControlOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.markersOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.hit, true, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.lastMarkerCount, 2, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.markerCount, 3, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.control.width >= 44 && geometry.control.height >= 44, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.title.width > 0 && geometry.current.width > 0, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.title.left >= geometry.header.left - 1 && geometry.title.right <= geometry.header.right + 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.current.left >= geometry.header.left - 1 && geometry.current.right <= geometry.header.right + 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.control.left >= geometry.header.left - 1 && geometry.control.right <= geometry.header.right + 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.lastMarker.top >= geometry.header.bottom - 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.lastMarker.right <= geometry.control.left - 8, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.lastMarker.left >= geometry.dialog.left && geometry.lastMarker.right <= geometry.dialog.right, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(Math.abs(geometry.dialog.left) <= 1 && Math.abs(geometry.dialog.top) <= 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.dialog.width >= geometry.viewport.width - 1 && geometry.dialog.height >= geometry.viewport.height - 1, `${viewport.label}: ${JSON.stringify(geometry)}`)

      await cdp.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: Math.floor(viewport.width / 2), y: Math.floor(viewport.height / 2), deltaX: 0, deltaY: 500 })
      await delay(120)
      assert.equal(await cdp.evaluate("scrollY"), before.scrollY, viewport.label)
      await cdp.evaluate(`(() => {
        const dialog = document.querySelector('[data-testid="hour-timeline"][role="dialog"]')
        const navigator = dialog.querySelector("input.chart-navigator")
        navigator.focus()
        navigator.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Tab" }))
      })()`)
      assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-series-labels .help-dot')`), true, viewport.label)
      await cdp.evaluate(`(() => {
        const first = document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-series-labels .help-dot')
        first.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Tab", shiftKey: true }))
      })()`)
      assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-timeline"][role="dialog"] input.chart-navigator')`), true, viewport.label)
      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-expand').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"][role="dialog"]') === null`, `${viewport.label} close action`)
      await cdp.waitFor(`document.documentElement.style.overflow === "visible" && document.body.style.overflow === "auto"`, `${viewport.label} scroll restore`)
      await cdp.waitFor(`document.activeElement === document.querySelector('[data-testid="hour-timeline"] .chart-expand')`, `${viewport.label} close focus return`)
      await settleLayout(cdp)
      assert.equal(await cdp.evaluate("scrollY"), before.scrollY, viewport.label)
      assert.equal(await cdp.evaluate(`getComputedStyle(document.documentElement).overflowAnchor`), "none", viewport.label)

      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] .chart-expand').click()`)
      await cdp.waitFor(`(() => {
        const control = document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-expand')
        return control !== null && document.activeElement === control
          && document.documentElement.style.overflow === "hidden" && document.body.style.overflow === "hidden"
      })()`, `${viewport.label} Escape setup`)
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"][role="dialog"]') === null`, `${viewport.label} Escape close`)
      await cdp.waitFor(`document.activeElement === document.querySelector('[data-testid="hour-timeline"] .chart-expand')`, `${viewport.label} Escape focus return`)
      await settleLayout(cdp)
      const restored = await cdp.evaluate(`({ body: document.body.style.overflow, root: document.documentElement.style.overflow, scrollY })`)
      assert.equal(restored.body, before.body, viewport.label)
      assert.equal(restored.root, before.root, viewport.label)
      assert.ok(Math.abs(restored.scrollY - before.scrollY) <= 1, `${viewport.label}: ${JSON.stringify({ before, restored })}`)
      assert.equal(await cdp.evaluate(`getComputedStyle(document.documentElement).overflowAnchor`), "none", viewport.label)
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the production artifact preserves wire keys and exact finding page state", { timeout: 120_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: false }
  let heldContextPage = null
  let heldSystemPage = null
  let systemPageWasHeld = false
  let relationMode = "single"
  let inlinePlanQueryMode = "ready"
  let contextPageRequested
  let systemPageRequested
  const contextPage = new Promise((resolve) => { contextPageRequested = resolve })
  const systemPage = new Promise((resolve) => { systemPageRequested = resolve })
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") {
      answerSession(request, response, authState)
      return
    }
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) {
      unauthorized(response)
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") {
      ndjson(response, [])
      return
    }
    if (url.pathname === "/api/hour") {
      ndjson(response, timelineRecords(Number(url.searchParams.get("from") ?? HOUR)))
      return
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      if (sections.includes("pg_store_plans")) {
        ndjson(response, planRecords())
      } else if (url.searchParams.has("row_ordinal") && sections.includes("pg_stat_user_indexes")) {
        ndjson(response, exactIndexRecords())
      } else if (url.searchParams.has("row_ordinal")) {
        ndjson(response, statementRecords(false))
      } else if (sections.includes("pg_stat_statements")) {
        const inline = url.searchParams.get("search") === "query_id:42"
          && url.searchParams.get("page_size") === "1"
          && url.searchParams.get("first_match") === "1"
          && !url.searchParams.has("text")
        if (inline) {
          if (inlinePlanQueryMode === "error") {
            response.writeHead(503)
            response.end()
          } else {
            ndjson(response, inlinePlanQueryMode === "empty" ? emptyPlanQueryRecords() : inlinePlanQueryRecords())
          }
          return
        }
        const planNavigation = url.searchParams.get("search") === "database:operators AND role:reporter AND query_id:42"
        if (planNavigation) {
          ndjson(response, planStatementRecords())
          return
        }
        const filtered = ["queryid", "userid", "dbid", "toplevel"].every((field) => url.searchParams.has(`where.${field}`))
        if (filtered && heldContextPage === null) {
          heldContextPage = response
          contextPageRequested()
        } else {
          ndjson(response, filtered ? statementRecords(true) : statementRecords(true, 4_807, true, 50))
        }
      } else if (sections.includes("pg_stat_user_tables") || sections.includes("pg_stat_user_indexes")) {
        ndjson(response, relationRecords(url, relationMode))
      } else if (sections.includes("os_cpu")) {
        if (systemPageWasHeld) {
          ndjson(response, systemSnapshotRecords())
        } else {
          systemPageWasHeld = true
          heldSystemPage = response
          systemPageRequested()
        }
      } else if (sections.includes("pg_stat_activity")) {
        ndjson(response, snapshotRecords())
      } else {
        ndjson(response, [])
      }
      return
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("artifact test server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    const errors = []
    const external = []
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data)
      if (message.method === "Runtime.exceptionThrown") {
        errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text)
      }
      if (message.method === "Runtime.consoleAPICalled" && ["assert", "error"].includes(message.params.type)) {
        errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "))
      }
      if (message.method === "Log.entryAdded" && message.params.entry.level === "error") {
        if (!expectedUnauthorizedLog(message.params.entry.text)) errors.push(message.params.entry.text)
      }
      if (message.method === "Network.loadingFailed"
        && message.params.canceled !== true
        && message.params.errorText !== "net::ERR_ABORTED") {
        errors.push(message.params.errorText)
      }
      if (message.method === "Network.responseReceived" && message.params.response.status >= 400) {
        const response = message.params.response
        const url = new URL(response.url)
        if (!(response.status === 401 && url.origin === origin && url.pathname === "/auth/session")) {
          errors.push(`${response.status}:${response.url}`)
        }
      }
      if (message.method === "Network.requestWillBeSent") {
        const requested = message.params.request.url
        if (/^https?:/.test(requested) && new URL(requested).origin !== origin) external.push(requested)
      }
    })
    await Promise.all([
      cdp.send("Page.enable"),
      cdp.send("Runtime.enable"),
      cdp.send("Network.enable"),
      cdp.send("Log.enable"),
    ])
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "America/New_York" })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "login form")
    await cdp.evaluate(`(() => {
      const set = (name, value) => {
        const input = document.querySelector('[name="' + name + '"]')
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, value)
        input.dispatchEvent(new Event("input", { bubbles: true }))
      }
      set("username", "artifact")
      set("password", "wire")
      document.querySelector("form").requestSubmit()
    })()`)
    await cdp.waitFor(
      `document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`,
      "the snapshot-backed Activity row",
      15_000,
    )
    const rendered = await cdp.evaluate(`(() => ({
      row: document.querySelector('[data-testid="pg-activity-table"] .entity-row').textContent,
      missing: document.querySelector('[data-testid="cursor-behind"]')?.textContent ?? null,
    }))()`)
    assert.match(rendered.row, /4242/)
    assert.match(rendered.row, /select artifact_wire_contract/)
    assert.equal(rendered.missing, null)
    await assertCompactTimelineContained(cdp, ".workspace > .pg-tabs", "PostgreSQL")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    assert.ok(requests.some(({ path }) => path === `/api/segments/${SEGMENT}/snapshot`))
    const firstApi = requests.findIndex(({ path }) => path.startsWith("/api/"))
    const login = requests.findIndex(({ method, path }) => method === "POST" && path === "/auth/session")
    assert.ok(login > requests.findIndex(({ method, path }) => method === "GET" && path === "/auth/session"))
    assert.ok(firstApi > login, JSON.stringify(requests.slice(0, firstApi + 1), null, 2))
    assert.deepEqual(
      requests.filter(({ authorization }) => authorization !== null).map(({ authorization, method, path }) => ({ authorization, method, path })),
      [{ authorization: "Basic YXJ0aWZhY3Q6d2lyZQ==", method: "POST", path: "/auth/session" }],
    )
    assert.ok(requests.filter(({ path }) => path.startsWith("/api/")).every(({ authorization, cookie, marker }) => (
      authorization === null && cookie === SESSION_COOKIE && marker === "1"
    )))
    const localClocks = await cdp.evaluate(`(() => ({
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent,
      cursorSecondary: document.querySelector('[data-testid="cursor-time"] small')?.textContent ?? null,
      hour: document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent,
      hourContext: document.querySelector('[data-testid="hour-picker-trigger"] small')?.textContent,
      sample: document.querySelector('.cursor-time')?.textContent.includes('Sample'),
      updated: document.querySelector('[data-testid="updated-time"]')?.textContent ?? "",
      updatedSecondary: document.querySelector('[data-testid="updated-time"] small')?.textContent ?? null,
    }))()`)
    assert.match(localClocks.cursor, /01:30:00/)
    assert.doesNotMatch(localClocks.cursor, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.equal(localClocks.cursorSecondary, null)
    assert.equal(localClocks.hour, "01:00–02:00")
    assert.match(localClocks.hourContext, /08\/13\/2026/)
    assert.doesNotMatch(localClocks.hourContext, /GMT|UTC/)
    // The status line reports staleness as an age, not a wall clock (app.tsx UpdatedAge).
    assert.match(localClocks.updated, /^(?:Updated)?\d+ [smh] ago$|^(?:Updated)?\d+ min ago$/)
    assert.doesNotMatch(localClocks.updated, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.equal(localClocks.updatedSecondary, null)
    assert.equal(localClocks.sample, false)

    const initialTheme = await cdp.evaluate(`document.documentElement.dataset.theme`)
    const alternateTheme = initialTheme === "dark" ? "light" : "dark"
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`document.documentElement.dataset.theme === ${JSON.stringify(alternateTheme)}`, "the alternate theme")

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("August") === true`, "the August calendar")
    const initialPicker = await cdp.evaluate(`(() => ({
      currentAction: document.querySelector('[data-testid="hour-current"]')?.tagName,
      currentDay: document.querySelector('[data-testid="day-cell"][data-day="2026-08-13"]')?.getAttribute('aria-pressed'),
      currentHour: document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR}"]')?.getAttribute('aria-pressed'),
      boundaryDayDisabled: document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.disabled,
      boundaryDayVisible: document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.getBoundingClientRect().height > 0,
      headerToggle: document.querySelector('[data-testid="hour-popover"] > header button') !== null,
      hourCount: document.querySelectorAll('[data-testid="hour-cell"]').length,
      popovers: document.querySelectorAll('[data-testid="hour-popover"]').length,
      separateControls: document.querySelector('[data-testid="hour-popover"]').querySelectorAll('input, select').length,
      unavailableDay: document.querySelector('[data-testid="day-cell"][data-day="2026-08-12"]')?.disabled,
    }))()`)
    assert.deepEqual(initialPicker, {
      currentAction: "STRONG",
      currentDay: "true",
      currentHour: "true",
      boundaryDayDisabled: false,
      boundaryDayVisible: true,
      headerToggle: false,
      hourCount: 2,
      popovers: 1,
      separateControls: 0,
      unavailableDay: true,
    })
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const size = await cdp.evaluate(`(() => {
        const popover = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
        const calendar = document.querySelector('[data-testid="day-picker"]').getBoundingClientRect()
        const hours = document.querySelector('[data-testid="hour-grid"]').getBoundingClientRect()
        const boundaryDay = document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]').getBoundingClientRect()
        return {
          calendar: { bottom: calendar.bottom, left: calendar.left, right: calendar.right, top: calendar.top },
          clientHeight: document.documentElement.clientHeight,
          clientWidth: document.documentElement.clientWidth,
          boundaryDay: { bottom: boundaryDay.bottom, left: boundaryDay.left, right: boundaryDay.right, top: boundaryDay.top },
          hours: { bottom: hours.bottom, left: hours.left, right: hours.right, top: hours.top },
          popover: { bottom: popover.bottom, left: popover.left, right: popover.right, top: popover.top },
          scrollWidth: document.documentElement.scrollWidth,
        }
      })()`)
      assert.ok(size.scrollWidth <= size.clientWidth, `${width}px picker overflow: ${JSON.stringify(size)}`)
      assert.ok(size.popover.left >= 0 && size.popover.right <= size.clientWidth, `${width}px horizontal picker bounds: ${JSON.stringify(size)}`)
      assert.ok(size.popover.top >= 0 && size.popover.bottom <= size.clientHeight, `${width}px vertical picker bounds: ${JSON.stringify(size)}`)
      assert.ok(size.boundaryDay.left >= size.calendar.left && size.boundaryDay.right <= size.calendar.right, `${width}px boundary-day visibility: ${JSON.stringify(size)}`)
      assert.ok(size.calendar.right <= size.hours.left, `${width}px picker columns: ${JSON.stringify(size)}`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 480 })
    await settleLayout(cdp)
    const narrow = await cdp.evaluate(`(() => {
      const calendar = document.querySelector('[data-testid="day-picker"]').getBoundingClientRect()
      const hours = document.querySelector('[data-testid="hour-grid"]').getBoundingClientRect()
      const popover = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
      return { calendarBottom: calendar.bottom, clientWidth: document.documentElement.clientWidth, hoursTop: hours.top, popoverLeft: popover.left, popoverRight: popover.right, scrollWidth: document.documentElement.scrollWidth }
    })()`)
    assert.ok(narrow.calendarBottom <= narrow.hoursTop, `narrow picker stack: ${JSON.stringify(narrow)}`)
    assert.ok(narrow.popoverLeft >= 0 && narrow.popoverRight <= narrow.clientWidth && narrow.scrollWidth <= narrow.clientWidth, `narrow picker bounds: ${JSON.stringify(narrow)}`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR}"]').dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowRight' }))`)
    assert.equal(await cdp.evaluate(`document.activeElement?.dataset.instant`), String(HOUR + HOUR_US))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "picker Escape close")
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`document.documentElement.dataset.theme === ${JSON.stringify(initialTheme)}`, "the initial theme")

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.getBoundingClientRect().height > 0`, "boundary day immediately visible")
    await cdp.evaluate(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.getAttribute('aria-pressed') === "true"`, "the local August 9 hours")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="hour-cell"]').length`), 1)
    await cdp.evaluate(`document.querySelector('[data-testid="hour-cell"][data-instant="${AUGUST_HOUR}"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "23:00–00:00"`, "the exact local boundary hour")
    await cdp.waitFor(`Math.floor(Number(new URLSearchParams(location.search).get("at")) / ${HOUR_US}) * ${HOUR_US} === ${AUGUST_HOUR}`, "the exact boundary address")
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)
    const augustRequest = requests.find(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("from") === String(AUGUST_HOUR))
    assert.notEqual(augustRequest, undefined)
    assert.equal(new URLSearchParams(augustRequest.query).get("to"), String(AUGUST_HOUR + HOUR_US - 1))

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.evaluate(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-13"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR + HOUR_US}"]') !== null`, "the recorded August 13 hour")
    await cdp.evaluate(`document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR + HOUR_US}"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "02:00–03:00"`, "the recorded local August 13 selection")
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "23:00–00:00"`, "the previous catalogued instant")
    await cdp.waitFor(`Math.floor(Number(new URLSearchParams(location.search).get("at")) / ${HOUR_US}) * ${HOUR_US} === ${AUGUST_HOUR}`, "the previous catalogued address")
    const originalAddress = await cdp.evaluate(`(() => { const url = new URL(location.href); url.searchParams.set("at", "${AT}"); return url.href })()`)
    await cdp.send("Page.navigate", { url: originalAddress })
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "01:00–02:00"`, "the original exact selection", 15_000)

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[aria-label="Next month"]') !== null`, "the reopened month controls")
    await cdp.evaluate(`document.querySelector('[aria-label="Next month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("December 2026") === true`, "the next catalog month")
    await cdp.evaluate(`document.querySelector('[aria-label="Next month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("January 2027") === true`, "the next year month")
    await cdp.evaluate(`document.querySelector('[aria-label="Previous month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("December 2026") === true`, "the previous year month")
    await cdp.evaluate(`document.querySelector('[aria-label="Previous month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("August 2026") === true`, "the selected month restored")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))`)

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click(); document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.toLocaleLowerCase("ru").includes("август") === true`, "the Russian calendar month")
    const russianPicker = await cdp.evaluate(`(() => ({
      context: document.querySelector('[data-testid="hour-popover"] > header > span')?.textContent ?? null,
      day: document.querySelector('[data-testid="day-cell"][data-day="2026-08-13"]')?.getAttribute('aria-label'),
      text: document.querySelector('[data-testid="hour-popover"]')?.textContent ?? "",
      zoneLabel: ${ZONE_LABEL} ?? "",
    }))()`)
    assert.equal(russianPicker.context, null)
    assert.match(russianPicker.day, /13\.08\.2026/)
    assert.doesNotMatch(russianPicker.text, /GMT|UTC/)
    assert.equal(russianPicker.zoneLabel, "Время браузера")
    assert.equal(await cdp.evaluate(`document.querySelector('.cursor-time')?.textContent.includes('Отсчёт')`), false)
    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'mouse' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "picker outside close")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await switchZone(cdp, "utc")
    await cdp.waitFor(`document.documentElement.lang === "ru" && ${ZONE_VALUE} === "utc" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("05:30:00")`, "the UTC re-render")
    const utcClocks = await cdp.evaluate(`(() => ({
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent,
      cursorSecondary: document.querySelector('[data-testid="cursor-time"] small') !== null,
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent,
      hourZoneSuffix: document.querySelector('[data-testid="hour-picker-trigger"] small')?.textContent.includes('UTC') ?? false,
      updated: document.querySelector('[data-testid="updated-time"]')?.textContent ?? "",
      updatedSecondary: document.querySelector('[data-testid="updated-time"] small') !== null,
      zoneLabel: ${ZONE_LABEL} ?? "",
    }))()`)
    assert.equal(utcClocks.zoneLabel, "UTC")
    assert.match(utcClocks.cursor, /05:30:00/)
    assert.match(utcClocks.hour, /05:00–06:00/)
    assert.doesNotMatch(utcClocks.cursor, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.doesNotMatch(utcClocks.hour, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.match(utcClocks.updated, /^(?:Обновлено)?\d+ \S+ назад$/)
    assert.doesNotMatch(utcClocks.updated, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.equal(utcClocks.cursorSecondary, false)
    assert.equal(utcClocks.hourZoneSuffix, false)
    assert.equal(utcClocks.updatedSecondary, false)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await switchZone(cdp, "browser")
    await cdp.waitFor(`document.documentElement.lang === "en" && ${ZONE_VALUE} === "browser" && ${ZONE_LABEL} === "Browser time" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("01:30:00")`, "the local-time restore")

    await cdp.evaluate(`([...document.querySelectorAll(".pg-tabs button")].find((button) => button.textContent === "Tables")).click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row').length === 1`, "the relation wire row")
    const postgresJoinGap = await cdp.evaluate(`(() => {
      const timeline = document.querySelector('.timeline-shell').getBoundingClientRect()
      const controls = document.querySelector('.workspace > .pg-tabs').getBoundingClientRect()
      return controls.top - timeline.bottom
    })()`)
    assert.ok(postgresJoinGap <= 1, `PostgreSQL major-region gap ${postgresJoinGap}px`)
    const relationRow = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-row').textContent`)
    assert.match(relationRow, /artifact_db/)
    assert.match(relationRow, /artifact_table/)
    const tablePresentation = await cdp.evaluate(`(() => ({
      cells: [...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row [role="cell"]')].map((cell) => cell.textContent),
      headers: [...document.querySelectorAll('[data-testid="pg-tables-table"] [role="columnheader"]')].map((header) => header.textContent),
    }))()`)
    assert.equal(tablePresentation.cells.includes("42"), false)
    assert.equal(tablePresentation.cells.includes("73"), false)
    assert.doesNotMatch(tablePresentation.headers.join(" "), /Database ID|Table OID|Index OID/)
    const relationRequest = requests.find(({ query }) => query.includes("section=pg_stat_user_tables") && query.includes("group=object"))
    assert.notEqual(relationRequest, undefined, JSON.stringify(requests.map(({ query }) => query), null, 2))
    const relationQuery = new URLSearchParams(relationRequest.query)
    assert.equal(relationQuery.get("group"), "object")
    assert.equal(relationQuery.get("page_size"), "200")
    assert.equal(relationQuery.getAll("field").includes("datid"), true)
    assert.equal(relationQuery.getAll("field").includes("relid"), true)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') !== null`, "the table detail")
    const tableDetail = await cdp.evaluate(`(() => {
      const detail = document.querySelector('[data-testid="pg-relation-detail"]')
      const rows = [...detail.querySelectorAll('dl > .detail-row')]
      return {
        compactRows: rows.length,
        maxRowHeight: Math.max(...rows.map((row) => row.getBoundingClientRect().height)),
        aligned: rows.every((row) => { const term = row.querySelector('dt').getBoundingClientRect(); const value = row.querySelector('dd').getBoundingClientRect(); return value.left > term.left && Math.abs(value.top - term.top) <= 2 }),
        labels: [...detail.querySelectorAll('dt')].map((label) => label.textContent),
        values: [...detail.querySelectorAll('dd')].map((value) => value.textContent),
      }
    })()`)
    assert.ok(tableDetail.compactRows > 0 && tableDetail.maxRowHeight <= 35 && tableDetail.aligned, JSON.stringify(tableDetail))
    assert.doesNotMatch(tableDetail.labels.join(" "), /Database ID|Table OID|Index OID/)
    assert.equal(tableDetail.values.includes("42"), false)
    assert.equal(tableDetail.values.includes("73"), false)
    await cdp.evaluate(`document.querySelector(".inspector-close").click()`)

    relationMode = "long"
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "Size and buffers")).click()`)
    await cdp.waitFor(`(() => { const node = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); return node !== null && node.scrollWidth > node.clientWidth })()`, "the wide size and buffers table")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="virtual-body"]')?.style.height === "4800px"`, "the long virtual relation table")
    const estimate = await cdp.evaluate(`(() => {
      const node = [...document.querySelectorAll('[data-testid="pg-tables-table"] [title]')].find((cell) => cell.title.includes('9,007,199,254,740,993'))
      return node === undefined ? null : { label: node.getAttribute('aria-label'), text: node.textContent, title: node.title }
    })()`)
    assert.deepEqual(estimate, { label: "≈9,007,199,254,740,993 rows", text: "≈9.01E15 rows", title: "≈9,007,199,254,740,993 rows" })
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768], [1024, 1366]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const initial = await cdp.evaluate(`(() => {
        const table = document.querySelector('[data-testid="pg-tables-table"]')
        const body = table.querySelector('.entity-scroll')
        const layout = document.querySelector('.pg-entity-layout')
        const paging = document.querySelector('[data-testid="table-paging"]')
        const sticky = table.querySelector('.entity-header-cell.entity-sticky')
        const bodyRect = body.getBoundingClientRect()
        const contentBottom = Math.max(layout.getBoundingClientRect().bottom, paging?.getBoundingClientRect().bottom ?? 0)
        const firstRow = table.querySelector('.entity-row')
        return {
          bodyClientWidth: body.clientWidth,
          bodyHeight: body.clientHeight,
          bodyScrollWidth: body.scrollWidth,
          clientWidth: document.documentElement.clientWidth,
          clientHeight: document.documentElement.clientHeight,
          contentBottom,
          documentScrollWidth: document.documentElement.scrollWidth,
          railBottom: bodyRect.bottom,
          railHeight: body.offsetHeight - body.clientHeight,
          railTabIndex: body.tabIndex,
          railVisible: bodyRect.bottom <= document.documentElement.clientHeight + .5,
          rowHeight: firstRow?.getBoundingClientRect().height ?? 0,
          stickyLeft: sticky.getBoundingClientRect().left,
          tableLeft: bodyRect.left,
          virtualHeight: table.querySelector('[data-testid="virtual-body"]').getBoundingClientRect().height,
          visibleRows: [...table.querySelectorAll('.entity-row')].filter((row) => {
            const rect = row.getBoundingClientRect()
            return rect.bottom > bodyRect.top && rect.top < bodyRect.bottom
          }).length,
        }
      })()`)
      assert.ok(initial.bodyScrollWidth > initial.bodyClientWidth, `${width}px wide table: ${JSON.stringify(initial)}`)
      // The virtualizer fills the scroll body: as many rows as fit, no gap.
      assert.ok(initial.visibleRows >= Math.floor(initial.bodyHeight / initial.rowHeight) - 1, `${width}x${height} visible rows: ${JSON.stringify(initial)}`)
      assert.equal(initial.virtualHeight, 4800, `${width}x${height} virtual height`)
      assert.ok(initial.contentBottom <= initial.clientHeight && initial.clientHeight - initial.contentBottom <= 24, `${width}x${height} remaining viewport: ${JSON.stringify(initial)}`)
      assert.equal(initial.railTabIndex, 0, `${width}px focusable horizontal rail`)
      assert.ok(initial.railHeight > 0 && initial.railVisible, `${width}px visible horizontal rail: ${JSON.stringify(initial)}`)
      assert.ok(Math.abs(initial.stickyLeft - initial.tableLeft) <= 1, `${width}px initial sticky identity: ${JSON.stringify(initial)}`)
      assert.ok(initial.documentScrollWidth <= initial.clientWidth, `${width}px relation document overflow: ${JSON.stringify(initial)}`)

      await cdp.evaluate(`(() => { const body = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); body.focus(); body.scrollLeft = body.scrollWidth })()`)
      await cdp.waitFor(`(() => {
        const body = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll')
        return body.scrollLeft >= body.scrollWidth - body.clientWidth - 1
      })()`, `${width}px rightmost table column`)
      const end = await cdp.evaluate(`(() => {
        const table = document.querySelector('[data-testid="pg-tables-table"]')
        const body = table.querySelector('.entity-scroll')
        const cells = table.querySelectorAll('.entity-header-cell')
        const first = cells[0].getBoundingClientRect()
        const last = cells[cells.length - 1].getBoundingClientRect()
        const viewport = body.getBoundingClientRect()
        return { activeRail: document.activeElement === body, firstLeft: first.left, lastRight: last.right, viewportLeft: viewport.left, viewportRight: viewport.left + body.clientWidth }
      })()`)
      assert.equal(end.activeRail, true, `${width}px focused horizontal rail`)
      assert.ok(Math.abs(end.firstLeft - end.viewportLeft) <= 1, `${width}px sticky identity after horizontal scroll: ${JSON.stringify(end)}`)
      const endGutter = end.viewportRight - end.lastRight
      assert.ok(endGutter >= 7 && endGutter <= 9, `${width}px stable rightmost-column gutter: ${JSON.stringify({ ...end, endGutter })}`)
      await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').scrollLeft = 0`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`(() => { const body = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); body.scrollTop = body.scrollHeight })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="virtual-body"]')?.style.height === "4920px"`, "the one guarded relation cursor page")
    const cursorPages = requests.filter(({ query }) => new URLSearchParams(query).get("cursor") === "viewport-page-two")
    assert.equal(cursorPages.length, 1, JSON.stringify(cursorPages))
    await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').scrollTop = 0`)
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row')].some((row) => row.getAttribute('aria-label') === 'artifact_db.public.artifact_table')`, "the first virtual relation row")

    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row')].find((row) => row.getAttribute('aria-label') === 'artifact_db.public.artifact_table')).click()`)
    const alignedDetail = await cdp.evaluate(`(() => {
      const inspector = document.querySelector('[data-testid="inspector"]').getBoundingClientRect()
      const body = document.querySelector('.inspector-body').getBoundingClientRect()
      const detail = document.querySelector('[data-testid="pg-relation-detail"]').getBoundingClientRect()
      return { bodyBottom: body.bottom, bodyTop: body.top, detailBottom: detail.bottom, detailTop: detail.top, inspectorBottom: inspector.bottom, inspectorTop: inspector.top, viewport: innerHeight }
    })()`)
    assert.ok(alignedDetail.inspectorTop >= -1 && alignedDetail.inspectorBottom <= alignedDetail.viewport + 1
      && alignedDetail.detailTop >= alignedDetail.bodyTop - 1 && alignedDetail.detailBottom <= alignedDetail.bodyBottom + 1,
    JSON.stringify(alignedDetail))
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the Russian estimate labels")
    const russianEstimate = await cdp.evaluate(`(() => {
      const nodes = [...document.querySelectorAll('[data-testid="pg-relation-detail"] [title]')]
      const exact = nodes.find((node) => node.title.includes('9 007 199 254 740 993'))
      const toast = nodes.find((node) => node.title.includes('713 456'))
      const labels = [...document.querySelectorAll('[data-testid="pg-relation-detail"] dt')].map((node) => node.textContent.trim())
      return { exact: exact?.title ?? null, labels, toast: toast?.textContent ?? null, toastExact: toast?.title ?? null }
    })()`)
    assert.equal(russianEstimate.exact, "≈9 007 199 254 740 993 строки")
    assert.equal(russianEstimate.toast, "≈713 тыс. строк")
    assert.equal(russianEstimate.toastExact, "≈713 456 строк")
    assert.equal(russianEstimate.labels.filter((label) => /buffer|blks/i.test(label)).some((label) => /[А-Яа-яЁё]/u.test(label)), false)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "en"`, "the English estimate restore")
    relationMode = "single"
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-link"]') !== null`, "the table-to-index link")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-link"]').click()`)
    await cdp.waitFor(`location.search.includes("view=pg.indexes") && location.search.includes("datid=42") && location.search.includes("relid=73")`, "numeric table identity in index navigation")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("artifact_index") === true`, "the linked index row")
    const linkedScope = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] [data-testid="table-status"]').textContent`)
    assert.match(linkedScope, /artifact_db · artifact_table/)
    assert.doesNotMatch(linkedScope, /\b(?:42|73)\b|\bOID\b/)

    const indexRow = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    assert.match(indexRow, /artifact_db/)
    assert.match(indexRow, /public/)
    assert.match(indexRow, /artifact_table/)
    assert.match(indexRow, /artifact_index/)
    const indexPresentation = await cdp.evaluate(`(() => ({
      cells: [...document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row [role="cell"]')].map((cell) => cell.textContent),
      headers: [...document.querySelectorAll('[data-testid="pg-indexes-table"] [role="columnheader"]')].map((header) => header.textContent),
    }))()`)
    for (const oid of ["42", "73", "74"]) assert.equal(indexPresentation.cells.includes(oid), false)
    assert.doesNotMatch(indexPresentation.headers.join(" "), /Database ID|Table OID|Index OID/)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-exact-indexdef"]')?.textContent.includes("CREATE UNIQUE INDEX artifact_index") === true`, "the exact index definition")
    assert.ok(requests.some(({ query }) => query.includes("row_ordinal=8") && !query.includes("text=")))
    const indexDetail = await cdp.evaluate(`(() => ({
      compactRows: document.querySelectorAll('[data-testid="pg-relation-detail"] dl > .detail-row').length,
      html: document.querySelector('[data-testid="pg-relation-detail"]')?.outerHTML ?? null,
      labels: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dt')].map((label) => label.textContent),
      values: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dd')].map((value) => value.textContent),
    }))()`)
    assert.ok(indexDetail.compactRows > 0, indexDetail.html)
    assert.doesNotMatch(indexDetail.labels.join(" "), /Database ID|Table OID|Index OID/)
    for (const oid of ["42", "73", "74"]) assert.equal(indexDetail.values.includes(oid), false)
    await cdp.evaluate(`document.querySelector(".inspector-close").click()`)
    relationMode = "short"
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "State")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] [data-testid="virtual-body"]')?.style.height === "72px"`, "the short relation set")
    const shortTable = await cdp.evaluate(`(() => {
      const body = document.querySelector('[data-testid="pg-indexes-table"] .entity-scroll')
      const bounds = body.getBoundingClientRect()
      const header = document.querySelector('[data-testid="pg-indexes-table"] .entity-head').getBoundingClientRect()
      return {
        axis: body.dataset.scrollAxis,
        bottom: bounds.bottom,
        clientHeight: body.clientHeight,
        height: bounds.height,
        overflowY: getComputedStyle(body).overflowY,
        railHeight: body.offsetHeight - body.clientHeight,
        rows: document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length,
        scrollHeight: body.scrollHeight,
        verticalOwner: body.scrollHeight > body.clientHeight + 1,
        viewportHeight: document.documentElement.clientHeight,
        header: header.height,
        virtual: document.querySelector('[data-testid="pg-indexes-table"] [data-testid="virtual-body"]').getBoundingClientRect().height,
      }
    })()`)
    assert.equal(shortTable.rows, 3)
    assert.equal(shortTable.virtual, 72)
    assert.equal(shortTable.axis, "horizontal")
    assert.equal(shortTable.overflowY, "hidden")
    assert.equal(shortTable.verticalOwner, false)
    assert.ok(shortTable.scrollHeight <= shortTable.clientHeight + 1, JSON.stringify(shortTable))
    assert.ok(Math.abs(shortTable.height - shortTable.header - shortTable.virtual - shortTable.railHeight) <= 1, JSON.stringify(shortTable))
    assert.ok(shortTable.bottom <= shortTable.viewportHeight + 1, JSON.stringify(shortTable))

    const beforeOidSearch = requests.length
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "74")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await delay(700)
    const oidSearchRequest = requests.slice(beforeOidSearch).find(({ query }) => query.includes("section=pg_stat_user_indexes") && query.includes("search=74"))
    assert.notEqual(oidSearchRequest, undefined, JSON.stringify(requests.slice(beforeOidSearch), null, 2))
    const oidSearchQuery = new URLSearchParams(oidSearchRequest.query)
    assert.equal(oidSearchQuery.get("where.datid"), "42")
    assert.equal(oidSearchQuery.get("where.relid"), "73")
    for (const field of ["datid", "relid", "indexrelid"]) assert.equal(oidSearchQuery.getAll("field").includes(field), true, field)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await delay(400)

    const clickRelation = async (label) => {
      await cdp.evaluate(`([...document.querySelectorAll(".workspace .lensbar button")].find((button) => button.textContent === ${JSON.stringify(label)})).click()`)
    }
    await clickRelation("Schemas")
    await cdp.waitFor(`location.search.includes("level=schema") && document.querySelector('[data-testid="pg-indexes-table"] .entity-row') !== null`, "schema level")
    await clickRelation("Databases")
    await cdp.waitFor(`location.search.includes("level=database") && document.querySelector('[data-testid="pg-indexes-table"] .entity-row') !== null`, "database level")
    const databasePresentation = await cdp.evaluate(`(() => ({
      cells: [...document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row [role="cell"]')].map((cell) => cell.textContent),
      headers: [...document.querySelectorAll('[data-testid="pg-indexes-table"] [role="columnheader"]')].map((header) => header.textContent),
    }))()`)
    assert.equal(databasePresentation.cells.includes("42"), false)
    assert.doesNotMatch(databasePresentation.headers.join(" "), /Database ID|Table OID|Index OID/)
    await cdp.evaluate("history.back()")
    await cdp.waitFor(`location.search.includes("level=schema")`, "schema level restored by browser back")
    await clickRelation("Databases")
    await cdp.waitFor(`location.search.includes("level=database")`, "database rollup restored")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-drill"]') !== null`, "explicit rollup drill action")
    assert.ok(await cdp.evaluate(`document.querySelectorAll('[data-testid="pg-relation-detail"] dl > .detail-row').length > 0`))
    await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-drill"]').click()`)
    await cdp.waitFor(`location.search.includes("level=schema") && location.search.includes("datid=42")`, "database-scoped schema drill")
    await cdp.evaluate(`([...document.querySelectorAll(".workspace .lensbar button")].find((button) => button.textContent === "All")).click()`)
    await cdp.waitFor(`!location.search.includes("level=") && !location.search.includes("datid=")`, "reset to all index objects")

    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "State")).click()`)
    await clickRelation("Databases")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("363") === true`, "categorical index counts")
    const englishCounts = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    assert.doesNotMatch(englishCounts, /(?:363|223|111|0)\/s/)
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "Usage")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("3/s") === true`, "exact English rate unit")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("3/с") === true`, "exact Russian rate unit")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "State")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("363") === true`, "Russian categorical counts")
    const russianCounts = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    assert.doesNotMatch(russianCounts, /(?:363|223|111|0)\/с/)
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const size = await cdp.evaluate(`({ clientWidth: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth })`)
      assert.ok(size.scrollWidth <= size.clientWidth, `${width}px relation overflow: ${JSON.stringify(size)}`)
    }
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click(); document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.querySelector(".system-main") !== null`, "the host workspace")
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await systemPage
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]') !== null`, "the crowded Russian loading bar")
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const size = await cdp.evaluate(`(() => {
        const clientWidth = document.documentElement.clientWidth
        const overflow = [...document.querySelectorAll("body *")].flatMap((node) => {
          const rect = node.getBoundingClientRect()
          return rect.right > clientWidth + 0.5 ? [{ className: node.className, right: rect.right, tag: node.tagName }] : []
        }).slice(0, 8)
        return { clientWidth, overflow, scrollWidth: document.documentElement.scrollWidth }
      })()`)
      assert.ok(size.scrollWidth <= size.clientWidth, `${width}px document overflow: ${JSON.stringify(size)}`)
    }
    ndjson(heldSystemPage, systemSnapshotRecords())
    heldSystemPage = null
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)

    await cdp.evaluate(`([...document.querySelectorAll(".source-tabs button")].find((button) => button.textContent === "Events")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-item"] button') !== null`, "the statement finding")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 360 })
    await settleLayout(cdp)
    const eventSearch = await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="events-console"] input[type="search"]')
      const label = input.closest('label')
      const icon = label.querySelector('svg')
      const box = (node) => { const value = node.getBoundingClientRect(); return { left: value.left, right: value.right, width: value.width } }
      return { client: document.documentElement.clientWidth, icon: box(icon), input: box(input), label: box(label), nav: [...document.querySelectorAll('.source-tabs button')].map((button) => button.textContent), placeholder: input.placeholder, scroll: document.documentElement.scrollWidth }
    })()`)
    assert.deepEqual(eventSearch.nav, ["Host", "Processes", "PostgreSQL", "Events"])
    assert.equal(eventSearch.placeholder, "Текст или field:value AND size>100MB")
    assert.ok(eventSearch.icon.right <= eventSearch.input.left + 1, JSON.stringify(eventSearch))
    assert.ok(eventSearch.label.left >= -1 && eventSearch.label.right <= eventSearch.client + 1 && eventSearch.scroll <= eventSearch.client, JSON.stringify(eventSearch))
    await cdp.evaluate(`document.querySelector('[data-testid="events-console"] [aria-label="Синтаксис и поля поиска"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"] [role="dialog"]') !== null`, "the narrow RU search help")
    const searchHelp = await cdp.evaluate(`(() => {
      const overlay = document.querySelector('[data-testid="search-help"]')
      const dialog = overlay.querySelector('[role="dialog"]')
      const bounds = dialog.getBoundingClientRect()
      return {
        active: document.activeElement?.getAttribute("aria-label") ?? "",
        ariaModal: dialog.getAttribute("aria-modal"),
        fields: dialog.textContent,
        left: bounds.left, right: bounds.right,
        viewport: document.documentElement.clientWidth,
        scroll: document.documentElement.scrollWidth,
      }
    })()`)
    assert.equal(searchHelp.ariaModal, "true")
    assert.match(searchHelp.active, /Закрыть/)
    assert.match(searchHelp.fields, /kind/)
    assert.match(searchHelp.fields, /source/)
    assert.match(searchHelp.fields, /category/)
    assert.doesNotMatch(searchHelp.fields, /queryid_stat_statements|planid|relname/)
    assert.ok(searchHelp.left >= -1 && searchHelp.right <= searchHelp.viewport + 1 && searchHelp.scroll <= searchHelp.viewport, JSON.stringify(searchHelp))
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "search help closed by Escape")
    assert.equal(await cdp.evaluate(`document.activeElement?.getAttribute("aria-label")`), "Синтаксис и поля поиска")
    await assertSearchControlContained(cdp, "Events search")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[data-testid="event-item"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="entity-context-filter"]') !== null`, "the exact statement context")
    await contextPage
    const preview = await cdp.evaluate(`(() => ({
      chip: document.querySelector('[data-testid="entity-context-filter"]')?.textContent ?? "",
      detail: document.querySelector(".pg-detail") !== null,
      row: document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent ?? "",
      search: document.querySelector('[data-testid="table-filter"]')?.getAttribute("aria-label") ?? "",
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
    }))()`)
    assert.match(preview.chip, /Query 9007199254740991 · operators · reporterShow all/)
    assert.doesNotMatch(preview.chip, /queryid=|userid=|dbid=|toplevel=/)
    assert.match(preview.row, /select artifact_exact_context/)
    assert.match(preview.search, /Search rows/)
    assert.match(preview.status, /filtered page is loading/i)
    assert.doesNotMatch(preview.status, /Loaded 0 of 0/)
    assert.equal(preview.detail, false)

    ndjson(heldContextPage, statementRecords(true))
    heldContextPage = null
    await cdp.waitFor(
      `document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent.includes("Loaded 1 of 1") === true`,
      "the settled identity-filtered page count",
    )
    const identityRequest = requests.find(({ query }) => query.includes("where.queryid=") && query.includes("page_size=200"))
    assert.notEqual(identityRequest, undefined, JSON.stringify(requests.map(({ query }) => query), null, 2))
    const identityQuery = new URLSearchParams(identityRequest.query)
    assert.equal(identityQuery.get("where.queryid"), "9007199254740991")
    assert.equal(identityQuery.get("where.userid"), "10")
    assert.equal(identityQuery.get("where.dbid"), "20")
    assert.equal(identityQuery.get("where.toplevel"), "true")
    assert.equal(identityQuery.get("type_id"), "1002003")
    assert.equal(identityQuery.get("page_size"), "200")
    const exactRequest = requests.find(({ query }) => query.includes("row_ordinal=91"))
    assert.notEqual(exactRequest, undefined)
    assert.equal(new URLSearchParams(exactRequest.query).has("page_size"), false)
    await delay(600)
    assert.equal(requests.filter(({ query }) => query.includes("where.queryid=") && query.includes("page_size=200")).length, 1)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector(".pg-detail") !== null`, "detail after explicit row selection")
    await cdp.evaluate(`document.querySelector(".inspector-close").click(); document.querySelector('[data-testid="entity-context-filter"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent.includes("Loaded 50 of 4,807") === true`, "the paged full statement set")
    await cdp.waitFor(`document.querySelector('[data-testid="table-paging"]') !== null`, "active statement paging")

    const invalidSearchStart = requests.length
    const lastValidRows = await cdp.evaluate(`(() => ({
      first: document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent ?? "",
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
    }))()`)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "query_id:9007199254740991 AND taname:orders")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "query_id:9007199254740991 AND taname:orders", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-error"] mark')?.textContent === "taname"`, "the exact invalid selector span")
    const invalidSearch = await cdp.evaluate(`(() => ({
      error: document.querySelector('[data-testid="search-error"]')?.textContent ?? "",
      first: document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent ?? "",
      invalid: document.querySelector('[data-testid="table-filter"]')?.getAttribute("aria-invalid"),
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      url: new URL(location.href).searchParams.get("find"),
    }))()`)
    assert.match(invalidSearch.error, /Unknown field “taname”/)
    assert.equal(invalidSearch.invalid, "true")
    assert.deepEqual({ first: invalidSearch.first, status: invalidSearch.status }, lastValidRows)
    assert.equal(invalidSearch.url, null)
    assert.equal(requests.slice(invalidSearchStart).some(({ query }) => query.includes("taname")), false)

    const quantitativeSearch = "exec_time_rate>500ms/s AND call_rate>1/s"
    await cdp.evaluate('(() => { const input = document.querySelector("[data-testid=table-filter]"); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "exec_time_rate>500ms/s AND call_rate>1/s"); input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertFromPaste" })); input.form.requestSubmit() })()')
    await cdp.waitFor('new URL(location.href).searchParams.get("find") === "exec_time_rate>500ms/s AND call_rate>1/s" && document.querySelectorAll("[data-testid=search-chips] button").length === 2', "quantitative statement chips")
    const quantitativeState = await cdp.evaluate('(() => ({ aria: document.querySelector("[data-testid=search-chips]")?.getAttribute("aria-label") ?? "", fields: [...document.querySelectorAll("[data-testid=search-chips] strong")].map((field) => field.textContent), text: document.querySelector("[data-testid=search-chips]")?.textContent ?? "" }))()')
    assert.deepEqual(quantitativeState.fields, ["Execution time/s", "Calls/s"])
    assert.match(quantitativeState.text, /> 500 ms\/sANDCalls\/s · > 1 \/s/)
    assert.match(quantitativeState.aria, /exec_time_rate>500ms\/s AND call_rate>1\/s/)
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === quantitativeSearch))

    const unitErrorStart = requests.length
    await cdp.evaluate('(() => { const input = document.querySelector("[data-testid=table-filter]"); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "exec_time_rate>500ms"); input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertFromPaste" })); input.form.requestSubmit() })()')
    await cdp.waitFor('document.querySelector("[data-testid=search-error]")?.textContent.includes("not valid for this field") === true', "duration-rate unit error")
    assert.equal(await cdp.evaluate('new URL(location.href).searchParams.get("find")'), quantitativeSearch)
    assert.equal(requests.slice(unitErrorStart).some(({ query }) => new URLSearchParams(query).get("search") === "exec_time_rate>500ms"), false)

    for (const locale of ["en", "ru"]) {
      await cdp.evaluate('document.querySelector("[data-testid=locale-' + locale + ']").click()')
      await cdp.waitFor('document.documentElement.lang === "' + locale + '"', locale + " quantitative help locale")
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
        const helpLabel = locale === "en" ? "Search syntax and fields" : "Синтаксис и поля поиска"
        await cdp.waitFor(`document.querySelector('[aria-label="${helpLabel}"]') !== null`, locale + " quantitative help trigger at " + width)
        await cdp.evaluate(`document.querySelector('[aria-label="${helpLabel}"]').click()`)
        await cdp.waitFor('document.querySelector("[data-testid=search-help]") !== null', locale + " quantitative help at " + width)
        const help = await cdp.evaluate('(() => { const dialog = document.querySelector("[data-testid=search-help] [role=dialog]"); const bounds = dialog.getBoundingClientRect(); return { fields: dialog.textContent, left: bounds.left, right: bounds.right, scroll: document.documentElement.scrollWidth, viewport: document.documentElement.clientWidth } })()')
        assert.match(help.fields, /exec_time_rate/)
        assert.match(help.fields, /wal_rate/)
        assert.doesNotMatch(help.fields, /cpu_cores|rmem_kb|total_exec_time|shared_blks_read/)
        assert.ok(help.left >= -1 && help.right <= help.viewport + 1 && help.scroll <= help.viewport, locale + " " + width + ": " + JSON.stringify(help))
        await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
        await cdp.waitFor('document.querySelector("[data-testid=search-help]") === null', locale + " quantitative help close at " + width)
      }
    }
    await cdp.evaluate('document.querySelector("[data-testid=locale-en]").click()')
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })

    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "(query_id:9007199254740991 or db:operators) and role:reporter")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "(query_id:9007199254740991 or db:operators) and role:reporter", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "(query_id:9007199254740991 OR database:operators) AND role:reporter" && document.querySelectorAll('[data-testid="search-chips"] button').length === 3`, "canonical pasted boolean search chips")
    const canonicalSearchState = await cdp.evaluate(`(() => ({
      accessible: document.querySelector('[data-testid="search-chips"]').getAttribute("aria-label"),
      input: document.querySelector('[data-testid="table-filter"]').value,
      labels: [...document.querySelectorAll('[data-testid="search-chips"] button')].map((button) => button.getAttribute("aria-label")),
      text: document.querySelector('[data-testid="search-chips"]').textContent,
    }))()`)
    assert.equal(canonicalSearchState.input, "(query_id:9007199254740991 OR database:operators) AND role:reporter")
    assert.match(canonicalSearchState.accessible, /Applied search filters: \(query_id:9007199254740991 OR database:operators\) AND role:reporter/)
    assert.match(canonicalSearchState.text, /\(query_id: 9007199254740991ORdatabase: operators\)ANDrole: reporter/)
    assert.deepEqual(canonicalSearchState.labels, [
      "Remove query_id: 9007199254740991",
      "Remove database: operators",
      "Remove role: reporter",
    ])
    await cdp.evaluate(`(() => {
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text) => { window.__copiedSearch = text } } })
      document.querySelector('[aria-label="Search syntax and fields"]').click()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') !== null`, "boolean search help")
    const copiedExample = await cdp.evaluate(`(() => {
      const button = [...document.querySelectorAll('[data-testid="search-help"] button')].find((candidate) => candidate.querySelector("code") !== null)
      button.click()
      return button.textContent
    })()`)
    assert.equal(await cdp.evaluate(`window.__copiedSearch ?? ""`), copiedExample)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "boolean search help closed")
    await cdp.evaluate(`document.querySelector('[data-testid="search-chips"] button').focus()`)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: " ", code: "Space", windowsVirtualKeyCode: 32 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: " ", code: "Space", windowsVirtualKeyCode: 32 })
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter"`, "keyboard chip removal")
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "(query_id:9007199254740991 OR database:operators) AND role:reporter" && document.querySelectorAll('[data-testid="search-chips"] button').length === 3`, "Back restores the canonical boolean search")
    await cdp.evaluate(`history.forward()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter" && document.querySelectorAll('[data-testid="search-chips"] button').length === 2`, "Forward restores the removed boolean chip")
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null && document.querySelector('[data-testid="search-chips"]') === null`, "clear restores ordinary rows")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent.includes("Loaded 50 of 4,807") === true`, "the search-clear page")

    await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector(".pg-detail") !== null`, "detail beside active paging")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-statement-related-plans"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans" && new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter AND query_id:9007199254740991"`, "Statement opens every matching Plan through public search")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "database:operators AND role:reporter AND query_id:9007199254740991"))
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.statements" && document.querySelector(".pg-detail") !== null`, "Back restores Statement detail")
    await assertDetailRowsDoNotOverlap(cdp, "Statement exact ID detail")
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const placement = await cdp.evaluate(`(() => {
        const layout = document.querySelector('[data-testid="pg-entity-layout"]')
        const workspace = document.querySelector('.workspace').getBoundingClientRect()
        const table = layout.querySelector('[data-testid="pg-statements-table"]').getBoundingClientRect()
        const paging = layout.querySelector('[data-testid="table-paging"]').getBoundingClientRect()
        const inspector = document.querySelector('[data-testid="inspector"]').getBoundingClientRect()
        return {
          inspector: { left: inspector.left, top: inspector.top },
          paging: { top: paging.top },
          table: { bottom: table.bottom },
          workspace: { right: workspace.right, top: workspace.top },
        }
      })()`)
      assert.ok(placement.inspector.top <= placement.workspace.top + 1, `${width}px Inspector starts with workspace: ${JSON.stringify(placement)}`)
      assert.ok(placement.inspector.left >= placement.workspace.right - 1, `${width}px Inspector beside workspace: ${JSON.stringify(placement)}`)
      assert.ok(placement.paging.top >= placement.table.bottom - 1, `${width}px paging below table: ${JSON.stringify(placement)}`)
    }

    await cdp.evaluate(`document.querySelector('.inspector-close').click(); ([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Plans'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plans-table"] .entity-row') !== null`, "the Vadv Plans row")
    const planTable = await cdp.evaluate(`(() => ({
      headers: [...document.querySelectorAll('[data-testid="pg-plans-table"] [role="columnheader"]')].map((header) => header.textContent),
      row: document.querySelector('[data-testid="pg-plans-table"] .entity-row').textContent,
    }))()`)
    assert.match(planTable.headers.join(" "), /Plan summary/)
    assert.match(planTable.headers.join(" "), /Related query ID/i)
    assert.doesNotMatch(planTable.headers.join(" "), /(?:^|\s)Query ID(?:\s|$)/)
    assert.match(planTable.row, /Merge Join.*cost=0\.85\.\.81\.42/)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="related-planid"]') !== null`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="related-queryid_stat_statements"]') !== null`), true)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'planning_share>20% AND call_rate>1/s')
      input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste' }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "planning_share>20% AND call_rate>1/s"`, "quantitative Plans search")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "planning_share>20% AND call_rate>1/s"))
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="search-chips"] strong')].map((node) => node.textContent)`), ["Planning share", "Calls/s"])
    await cdp.evaluate(`document.querySelector('[aria-label="Search syntax and fields"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') !== null`, "Plans quantitative help")
    const planHelp = await cdp.evaluate(`document.querySelector('[data-testid="search-help"]').textContent`)
    assert.match(planHelp, /slow_call_rate/)
    assert.doesNotMatch(planHelp, /wal_rate|cpu_cores|shared_blks_read/)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "close Plans quantitative help")
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null`, "clear quantitative Plans search")
    await cdp.evaluate(`document.querySelector('[data-testid="related-planid"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans" && new URL(location.href).searchParams.get("find") === "plan_id:77"`, "Plan ID opens the shared Plans filter")
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans" && new URL(location.href).searchParams.get("find") === null`, "Back restores the unfiltered Plans page")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "ready" && document.querySelector('[data-testid="pg-text-plan"]') !== null`, "the related query and native text execution plan")
    const planDetail = await cdp.evaluate(`(() => ({
      copy: document.querySelector('[data-testid="pg-plan-view"] button')?.textContent ?? null,
      bodyCount: document.querySelectorAll('[data-testid="pg-text-plan"]').length,
      queryBodyCount: document.querySelectorAll('[data-testid="pg-plan-query-text"]').length,
      queryCopy: [...document.querySelectorAll('[data-testid="pg-plan-query-view"] button')].map((button) => button.textContent),
      queryText: [...document.querySelectorAll('[data-testid="pg-plan-query-text"] pre')].map((node) => node.textContent),
      secondaryDisclosure: document.querySelector('[data-testid="pg-plan-view"] details') !== null,
      text: document.querySelector('[data-testid="pg-text-plan"]').textContent,
      queryBeforePlan: document.querySelector('[data-testid="pg-plan-query-view"]').getBoundingClientRect().top < document.querySelector('[data-testid="pg-plan-view"]').getBoundingClientRect().top,
    }))()`)
    assert.equal(planDetail.text, VADV_TEXT_PLAN)
    assert.equal(planDetail.copy, "Copy")
    assert.equal(planDetail.bodyCount, 1)
    assert.equal(planDetail.queryBodyCount, 1)
    assert.deepEqual(planDetail.queryCopy, ["Copy"])
    assert.deepEqual(planDetail.queryText, [INLINE_QUERY_PRIMARY])
    assert.equal(planDetail.queryBeforePlan, true)
    assert.equal(planDetail.secondaryDisclosure, false)
    await cdp.evaluate(`Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText(value) { globalThis.__copiedPlanQuery = value; return Promise.resolve() } } })`)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plan-query-view"] button').click()`)
    await cdp.waitFor(`globalThis.__copiedPlanQuery === ${JSON.stringify(INLINE_QUERY_PRIMARY)}`, "the exact recorded query copy")

    for (const locale of ["en", "ru"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `plan detail ${locale} locale`)
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: width === 360 ? 640 : 800, mobile: false, width })
        await settleLayout(cdp)
        const textLayout = await cdp.evaluate(`(() => {
          const query = document.querySelector('[data-testid="pg-plan-query-view"]')
          const list = document.querySelector('[data-testid="pg-plan-query-list"]')
          const plan = document.querySelector('[data-testid="pg-plan-view"]')
          const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top } }
          return {
            copy: [...query.querySelectorAll('button')].map((button) => button.textContent),
            labels: [query.querySelector('header strong')?.textContent, plan.querySelector('header strong')?.textContent],
            listClientHeight: list.clientHeight,
            listScrollHeight: list.scrollHeight,
            order: bounds(query).top < bounds(plan).top,
            overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
            query: bounds(query),
            plan: bounds(plan),
            whiteSpace: getComputedStyle(query.querySelector('pre')).whiteSpace,
          }
        })()`)
        assert.deepEqual(textLayout.labels, ["Query", "Execution plan"], `${locale} ${width}px labels`)
        assert.deepEqual(textLayout.copy, ["Copy"], `${locale} ${width}px copy actions`)
        assert.equal(textLayout.order, true, `${locale} ${width}px text block order`)
        assert.equal(textLayout.whiteSpace, "pre-wrap", `${locale} ${width}px whitespace`)
        assert.ok(textLayout.listClientHeight <= 320 && textLayout.listScrollHeight > textLayout.listClientHeight, `${locale} ${width}px independent query scroll: ${JSON.stringify(textLayout)}`)
        assert.ok(textLayout.query.left >= -1 && textLayout.query.right <= width + 1 && textLayout.plan.left >= -1 && textLayout.plan.right <= width + 1, `${locale} ${width}px block bounds: ${JSON.stringify(textLayout)}`)
        assert.equal(textLayout.overflow, false, `${locale} ${width}px document overflow`)
      }
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })

    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    const expectedQueryFailureAt = errors.length
    inlinePlanQueryMode = "error"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "error"`, "the related query network failure")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-text-plan"]')?.textContent`), VADV_TEXT_PLAN)
    inlinePlanQueryMode = "ready"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plan-query-error"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "ready"`, "the related query retry")
    const expectedQueryFailures = errors.splice(expectedQueryFailureAt)
    assert.equal(expectedQueryFailures.some((message) => message.startsWith("503:") && message.includes("/snapshot?") && message.includes("query_id%3A42")), true)
    assert.equal(expectedQueryFailures.every((message) => message.startsWith("503:") || message.includes("Failed to load resource: the server responded with a status of 503")), true)

    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    inlinePlanQueryMode = "empty"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "unavailable"`, "the honest related query unavailable state")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-text-plan"]')?.textContent`), VADV_TEXT_PLAN)
    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    inlinePlanQueryMode = "ready"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "ready"`, "the restored related query")
    await cdp.evaluate(`([...document.querySelectorAll('.pg-detail-head button')].find((button) => button.textContent.includes('Related statements'))).click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.statements" && new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter AND query_id:42"`, "the related plan statement route")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent.includes("select from plan_navigation") === true`, "the matched last query")
    const planContext = await cdp.evaluate(`document.querySelector('[data-testid="search-chips"]').textContent`)
    assert.match(planContext, /database: operators/)
    assert.match(planContext, /role: reporter/)
    assert.match(planContext, /query_id: 42/)
    const planQueryRequest = requests.find(({ query }) => {
      const parameters = new URLSearchParams(query)
      return parameters.get("search") === "query_id:42" && parameters.get("first_match") === "1"
    })
    assert.notEqual(planQueryRequest, undefined, JSON.stringify(requests.map(({ query }) => query), null, 2))
    const planQueryParameters = new URLSearchParams(planQueryRequest.query)
    assert.equal(planQueryParameters.has("where.queryid"), false)
    assert.equal(planQueryParameters.has("where.userid"), false)
    assert.equal(planQueryParameters.has("where.dbid"), false)
    assert.equal(planQueryParameters.has("type_id"), false)
    assert.equal(planQueryParameters.has("text"), false)
    assert.equal(planQueryParameters.has("cursor"), false)
    assert.equal(planQueryParameters.get("page_size"), "1")
    assert.equal(planQueryParameters.get("at"), String(AT))
    assert.deepEqual(planQueryParameters.getAll("field"), ["query"])
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(AT))
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans"`, "browser back to Plans")

    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Activity'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "Activity rows for related navigation")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-related-statements"]')?.disabled === false`, "Activity related Statements action")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-related-statements"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.statements" && new URL(location.href).searchParams.get("find") === "database:operators AND query_id:991"`, "Activity opens all matching Statements")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "database:operators AND query_id:991"))
    const activityQueryRequest = requests.find(({ query }) => new URLSearchParams(query).get("search") === "database:operators AND query_id:991")
    assert.notEqual(activityQueryRequest, undefined)
    assert.equal(new URLSearchParams(activityQueryRequest.query).has("where.userid"), false)
    assert.equal(new URLSearchParams(activityQueryRequest.query).has("where.toplevel"), false)
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.activity" && document.querySelector('[data-testid="pg-activity-table"]') !== null`, "Back restores Activity")

    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === null && document.querySelector('[data-testid="hour-timeline"]') !== null`, "Processes timeline")
    await assertCompactTimelineContained(cdp, ".workspace > .lensbar", "Processes")

    const hostClick = await cdp.evaluate(`(() => {
      const button = [...document.querySelectorAll(".source-tabs button")].find((candidate) => candidate.textContent === "Host")
      button.click()
      return button.textContent
    })()`)
    assert.equal(hostClick, "Host")
    await cdp.evaluate(`(() => {
      history.pushState({}, "", "/?at=${AT}&view=host.overview")
      dispatchEvent(new PopStateEvent("popstate"))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null && document.querySelector(".system-main") !== null`, "the System Overview")
    await assertCompactTimelineContained(cdp, ".system-main", "Host")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await delay(600)
    const system = await cdp.evaluate(`(() => ({
      chart: document.querySelector('[data-testid^="system-group-chart-"]') !== null,
      metric: new URL(location.href).searchParams.get("metric"),
      source: document.querySelector(".source-active")?.textContent ?? null,
    }))()`)
    assert.equal(system.source, "Host")
    // The ledger opens quiet: no auto-selected metric, no force-opened chart.
    assert.equal(system.chart, false, JSON.stringify(system))
    assert.equal(system.metric, null, JSON.stringify(system))
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the System resource table")
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "the disk ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-expansion-disk"]') !== null`, "the expanded Disk row")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-disk"]')?.getAttribute("aria-expanded")`), "true")
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU cards", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas') !== null`, "the inline CPU chart")
    await assertHoverGeometryStable(cdp, '[data-testid="system-cpu-composition"]', "wide System dock")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 420 })
    await settleLayout(cdp)
    await assertHoverGeometryStable(cdp, '[data-testid="system-cpu-composition"]', "420px System dock")
    for (const [width, height] of [[1920, 1080], [1366, 768], [1280, 431], [1024, 768], [1024, 1366]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      const layout = await cdp.evaluate(`document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => { try {
        window.scrollTo(0, 0)
        const bounds = (node) => {
          const rect = node.getBoundingClientRect()
          return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width }
        }
        const consolePanel = document.querySelector(".system-main")
        const main = document.querySelector(".system-main")
        const history = document.querySelector('[data-testid="system-group-chart-cpu"]')
        const expansion = document.querySelector('[data-testid="use-expansion-cpu"]')
        const chart = history.querySelector(".uplot-figure")
        const canvas = chart.querySelector("canvas")
        const host = chart.querySelector(".uplot-host")
        const plot = chart.querySelector(".u-over")
        const contentBottom = document.querySelector('[data-testid="use-table"]').getBoundingClientRect().bottom
        const panels = [document.querySelector(".timeline-shell"), history]
        const overlaps = []
        for (let left = 0; left < panels.length; left += 1) {
          for (let right = left + 1; right < panels.length; right += 1) {
            const a = panels[left].getBoundingClientRect()
            const b = panels[right].getBoundingClientRect()
            if (Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1
              && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1) {
              overlaps.push([panels[left].className, panels[right].className])
            }
          }
        }
        const historyBounds = bounds(history)
        const chartBounds = bounds(chart)
        resolve({
          chart: chartBounds,
          chartAccess: {
            canvasAriaHidden: canvas.getAttribute("aria-hidden"),
            canvasCount: chart.querySelectorAll(".uplot-host canvas").length,
            hostLabel: host.getAttribute("aria-label"),
            hostRole: host.getAttribute("role"),
            navigator: chart.querySelector('input.chart-navigator[type="range"]') !== null,
            summary: chart.querySelector(".chart-summary")?.textContent ?? "",
          },
          console: bounds(consolePanel),
          contentBottom,
          documentClientWidth: document.documentElement.clientWidth,
          documentScrollWidth: document.documentElement.scrollWidth,
          expansion: bounds(expansion),
          history: historyBounds,
          historyTail: historyBounds.bottom - chartBounds.bottom,
          host: bounds(host),
          main: bounds(main),
          overlaps,
          plot: bounds(plot),
        })
      } catch (error) { resolve({ error: String(error && error.stack || error) }) } }))))`)
      assert.equal(layout.error, undefined, layout.error)
      assert.ok(layout.chart.height >= 300 && layout.chart.height <= 360, `${width}x${height} System chart height: ${JSON.stringify(layout)}`)
      assert.ok(layout.host.height >= 190, `${width}x${height} System chart host height: ${JSON.stringify(layout)}`)
      assert.ok(layout.plot.height >= 145, `${width}x${height} System plot height: ${JSON.stringify(layout)}`)
      assert.ok(layout.history.left >= layout.expansion.left - 1 && layout.history.right <= layout.expansion.right + 1
        && layout.history.top >= layout.expansion.top - 1 && layout.history.bottom <= layout.expansion.bottom + 1,
      `${width}x${height} System expansion containment: ${JSON.stringify(layout)}`)
      assert.deepEqual(layout.chartAccess.canvasAriaHidden, "true")
      assert.equal(layout.chartAccess.canvasCount, 1)
      assert.equal(layout.chartAccess.hostRole, "img")
      assert.match(layout.chartAccess.hostLabel, /CPU used.*cores/)
      assert.equal(layout.chartAccess.navigator, true)
      assert.ok(layout.chartAccess.summary.length > 0)
      assert.ok(layout.history.height - layout.chart.height <= 240 && layout.historyTail >= -1 && layout.historyTail <= 30, `${width}x${height} compact System dock: ${JSON.stringify(layout)}`)
      assert.ok(Math.abs(layout.console.bottom - layout.contentBottom) <= 1.5, `${width}x${height} content-sized System layout: ${JSON.stringify(layout)}`)
      assert.ok(layout.chart.left >= layout.history.left - 1 && layout.chart.right <= layout.history.right + 1
        && layout.chart.top >= layout.history.top - 1 && layout.chart.bottom <= layout.history.bottom + 1,
      `${width}x${height} System chart containment: ${JSON.stringify(layout)}`)
      assert.ok(layout.documentScrollWidth <= layout.documentClientWidth, `${width}x${height} System document overflow: ${JSON.stringify(layout)}`)
      assert.deepEqual(layout.overlaps, [], `${width}x${height} System panel overlaps`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 1080, mobile: false, width: 1920 })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the overview resource table")
    const overviewOrder = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="use-table"]').getBoundingClientRect()
      const inspector = document.querySelector('[data-testid="inspector"]')?.getBoundingClientRect() ?? null
      return { gap: inspector === null ? null : inspector.left - table.right }
    })()`)
    assert.equal(overviewOrder.gap, null, `overview navigation closes the prior Inspector: ${JSON.stringify(overviewOrder)}`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "CPU metrics after Overview")
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas') !== null`, "the inline CPU chart after Overview")
    await cdp.evaluate(`(() => {
      if (window.__kronikaAxisText !== undefined) return
      window.__kronikaAxisText = []
      const original = CanvasRenderingContext2D.prototype.fillText
      CanvasRenderingContext2D.prototype.fillText = function (value, ...args) {
        window.__kronikaAxisText.push(String(value))
        return original.call(this, value, ...args)
      }
    })()`)
    const chartThemes = []
    for (let themeIndex = 0; themeIndex < 2; themeIndex += 1) {
      const theme = await cdp.evaluate(`document.documentElement.dataset.theme`)
      chartThemes.push(theme)
      for (const [width, height] of [[1920, 1080], [1366, 768], [1280, 431], [1024, 768], [390, 844]]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
        const state = await cdp.evaluate(`document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => { try {
          const figure = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-figure')
          const host = figure.querySelector(".uplot-host")
          const canvas = host.querySelector("canvas")
          const plot = host.querySelector(".u-over")
          const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width } }
          resolve({
            backingRatio: canvas.width / canvas.getBoundingClientRect().width,
            canvas: bounds(canvas),
            canvasAriaHidden: canvas.getAttribute("aria-hidden"),
            clientWidth: document.documentElement.clientWidth,
            figure: bounds(figure),
            host: bounds(host),
            hostLabel: host.getAttribute("aria-label"),
            hostRole: host.getAttribute("role"),
            navigatorCount: figure.querySelectorAll('input.chart-navigator[type="range"]').length,
            plot: bounds(plot),
            scrollWidth: document.documentElement.scrollWidth,
            summary: figure.querySelector(".chart-summary")?.textContent ?? "",
          })
        } catch (error) { resolve({ error: String(error && error.stack || error) }) } }))))`)
        assert.equal(state.error, undefined, state.error)
        assert.equal(state.canvasAriaHidden, "true", `${theme} ${width}px canvas accessibility: ${JSON.stringify(state)}`)
        assert.equal(state.hostRole, "img", `${theme} ${width}px chart role: ${JSON.stringify(state)}`)
        assert.match(state.hostLabel, /CPU used.*cores/, `${theme} ${width}px chart unit: ${JSON.stringify(state)}`)
        assert.equal(state.navigatorCount, 1, `${theme} ${width}px sample navigator: ${JSON.stringify(state)}`)
        assert.ok(state.summary.length > 0, `${theme} ${width}px chart summary: ${JSON.stringify(state)}`)
        assert.ok(state.plot.width >= 120, `${theme} ${width}px plot width: ${JSON.stringify(state)}`)
        assert.ok(state.host.height >= 150, `${theme} ${width}px chart host height: ${JSON.stringify(state)}`)
        assert.ok(state.plot.height >= 80, `${theme} ${width}px plot height: ${JSON.stringify(state)}`)
        assert.ok(state.canvas.left >= state.host.left - 1 && state.canvas.right <= state.host.right + 1,
          `${theme} ${width}px canvas bounds: ${JSON.stringify(state)}`)
        assert.ok(state.figure.left >= -1 && state.figure.right <= state.clientWidth + 1,
          `${theme} ${width}px chart viewport: ${JSON.stringify(state)}`)
        assert.ok(state.scrollWidth <= state.clientWidth, `${theme} ${width}px page overflow: ${JSON.stringify(state)}`)
        assert.ok(state.backingRatio >= 0.95 && state.backingRatio <= 1.05,
          `${theme} ${width}px DPR 1 backing store: ${JSON.stringify(state)}`)
      }
      if (themeIndex === 0) {
        await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
        await cdp.waitFor(`document.documentElement.dataset.theme !== ${JSON.stringify(theme)}`, "the second chart theme")
      }
    }
    assert.deepEqual(new Set(chartThemes), new Set(["dark", "light"]))
    const axisText = await cdp.evaluate(`window.__kronikaAxisText`)
    assert.equal(axisText.some((text) => text.includes("Time, browser local")), false, JSON.stringify(axisText))
    const timeAxes = axisText.filter((text) => /^\d{2}:\d{2}$/.test(text))
    assert.equal(timeAxes.length > 0, true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /GMT|UTC|\.\d{3}(?!\d)/.test(text)), false, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => text.includes("%")), true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /^0%?$/.test(text)), true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /^100%?$/.test(text)), true, JSON.stringify(axisText))

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 2, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`(() => {
      const canvas = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas')
      return canvas !== null && canvas.width / canvas.getBoundingClientRect().width >= 1.9
    })()`, "the DPR 2 chart backing store")
    const dprTwo = await cdp.evaluate(`(() => {
      const canvas = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas')
      return { ratio: canvas.width / canvas.getBoundingClientRect().width, screen: devicePixelRatio }
    })()`)
    assert.ok(dprTwo.ratio >= 1.9 && dprTwo.ratio <= 2.1, JSON.stringify(dprTwo))
    assert.equal(dprTwo.screen, 2)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`(() => {
      const canvas = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas')
      const ratio = canvas?.width / canvas?.getBoundingClientRect().width
      return ratio >= 0.95 && ratio <= 1.05
    })()`, "the restored DPR 1 chart backing store")

    const chartRequestsBeforeInspectorClose = requests.filter(({ path }) => path.startsWith("/api/")).length
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-group-chart-cpu"] .chart-expand, [data-testid="system-group-chart-cpu"] [role="dialog"].uplot-expanded') === null`), true)
    await delay(120)
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/")).length, chartRequestsBeforeInspectorClose)

    await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-cpu"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-expansion-cpu"]') === null`, "the collapsed CPU row")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') === null`), true)

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp`), String(AT))
    const sampleText = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(sampleText, /^01:30:00;/)
    assert.doesNotMatch(sampleText, /GMT|UTC|\.000/)
    assert.match(sampleText, /82/)

    const arrow = async (key, expected) => {
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: ${JSON.stringify(key)} }))`)
      await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${expected}"`, `${key} to ${expected}`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${expected}"`, `${key} exact sample ${expected}`)
    }
    await arrow("ArrowLeft", BEFORE_AT)
    await arrow("ArrowRight", AT)
    await arrow("ArrowRight", AFTER_AT)
    const point = async (target, expected) => {
      await cdp.evaluate(`(() => {
        const plot = document.querySelector('[data-testid="hour-timeline"] .u-over')
        const bounds = plot.getBoundingClientRect()
        const range = ${HOUR_US} * (1 + 38 / Math.max(1, bounds.width - 38))
        const clientX = bounds.left + (${target} - ${HOUR}) / range * bounds.width
        plot.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, clientX, isPrimary: true, pointerId: 7, pointerType: "mouse" }))
      })()`)
      await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${expected}"`, `pointer snap to ${expected}`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${expected}"`, `pointer exact sample ${expected}`)
    }
    await point(QUARTER + 3_000_000, QUARTER_NEXT)
    await point(QUARTER, QUARTER_PREVIOUS)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="hour-timeline"] .uplot').length`), 1)

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await switchZone(cdp, "utc")
    await cdp.waitFor(`document.documentElement.lang === "ru" && ${ZONE_VALUE} === "utc" && ${ZONE_LABEL} === "UTC" && document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")?.startsWith("05:14:55;")`, "the chart UTC render")
    const utcSample = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(utcSample, /^05:14:55;/)
    assert.doesNotMatch(utcSample, /GMT|UTC|\.\d{3}(?!\d)/)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await switchZone(cdp, "browser")
    await cdp.waitFor(`document.documentElement.lang === "en" && ${ZONE_VALUE} === "browser" && ${ZONE_LABEL} === "Browser time" && document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")?.startsWith("01:14:55;")`, "the chart local-time restore")

    const navigateTo = async (timestamp) => {
      const target = await cdp.evaluate(`(() => {
        const url = new URL(location.href)
        url.searchParams.set("at", "${timestamp}")
        return url.href
      })()`)
      await cdp.send("Page.navigate", { url: target })
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator') !== null`, `timeline at ${timestamp}`, 15_000)
    }
    await navigateTo(DST_EDT_HOUR + 1_800_000_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${DST_EDT_HOUR + 1_800_000_000}"`, "the repeated first exact sample")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(DST_EDT_HOUR + 1_800_000_000))
    const dstEdt = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(dstEdt, /^01:30:00;/)
    assert.doesNotMatch(dstEdt, /GMT|UTC|\.000/)
    await navigateTo(DST_EST_HOUR + 1_800_000_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${DST_EST_HOUR + 1_800_000_000}"`, "the repeated second exact sample")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(DST_EST_HOUR + 1_800_000_000))
    const dstEst = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(dstEst, /^01:30:00;/)
    assert.doesNotMatch(dstEst, /GMT|UTC|\.000/)
    assert.equal(dstEdt.split(";")[0], dstEst.split(";")[0])
    assert.deepEqual(errors, [])
    assert.deepEqual(external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the minified artifact restores and clears its opaque browser session", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const responses = []
  const errors = []
  const external = []
  const authState = { valid: false }
  let rejectNextApi = false
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") {
      answerSession(request, response, authState)
      return
    }
    if (url.pathname.startsWith("/api/")) {
      if (rejectNextApi) {
        rejectNextApi = false
        unauthorized(response)
        return
      }
      if (!browserIsAuthenticated(request, authState)) {
        unauthorized(response)
        return
      }
    }
    if (url.pathname === "/api/hour") {
      ndjson(response, timelineRecords(Number(url.searchParams.get("from") ?? HOUR)))
      return
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      ndjson(response, url.searchParams.getAll("section").includes("pg_stat_activity") ? snapshotRecords() : [])
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") {
      ndjson(response, [])
      return
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("session test server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const pageUrl = `${origin}/?at=${AT}&view=pg.activity`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  let browser = launchBrowser(profile)
  let socket
  try {
    let debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    let cdp = cdpSession(socket)
    trackPage(socket, origin, { errors, external, responses })
    await enablePage(cdp)

    let started = requests.length
    await cdp.send("Page.navigate", { url: pageUrl })
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "initial login")
    assertBootstrapBeforeApi(requests.slice(started), false)
    await submitLogin(cdp)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "signed-in application")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "initial API data")

    const storage = await cdp.evaluate(`(async () => ({
      cookie: document.cookie,
      indexedDatabases: (await indexedDB.databases()).map(({ name, version }) => ({ name, version })),
      local: Object.entries(localStorage),
      session: Object.entries(sessionStorage),
    }))()`)
    assert.equal(storage.cookie, "")
    assert.deepEqual(storage.indexedDatabases, [])
    assert.deepEqual(storage.session, [])
    const stored = JSON.stringify(storage)
    for (const secret of ["artifact", "wire", "Basic", SESSION_COOKIE, "YXJ0aWZhY3Q6d2lyZQ=="]) {
      assert.equal(stored.includes(secret), false, `browser storage contains ${secret}`)
    }

    started = requests.length
    await cdp.send("Page.reload")
    await waitForRequests(() => requests.slice(started).some(({ method, path }) => method === "GET" && path === "/auth/session"))
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "reload restoration")
    assertBootstrapBeforeApi(requests.slice(started), true)

    started = requests.length
    const created = await cdp.send("Target.createTarget", { url: "about:blank" })
    const tabSocket = await pageSocket(debugPort, created.targetId)
    const tab = cdpSession(tabSocket)
    trackPage(tabSocket, origin, { errors, external, responses })
    await enablePage(tab)
    await tab.send("Page.navigate", { url: pageUrl })
    await tab.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "new-tab restoration")
    assertBootstrapBeforeApi(requests.slice(started), true)
    await cdp.send("Target.closeTarget", { targetId: created.targetId })
    tabSocket.close()

    await cdp.send("Browser.close")
    await Promise.race([
      new Promise((resolve) => browser.once("exit", resolve)),
      delay(2_000),
    ])
    await stopBrowser(browser)
    socket.close()
    socket = undefined
    await delay(200)
    await rm(join(profile, "DevToolsActivePort"), { force: true })
    browser = launchBrowser(profile)
    debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    cdp = cdpSession(socket)
    trackPage(socket, origin, { errors, external, responses })
    await enablePage(cdp)
    started = requests.length
    await cdp.send("Page.navigate", { url: pageUrl })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "browser-restart restoration")
    assertBootstrapBeforeApi(requests.slice(started), true)

    const postsBeforeLogout = requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length
    started = requests.length
    await cdp.evaluate(`document.querySelector('.top-actions button[title="Sign out"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "logout")
    assert.equal(requests.slice(started).filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length, 1)
    started = requests.length
    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "signed-out reload")
    await waitForRequests(() => requests.slice(started).some(({ method, path }) => method === "GET" && path === "/auth/session"))
    const signedOutReload = requests.slice(started)
    assert.equal(signedOutReload.some(({ path }) => path.startsWith("/api/")), false)
    assert.equal(requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length, postsBeforeLogout)

    await submitLogin(cdp)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "relogin")
    const postsBeforeExpiry = requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length
    const deletesBeforeExpiry = requests.filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length
    const responsesBeforeExpiry = responses.length
    rejectNextApi = true
    started = requests.length
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="login-message"]')?.textContent.includes("session ended") === true`, "one expired-session transition")
    await delay(300)
    const expiryRequests = requests.slice(started)
    assert.equal(expiryRequests.filter(({ path }) => path.startsWith("/api/")).length, 1)
    assert.equal(expiryRequests.filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length, 1)
    assert.equal(requests.filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length, deletesBeforeExpiry + 1)
    assert.equal(requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length, postsBeforeExpiry)
    const forcedResponses = responses.slice(responsesBeforeExpiry).filter(({ status }) => status === 401)
    assert.equal(forcedResponses.length, 1)
    assert.equal(forcedResponses[0]?.path.startsWith("/api/"), true)
    assert.equal(forcedResponses[0]?.challenge, null)

    const authorizedRequests = requests.filter(({ path }) => path.startsWith("/api/"))
    assert.ok(authorizedRequests.every(({ authorization, cookie, marker }) => (
      authorization === null && cookie === SESSION_COOKIE && marker === "1"
    )))
    const basicRequests = requests.filter(({ authorization }) => authorization !== null)
    assert.ok(basicRequests.every(({ method, path }) => method === "POST" && path === "/auth/session"))
    assert.deepEqual(errors, [])
    assert.deepEqual(external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the slow-query detail keeps readable labels and human event time", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: false }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") {
      answerSession(request, response, authState)
      return
    }
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) {
      unauthorized(response)
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") {
      ndjson(response, [])
      return
    }
    if (url.pathname === "/api/hour") {
      ndjson(response, slowQueryTimelineRecords())
      return
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot` && url.searchParams.get("row_ordinal") === "3") {
      ndjson(response, slowQueryRecords())
      return
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("artifact test server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    const page = { errors: [], external: [], responses: [] }
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 882, mobile: false, width: 1280 })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=events` })
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "login form")
    await submitLogin(cdp)
    await cdp.waitFor(`document.querySelector('[data-testid="event-item"] button') !== null`, "the slow-query event")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click(); document.querySelector('[data-testid="event-item"] button').click()`)
    await cdp.waitFor(
      `[...document.querySelectorAll('[data-testid="event-detail"] dt')].some((label) => label.textContent.trim().toLocaleUpperCase("ru-RU") === "SAMPLE")`,
      "the resolved slow-query detail",
    )
    await settleLayout(cdp)

    const landscape = await cdp.evaluate(detailGeometryExpression())
    assert.equal(landscape.innerWidth, 1280)
    assert.ok(landscape.scrollWidth <= landscape.clientWidth, JSON.stringify(landscape))
    assert.ok(landscape.sample.label.width >= 120, JSON.stringify(landscape.sample))
    assert.equal(landscape.sample.label.lines, 1)
    assert.ok(landscape.sample.label.right + 7 <= landscape.sample.value.left, JSON.stringify(landscape.sample))
    assert.ok(Math.abs(landscape.sample.row.width - landscape.list.width) <= 1, JSON.stringify(landscape.sample))
    assert.ok(Math.abs(landscape.sample.value.right - landscape.sample.row.right) <= 1, JSON.stringify(landscape.sample))
    assert.ok(landscape.sample.value.clientWidth > landscape.sample.label.width, JSON.stringify(landscape.sample))
    assert.ok(landscape.sample.value.scrollWidth <= landscape.sample.value.clientWidth + 1, JSON.stringify(landscape.sample))
    assert.ok(landscape.sample.value.height > landscape.sample.value.lineHeight * 1.5, JSON.stringify(landscape.sample))
    assert.equal(landscape.sample.value.minWidth, "0px")
    assert.equal(landscape.pattern.label.lines, 1)
    assert.ok(landscape.pattern.value.scrollWidth <= landscape.pattern.value.clientWidth + 1, JSON.stringify(landscape.pattern))
    assert.equal(landscape.numeric.every(({ align }) => align === "right"), true)
    assert.ok(landscape.numeric.every(({ height }) => height <= 24), JSON.stringify(landscape.numeric))
    assert.ok(Math.max(...landscape.numeric.map(({ right }) => right)) - Math.min(...landscape.numeric.map(({ right }) => right)) <= 1)
    assert.equal(landscape.numeric[0]?.text, "3")
    assert.equal(landscape.numeric[1]?.text, "6,29 с")
    assert.equal(landscape.numeric[2]?.text, "12,6 с")
    assert.equal(landscape.chart.current, "")
    assert.doesNotMatch(landscape.labels.join("\n"), /,\s*(?:ms|мс)$/imu)
    assert.doesNotMatch(landscape.text, /тыс\.\s*мс/iu)
    assert.equal(landscape.chart.label, "")

    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot .u-over') !== null`, "the selected event Chart")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') === null`), true)
    const eventPreviewHeight = await cdp.evaluate(`document.querySelector('.timeline-preview')?.getBoundingClientRect().height ?? null`)
    if (eventPreviewHeight !== null) assert.ok(Math.abs(eventPreviewHeight - 124) <= .5)
    const eventChart = await cdp.evaluate(`(() => ({
      current: document.querySelector('.inspector-chart-slot .chart-current')?.textContent.trim() ?? '',
      label: document.querySelector('.inspector-chart-slot .uplot-host')?.getAttribute('aria-label') ?? '',
    }))()`)
    assert.equal(eventChart.current, "6,29 с")
    assert.doesNotMatch(eventChart.label, /(?:^|[, (])(?:ms|мс)(?:$|[,)])/iu)

    const hoverPoint = await cdp.evaluate(`(() => {
      const bounds = document.querySelector('.inspector-chart-slot .u-over').getBoundingClientRect()
      return { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 }
    })()`)
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...hoverPoint })
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot [data-testid="chart-hover-readout"]') !== null`, "the slow-query human duration hover")
    const hover = await cdp.evaluate(`document.querySelector('.inspector-chart-slot [data-testid="chart-hover-readout"]').textContent`)
    assert.match(hover, /6,29\sс/u)
    assert.doesNotMatch(hover, /тыс\.\s*мс|\(мс\)/iu)

    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[0].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector"][data-panel="detail"]') !== null`, "the selected event Detail")

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 882, mobile: false, width: 480 })
    await settleLayout(cdp)
    const narrow = await cdp.evaluate(detailGeometryExpression())
    assert.equal(narrow.innerWidth, 480)
    assert.ok(narrow.scrollWidth <= narrow.clientWidth, JSON.stringify(narrow))
    assert.equal(narrow.sample.label.lines, 1)
    assert.ok(narrow.sample.label.bottom <= narrow.sample.value.top + 0.5, JSON.stringify(narrow.sample))
    assert.ok(Math.abs(narrow.sample.label.left - narrow.sample.value.left) <= 1, JSON.stringify(narrow.sample))
    assert.ok(Math.abs(narrow.sample.label.width - narrow.sample.row.width) <= 1, JSON.stringify(narrow.sample))
    assert.ok(narrow.sample.value.scrollWidth <= narrow.sample.value.clientWidth + 1, JSON.stringify(narrow.sample))
    assert.ok(narrow.pattern.value.scrollWidth <= narrow.pattern.value.clientWidth + 1, JSON.stringify(narrow.pattern))
    assert.equal(narrow.numeric.every(({ align }) => align === "left"), true, JSON.stringify(narrow.numeric))
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("tablespace rollups keep exact history, URL drill, Back, search, and narrow geometry", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, url.searchParams.has("group") ? aggregateRelationHistoryRecords(url) : timelineRecords(HOUR))
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, relationRecords(url, "single"))
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("relation chart server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    const page = { errors: [], external: [], responses: [] }
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.indexes&level=tablespace&pg_lens=state&find=tablespace%3Afast_ssd` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length === 1`, "the tablespace index aggregate")
    const summary = await cdp.evaluate(`(() => ({
      cells: [...document.querySelector('[data-testid="pg-indexes-table"] .entity-row').querySelectorAll('[role="cell"]')].map((cell) => cell.textContent),
      levels: [...document.querySelectorAll('nav.lensbar button')].map((button) => button.textContent),
    }))()`)
    assert.equal(summary.cells[0], "fast_ssd")
    assert.match(summary.cells[1], /363/)
    assert.deepEqual(summary.levels.slice(0, 4), ["Indexes", "Schemas", "Databases", "Tablespaces"])
    const snapshots = requests.filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && new URLSearchParams(query).get("group") === "tablespace")
    assert.ok(snapshots.some(({ query }) => new URLSearchParams(query).get("search") === "tablespace:fast_ssd"), JSON.stringify(snapshots))

    const comparisonStart = requests.length
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "(tablespace:fast_ssd or tablespace:archive) and (size > 100.000MB or scan_rate>10/s)")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "(tablespace:fast_ssd or tablespace:archive) and (size > 100.000MB or scan_rate>10/s)", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    const groupedBoolean = "(tablespace:fast_ssd OR tablespace:archive) AND (size>100MB OR scan_rate>10/s)"
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === ${JSON.stringify(groupedBoolean)} && document.querySelector('[data-testid="search-chips"]')?.textContent.includes("Size · > 100 MB") === true`, "the hidden-lens grouped boolean chips")
    await waitForRequests(() => requests.slice(comparisonStart).some(({ query }) => new URLSearchParams(query).get("search") === groupedBoolean))
    const comparisonRequest = requests.slice(comparisonStart).find(({ query }) => new URLSearchParams(query).get("search") === groupedBoolean)
    assert.notEqual(comparisonRequest, undefined)
    const comparisonQuery = new URLSearchParams(comparisonRequest.query)
    assert.equal(comparisonQuery.get("group"), "tablespace")
    assert.equal(comparisonQuery.getAll("field").includes("main_fork_bytes"), false)
    const comparisonChip = await cdp.evaluate(`(() => {
      const chip = document.querySelector('[data-testid="search-chips"] [title="size>100MB"]').parentElement
      return {
        label: chip.querySelector("button").getAttribute("aria-label"),
        text: chip.textContent,
        title: chip.querySelector("[title]")?.getAttribute("title"),
      }
    })()`)
    assert.equal(comparisonChip.title, "size>100MB")
    assert.match(comparisonChip.label, /Remove Size: > 100 MB/)
    assert.match(comparisonChip.text, /Size · > 100 MB/)

    const mixedStart = requests.length
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "tablespace:fast_ssd OR size>100MB")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "tablespace:fast_ssd OR size>100MB", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-error"] mark')?.textContent === "OR"`, "the grouped mixed OR span")
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="search-error"]')?.textContent ?? ""`), /cannot mix names or text with metrics/)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), groupedBoolean)
    assert.equal(requests.slice(mixedStart).some(({ query }) => new URLSearchParams(query).get("search") === "tablespace:fast_ssd OR size>100MB"), false)

    const invalidStart = requests.length
    const retainedRow = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "size>=100MB")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "size>=100MB", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-error"] mark')?.textContent === ">="`, "the atomic unsupported operator span")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), groupedBoolean)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`), retainedRow)
    assert.equal(requests.slice(invalidStart).some(({ query }) => query.includes("size%3E%3D") || query.includes("size%3E=")), false)

    await assertSearchControlContained(cdp, "Indexes comparison search", '[data-search-surface="pg_stat_user_indexes"]')
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 640, mobile: false, width: 360 })
    await settleLayout(cdp)
    const narrowComparison = await cdp.evaluate(`(() => ({
      chip: document.querySelector('[data-testid="search-chips"]')?.textContent ?? "",
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    }))()`)
    assert.match(narrowComparison.chip, /Размер · > 100 MB/)
    assert.match(narrowComparison.chip, /OR/)
    assert.equal(narrowComparison.overflow, false)
    await cdp.evaluate(`document.querySelector('[aria-label="Синтаксис и поля поиска"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') !== null`, "the RU grouped-search help")
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="search-help"]')?.textContent ?? ""`), /оператор OR не может объединять текстовые условия с условиями по метрикам/)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "the RU grouped-search help closed")
    for (const width of [800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width })
      await settleLayout(cdp)
      await assertSearchControlContained(cdp, `RU grouped search at ${width}`, '[data-search-surface="pg_stat_user_indexes"]')
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: width === 360 ? 640 : 768, mobile: false, width })
      await settleLayout(cdp)
      await assertSearchControlContained(cdp, `EN grouped search at ${width}`, '[data-search-surface="pg_stat_user_indexes"]')
    }
    await assertSearchChipHierarchyMatrix(cdp, "grouped search chip hierarchy")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null`, "comparison clear")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') !== null`, "the aggregate detail")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-detail"] .uplot-host') === null`), true)
    const detailText = await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-detail"]').textContent`)
    assert.match(detailText, /fast_ssd/)
    assert.match(detailText, /1663/)
    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot .pg-metric-history .uplot-host canvas') !== null`, "the aggregate history Chart")
    await settleLayout(cdp)

    const historyRequests = requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).has("group"))
    assert.equal(historyRequests.length, 1, JSON.stringify(historyRequests))
    const query = new URLSearchParams(historyRequests[0].query)
    assert.equal(query.get("group"), "tablespace")
    assert.equal(query.get("where.tablespace_oid"), "1663")
    assert.equal(query.get("where.datid"), null)
    assert.equal(query.get("where.schemaname"), null)
    assert.equal(query.get("type_id"), null)
    assert.deepEqual(query.getAll("field"), ["index_count", "invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"])
    const layout = await cdp.evaluate(`(() => {
      const slot = document.querySelector('.inspector-chart-slot')
      const chart = slot.querySelector('.uplot-host')
      const plot = slot.querySelector('.u-over')
      const table = document.querySelector('[data-testid="pg-indexes-table"]')
      const selectors = [...slot.querySelectorAll('.history-selector button')]
      return {
        chartWidth: chart.getBoundingClientRect().width,
        slotWidth: slot.getBoundingClientRect().width,
        plotWidth: plot.getBoundingClientRect().width,
        tableWidth: table.getBoundingClientRect().width,
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        selectors: selectors.map((button) => button.textContent),
      }
    })()`)
    assert.ok(layout.chartWidth > 250 && layout.chartWidth <= layout.slotWidth, JSON.stringify(layout))
    assert.ok(layout.plotWidth > 176, JSON.stringify(layout))
    assert.ok(layout.tableWidth >= 500, JSON.stringify(layout))
    assert.equal(layout.overflow, false)
    assert.equal(layout.selectors.length, 6)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') === null`), true)
    assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('.timeline-preview').getBoundingClientRect().height`) - 124) <= .5)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-detail"] .chart-expand, [role="dialog"].uplot-expanded') === null`), true)
    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[0].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector"][data-panel="detail"]') !== null`, "the aggregate Detail restored")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-drill"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('level') === null && new URL(location.href).searchParams.get('tablespace_oid') === '1663'`, "the exact tablespace member URL")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length === 1`, "the tablespace members")
    const memberRequests = requests.filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && new URLSearchParams(query).get("where.tablespace_oid") === "1663")
    assert.ok(memberRequests.length > 0, JSON.stringify(requests))
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('level') === 'tablespace' && document.querySelector('[data-testid="pg-relation-detail"]') !== null`, "the selected tablespace restored by Back")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 844, mobile: false, width: 390 })
    await settleLayout(cdp)
    const narrow = await cdp.evaluate(`(() => ({
      clientWidth: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
      buttons: [...document.querySelectorAll('nav.lensbar .lens-tabs button')].map((button) => ({ text: button.textContent, width: button.getBoundingClientRect().width })),
    }))()`)
    assert.ok(narrow.scrollWidth <= narrow.clientWidth, JSON.stringify(narrow))
    assert.deepEqual(narrow.buttons.map(({ text }) => text), ["Indexes", "Schemas", "Databases", "Tablespaces"])
    assert.equal(narrow.buttons.every(({ width }) => width > 55), true, JSON.stringify(narrow))
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test.skip("legacy chart visibility preference is replaced by the permanent preview", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  let summaryMode = "initial"
  const heldSummaries = []
  const heldCgroups = []
  const servedActivityHistory = []
  let holdCgroups = false
  const historyRequests = (section) => requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === section)
  const snapshotRequests = (section) => requests.filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && new URLSearchParams(query).getAll("section").includes(section))
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const hour = Number(url.searchParams.get("from") ?? HOUR)
      const section = url.searchParams.get("section")
      if (section === "os_process_summary") {
        if (summaryMode === "hold") {
          heldSummaries.push(response)
          return
        }
        if (summaryMode === "fail") {
          response.writeHead(200, { "Content-Type": "application/x-ndjson; charset=utf-8" })
          response.end("{")
          return
        }
        if (summaryMode === "empty") return ndjson(response, [])
        return ndjson(response, processSummaryRecords(hour, summaryMode === "initial" ? 719 : 2, summaryMode === "initial" ? 719 : 721))
      }
      if (section === "pg_stat_activity") {
        const records = activityHistoryRecords(url)
        servedActivityHistory.push(records)
        return ndjson(response, records)
      }
      return ndjson(response, section === null ? timelineRecords(hour, true) : [])
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      if (sections.length === 1 && ["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].includes(sections[0])) {
        if (holdCgroups) {
          heldCgroups.push({ response, url })
          return
        }
        return ndjson(response, cgroupSnapshotRecords(url))
      }
      if (sections.includes("pg_stat_user_tables") || sections.includes("pg_stat_user_indexes")) return ndjson(response, relationRecords(url, "single"))
      if (sections.includes("os_cpu")) return ndjson(response, systemSnapshotRecords(true, Number(url.searchParams.get("at") ?? AT)))
      if (sections.includes("pg_stat_activity")) return ndjson(response, snapshotRecords())
      return ndjson(response, [])
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("chart browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=processes&lens=generic` })
    await cdp.waitFor(`document.querySelector('.process-summary > div:first-child strong')?.textContent === "719"`, "719 process-summary rows", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .uplot-host canvas') !== null`, "the Process timeline")
    await settleLayout(cdp)
    const shownProcessHeight = await cdp.evaluate(`document.querySelector('.process-table .entity-scroll').getBoundingClientRect().height`)
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty') === null`, "all Process charts hidden")
    await settleLayout(cdp)
    const hiddenProcessHeight = await cdp.evaluate(`document.querySelector('.process-table .entity-scroll').getBoundingClientRect().height`)
    assert.ok(hiddenProcessHeight > shownProcessHeight, JSON.stringify({ hiddenProcessHeight, shownProcessHeight }))
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').getAttribute("aria-label")`), "Show charts")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').getAttribute("aria-pressed")`), "false")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .uplot-host canvas') !== null`, "the restored Process timeline")

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, "the full-height process table")
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "the captured-user process row")
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'cpu_cores>0.1 AND rss>2MiB')
      input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste' }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "cpu_cores>0.1 AND rss>2MiB"`, "natural quantitative Processes search")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "cpu_cores>0.1 AND rss>2MiB"))
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="search-chips"] strong')].map((node) => node.textContent)`), ["CPU cores", "RSS"])
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null`, "clear quantitative Processes search")
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "captured-user row after quantitative search")
    const processJoinGap = await cdp.evaluate(`(() => {
      const timeline = document.querySelector('.timeline-shell').getBoundingClientRect()
      const controls = document.querySelector('.process-workspace > .lensbar').getBoundingClientRect()
      return controls.top - timeline.bottom
    })()`)
    assert.ok(processJoinGap <= 1, `Process major-region gap ${processJoinGap}px`)
    const capturedUsers = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      const headers = [...table.querySelectorAll('[role="columnheader"]')].map((cell) => cell.querySelector('.entity-sort span')?.textContent.trim() ?? '')
      const selected = [...table.querySelectorAll('.entity-row')].find((row) => row.textContent.includes('2686712'))
      return Object.fromEntries([...selected.querySelectorAll('[role="cell"]')].map((cell, index) => [headers[index], cell.textContent.trim()]))
    })()`)
    assert.equal(capturedUsers.User, "postgres (26)")
    assert.equal(capturedUsers["Effective user"], "postgres-worker (27)")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="process-table"] .entity-row')].find((row) => row.textContent.includes("2686712"))).querySelector('[data-testid="process-user-filter-user"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "user:postgres"`, "resolved real user opens canonical name search")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="process-dock"]') === null`), true)
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await delay(400)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null && [...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "process rows restored after real-user search")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="process-table"] .entity-row')].find((row) => row.textContent.includes("2686712"))).querySelector('[data-testid="process-user-filter-effective_user"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "effective_user:postgres-worker"`, "resolved effective user opens canonical name search")
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=processes&lens=generic` })
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "ordinary process rows restored after user search")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="process-table"] .entity-row')].find((row) => row.textContent.includes("2686712"))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-linked-dock"] [data-testid="pg-exact-query"]')?.textContent.includes("select activity_for_2686712") === true`, "the PID-first linked Activity query")
    await cdp.evaluate(`document.querySelector('[data-testid="lens-cpu"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-history-metric-majflt"]') !== null`, "Major page faults history")
    const cpuDetail = await cdp.evaluate(`(() => ({
      labels: [...document.querySelectorAll('[data-testid="pg-linked-dock"] > dl:first-of-type dt')].map((node) => node.textContent),
      schedulerChips: ["nice", "prio", "rtprio"].filter((field) => document.querySelector('[data-testid="process-history-metric-' + field + '"]') !== null),
      snapshot: document.querySelector('[data-testid="pg-linked-dock"]')?.textContent ?? "",
    }))()`)
    assert.equal(cpuDetail.schedulerChips.length, 0)
    for (const label of ["Nice", "Priority", "RT priority"]) assert.ok(cpuDetail.labels.some((value) => value.startsWith(label)), JSON.stringify(cpuDetail.labels))
    assert.equal(cpuDetail.labels.filter((value) => value === "User?").length, 1, JSON.stringify(cpuDetail.labels))
    assert.equal(cpuDetail.labels.filter((value) => value === "Effective user?").length, 1, JSON.stringify(cpuDetail.labels))
    assert.match(cpuDetail.snapshot, /postgres \(26\)/)
    assert.match(cpuDetail.snapshot, /postgres-worker \(27\)/)
    assert.doesNotMatch(cpuDetail.snapshot, /\b\d{2}[./]\d{2}[./]2026\b/)
    for (const lens of ["cpu", "memory", "disk", "generic"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="lens-${lens}"]').click()`)
      await settleLayout(cdp)
      await cdp.waitFor(`document.querySelector('[data-testid="table-paging"]') === null`, `${lens} process page settled`)
      const geometry = await cdp.evaluate(`(() => {
        const box = (node) => { const value = node.getBoundingClientRect(); return { bottom: value.bottom, top: value.top } }
        const main = document.querySelector('.process-main')
        const table = document.querySelector('[data-testid="process-table"]')
        const scroll = table.querySelector('.entity-scroll')
        const dock = document.querySelector('[data-testid="pg-linked-dock"]')
        return { dock: box(dock), dockClient: dock.clientHeight, dockScroll: dock.scrollHeight, main: box(main), scrollClient: scroll.clientHeight, scrollScroll: scroll.scrollHeight, table: box(table), viewport: innerHeight }
      })()`)
      assert.ok(geometry.viewport - geometry.main.bottom >= 0 && geometry.viewport - geometry.main.bottom <= 21, `${lens} remaining row: ${JSON.stringify(geometry)}`)
      assert.ok(Math.abs(geometry.table.top - geometry.main.top) <= 1 && Math.abs(geometry.table.bottom - geometry.main.bottom) <= 1, `${lens} table row: ${JSON.stringify(geometry)}`)
      assert.ok(Math.abs(geometry.dock.top - geometry.main.top) <= 1 && Math.abs(geometry.dock.bottom - geometry.main.bottom) <= 1, `${lens} dock row: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.scrollScroll > geometry.scrollClient, `${lens} independent table scroll: ${JSON.stringify(geometry)}`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 800 })
    await settleLayout(cdp)
    const narrowProcess = await cdp.evaluate(`(() => {
      const dock = document.querySelector('[data-testid="pg-linked-dock"]').getBoundingClientRect()
      const main = document.querySelector('.process-main').getBoundingClientRect()
      return { dockBottom: dock.bottom, dockTop: dock.top, mainBottom: main.bottom, viewport: innerHeight }
    })()`)
    assert.ok(narrowProcess.dockTop >= 0 && narrowProcess.dockBottom <= narrowProcess.viewport && narrowProcess.viewport - narrowProcess.mainBottom <= 21, JSON.stringify(narrowProcess))
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })

    summaryMode = "fail"
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "Host before same-hour summary remount")
    const hostJoinGap = await cdp.evaluate(`(() => {
      const timeline = document.querySelector('.timeline-shell').getBoundingClientRect()
      const primary = document.querySelector('.workspace > .lensbar, .workspace > .system-main').getBoundingClientRect()
      return primary.top - timeline.bottom
    })()`)
    assert.ok(hostJoinGap <= 1, `Host major-region gap ${hostJoinGap}px`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cgroups"]') !== null`, "the cgroups row", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cgroups"]') !== null`, "the cgroups ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cgroups"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="host-cgroups-modes"]') !== null`, "the cgroup modes")
    for (const [panel, mode] of [["os_cgroup_cpu", 0], ["os_cgroup_memory", 1], ["os_cgroup_io", 2]]) {
      await cdp.evaluate(`document.querySelectorAll('[data-testid="host-cgroups-modes"] button')[${mode}].click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="system-${panel}"] .entity-row') !== null`, `the ${panel} panel`, 15_000)
    }
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU metrics", 15_000)
    const systemHistoryBefore = historyRequests("os_cpu").length
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await waitForRequests(() => historyRequests("os_cpu").length > systemHistoryBefore)
    assert.ok(historyRequests("os_cpu").length > systemHistoryBefore)
    const systemSnapshots = requests.filter(({ path }) => path === `/api/segments/${SEGMENT}/snapshot`).map(({ query }) => new URLSearchParams(query))
    const primarySystem = systemSnapshots.find((query) => query.getAll("section").includes("os_cgroup_context"))
    assert.notEqual(primarySystem, undefined)
    assert.deepEqual(primarySystem.getAll("section").filter((section) => section.startsWith("os_cgroup_") && section !== "os_cgroup_context"), [])
    const expectedCgroups = {
      os_cgroup_cpu: null,
      os_cgroup_memory: null,
      os_cgroup_io: null,
    }
    for (const [section, path] of Object.entries(expectedCgroups)) {
      const matches = systemSnapshots.filter((query) => query.getAll("section").includes(section))
      assert.ok(matches.length >= 1, section)
      for (const query of matches) {
        assert.deepEqual(query.getAll("section"), [section])
        assert.equal(query.get("where.cgroup_path"), path)
        assert.equal(query.get("where.scope"), null)
      }
    }
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cgroups"]') !== null`, "the cgroups ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cgroups"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="host-cgroups-modes"]') !== null`, "the cgroup cursor workspace")
    holdCgroups = true
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "the changed System cursor")
    // Loading is not absence: the panels may hold their frames with the loading
    // line, but no prior rows are visible while the new key loads.
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => {
      const panel = document.querySelector('[data-testid="system-' + section + '"]')
      return panel === null || (panel.querySelectorAll(".entity-row").length === 0 && panel.textContent.includes("Loading rows"))
    })`, "prior cgroup rows cleared while the new key loads")
    await waitForRequests(() => heldCgroups.length === 3)
    const replacementPaths = {
      os_cgroup_cpu: null,
      os_cgroup_memory: null,
      os_cgroup_io: null,
    }
    assert.deepEqual(Object.fromEntries(heldCgroups.map(({ url }) => [url.searchParams.get("section"), url.searchParams.get("where.cgroup_path")])), replacementPaths)
    assert.equal(heldCgroups.every(({ url }) => url.searchParams.get("at") === String(BEFORE_AT) && url.searchParams.get("where.scope") === null), true)
    holdCgroups = false
    for (const held of heldCgroups.splice(0)) if (!held.response.destroyed) ndjson(held.response, cgroupSnapshotRecords(held.url))
    for (const [panel, mode] of [["os_cgroup_cpu", 0], ["os_cgroup_memory", 1], ["os_cgroup_io", 2]]) {
      await cdp.evaluate(`document.querySelectorAll('[data-testid="host-cgroups-modes"] button')[${mode}].click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="system-${panel}"] .entity-row') !== null`, "the replacement collector cgroup rows", 15_000)
    }
    holdCgroups = true
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowRight" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}"`, "the failed System cursor")
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => {
      const panel = document.querySelector('[data-testid="system-' + section + '"]')
      return panel === null || (panel.querySelectorAll(".entity-row").length === 0 && panel.textContent.includes("Loading rows"))
    })`, "prior cgroup rows cleared before a failed exact load")
    await waitForRequests(() => heldCgroups.length === 3)
    holdCgroups = false
    for (const held of heldCgroups.splice(0)) {
      if (held.response.destroyed) continue
      held.response.writeHead(200, { "Content-Type": "application/x-ndjson; charset=utf-8" })
      held.response.end("{")
    }
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]') === null`, "the cursor caught up after exact-load failures", 15_000)
    for (const [panel, mode] of [["os_cgroup_cpu", 0], ["os_cgroup_memory", 1], ["os_cgroup_io", 2]]) {
      await cdp.evaluate(`document.querySelectorAll('[data-testid="host-cgroups-modes"] button')[${mode}].click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="system-${panel}"]') === null`, `no stale ${panel} rows after exact-load failures`, 15_000)
    }
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Could not load process totals" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "719"`, "same-hour error with retained totals", 15_000)

    summaryMode = "hold"
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "System before held same-hour summary remount")
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await waitForRequests(() => heldSummaries.length !== 0)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Loading process totals…" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "719"`, "same-hour loading with retained totals", 15_000)
    const sameHourSummaries = heldSummaries.splice(0)
    for (const held of sameHourSummaries) if (!held.destroyed) ndjson(held, processSummaryRecords(HOUR, 2, 720))
    await cdp.waitFor(`document.querySelector('.process-summary > div:first-child strong')?.textContent === "720" && document.querySelector('[data-testid="process-summary-status"]') === null`, "same-hour replacement totals", 15_000)

    summaryMode = "hold"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
    await waitForRequests(() => heldSummaries.length !== 0)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Loading process totals…" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "—"`, "cleared totals during a cross-hour load", 15_000)
    summaryMode = "good"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('.process-summary > div:first-child strong')?.textContent === "721" && document.querySelector('[data-testid="process-summary-status"]') === null`, "replacement totals after the aborted request", 15_000)
    for (const held of heldSummaries) if (!held.destroyed) ndjson(held, processSummaryRecords(HOUR + HOUR_US, 2, 999))
    await delay(100)
    assert.equal(await cdp.evaluate(`document.querySelector('.process-summary > div:first-child strong')?.textContent`), "721")

    summaryMode = "fail"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Could not load process totals" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "—"`, "cross-hour summary request failure without prior totals", 15_000)
    summaryMode = "empty"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "No data in the selected hour" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "—"`, "successful empty process totals", 15_000)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "the activity table", 15_000)
    await settleLayout(cdp)
    const activityDuration = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-activity-table"]')
      const headers = [...table.querySelectorAll('[role="columnheader"]')].map((cell) => cell.textContent.trim())
      const index = headers.findIndex((label) => label.includes("Query time"))
      return index < 0 ? null : table.querySelector('.entity-row').querySelectorAll('[role="cell"]')[index]?.textContent.trim() ?? null
    })()`)
    assert.match(activityDuration, /\d/)
    const activityHistoryBefore = historyRequests("pg_stat_activity").length
    await cdp.evaluate(`(() => {
      const row = document.querySelector('[data-testid="pg-activity-table"] .entity-row')
      row.focus()
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null`, "Activity detail")
    await waitForRequests(() => historyRequests("pg_stat_activity").length > activityHistoryBefore)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"] .pg-metric-history .uplot-host canvas') !== null`, "numeric Activity history", 15_000)
    const activityHistory = historyRequests("pg_stat_activity")
    const visibleActivityHistoryCount = activityHistory.length
    const activityQuery = new URLSearchParams(activityHistory.at(-1).query)
    assert.equal(activityQuery.get("from"), String(HOUR))
    assert.equal(activityQuery.get("to"), String(HOUR + HOUR_US - 1))
    assert.equal(activityQuery.get("section"), "pg_stat_activity")
    assert.deepEqual(activityQuery.getAll("field"), ["pid", "state", "query_start"])
    assert.deepEqual([...activityQuery.keys()].filter((name) => name.startsWith("where.")), ["where.pid"])
    assert.equal(activityQuery.get("where.pid"), "4242")
    assert.equal(activityQuery.has("type_id"), false)
    const servedActivity = servedActivityHistory.at(-1)
    const servedLayout = servedActivity.find(({ record }) => record === "layout")
    assert.deepEqual(servedLayout.layout.columns.map(({ name }) => name), ["pid", "state", "query_start"])
    assert.equal(servedActivity.filter(({ record }) => record === "row").every(({ values }) => values.length === 3), true)
    const activitySample = await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"] .chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(activitySample, /Query time.*\d/)
    assert.doesNotMatch(activitySample, /—/)

    await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"] .chart-expand').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"] [role="dialog"]') !== null`, "the expanded Activity chart")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[role="dialog"]') === null`, "the Activity chart collapse")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"]') !== null`), true)

    await cdp.evaluate(`document.querySelector('[data-testid="help-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="help-panel"]') !== null`, "Help over Activity detail")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="help-panel"]') === null`, "Help close over Activity detail")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"]') !== null`), true)

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "hour picker over Activity detail")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "hour picker close over Activity detail")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"]') !== null`), true)

    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null && document.activeElement === document.querySelector('[data-testid="pg-activity-table"] .entity-row')`, "Activity detail close and row focus restore")
    const shownActivity = await cdp.evaluate(`(() => {
      const bounds = (selector) => document.querySelector(selector).getBoundingClientRect().height
      return {
        layout: bounds('[data-testid="pg-entity-layout"]'),
        main: bounds('.pg-entity-main'),
        scroll: bounds('[data-testid="pg-activity-table"] .entity-scroll'),
        table: bounds('[data-testid="pg-activity-table"]'),
        workspace: bounds('.workspace'),
      }
    })()`)
    const activitySnapshotsBeforeHiddenDetail = snapshotRequests("pg_stat_activity").length
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.timeline-shell') === null`, "activity charts hidden")
    await settleLayout(cdp)
    const hiddenActivity = await cdp.evaluate(`(() => {
      const bounds = (selector) => document.querySelector(selector).getBoundingClientRect().height
      return {
        layout: bounds('[data-testid="pg-entity-layout"]'),
        main: bounds('.pg-entity-main'),
        scroll: bounds('[data-testid="pg-activity-table"] .entity-scroll'),
        table: bounds('[data-testid="pg-activity-table"]'),
        workspace: bounds('.workspace'),
      }
    })()`)
    assert.ok(hiddenActivity.scroll > shownActivity.scroll + 100, JSON.stringify({ hiddenActivity, shownActivity }))
    await cdp.evaluate(`(() => {
      const row = document.querySelector('[data-testid="pg-activity-table"] .entity-row')
      row.focus()
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null && document.querySelector('[data-testid="pg-detail"] .pg-metric-history') === null`, "Activity detail with charts hidden")
    await waitForRequests(() => snapshotRequests("pg_stat_activity").length > activitySnapshotsBeforeHiddenDetail)
    await delay(100)
    assert.equal(historyRequests("pg_stat_activity").length, visibleActivityHistoryCount)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-exact-query"]')?.textContent`), "select artifact_wire_contract")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null`, "hidden Activity detail close")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.timeline-shell') !== null`, "activity charts restored")
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Tables")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] .entity-row') !== null`, "the relation table", 15_000)
    const relationHistoryBefore = historyRequests("pg_stat_user_tables").length
    await cdp.evaluate(`(() => {
      const row = document.querySelector('[data-testid="pg-tables-table"] .entity-row')
      row.focus()
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))
    })()`)
    await cdp.waitFor(`document.querySelector('.pg-metric-history') !== null`, "the relation history panel")
    await waitForRequests(() => historyRequests("pg_stat_user_tables").length > relationHistoryBefore)
    await settleLayout(cdp)
    const shownRelationHeight = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').getBoundingClientRect().height`)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') === null && document.activeElement === document.querySelector('[data-testid="pg-tables-table"] .entity-row')`, "relation detail close and row focus restore")
    await cdp.evaluate(`document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"] .pg-metric-history') !== null`, "the reopened relation history")
    await waitForRequests(() => historyRequests("pg_stat_user_tables").length > relationHistoryBefore + 1)
    const visibleRelationHistoryCount = historyRequests("pg_stat_user_tables").length
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .pg-metric-history') === null`, "all relation charts hidden")
    await settleLayout(cdp)
    const hiddenRelationHeight = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').getBoundingClientRect().height`)
    assert.ok(hiddenRelationHeight > shownRelationHeight + 100, JSON.stringify({ hiddenRelationHeight, shownRelationHeight }))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') === null`, "hidden relation detail close")
    await cdp.evaluate(`document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') !== null && document.querySelector('[data-testid="pg-relation-detail"] .pg-metric-history') === null`, "relation detail reopened with charts hidden")
    await delay(100)
    assert.equal(historyRequests("pg_stat_user_tables").length, visibleRelationHistoryCount)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-row') !== null`), true)

    const hiddenSystemSnapshotsBefore = snapshotRequests("os_cpu").length
    const hiddenSystemHistoryBefore = historyRequests("os_cpu").length
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.section-tabs [role="tab"]:first-child') !== null`, "Host sections")
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "System with charts hidden")
    await waitForRequests(() => snapshotRequests("os_cpu").length > hiddenSystemSnapshotsBefore)
    await delay(100)
    assert.equal(historyRequests("os_cpu").length, hiddenSystemHistoryBefore)
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .use-table, .use-history, .system-entity-history') === null`), true)
    const hiddenSummaryBefore = historyRequests("os_process_summary").length
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('.process-table') !== null`, "Processes with charts hidden")
    await waitForRequests(() => historyRequests("os_process_summary").length > hiddenSummaryBefore)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "No data in the selected hour"`, "visible process cards retained with charts hidden")
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .process-summary-history, .process-history') === null`), true)
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent === "Events")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="events-console"]') !== null`, "Events with charts hidden")
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty') === null`), true)

    const persistedSystemHistoryBefore = historyRequests("os_cpu").length
    const persistedSystemSnapshotsBefore = snapshotRequests("os_cpu").length
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="charts-toggle"]')?.getAttribute("aria-label") === "Show charts" && document.querySelector('.system-main') !== null`, "the persisted hidden preference", 15_000)
    await waitForRequests(() => snapshotRequests("os_cpu").length > persistedSystemSnapshotsBefore)
    await delay(100)
    assert.equal(historyRequests("os_cpu").length, persistedSystemHistoryBefore)
    assert.equal(await cdp.evaluate(`localStorage.getItem("kronika.charts")`), "0")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.timeline-shell') !== null`, "charts shown again")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("PostgreSQL is unavailable without current telemetry and returns for a stored hour", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  let historical = false
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, sourceTimelineRecords(historical))
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      return ndjson(response, sections.includes("pg_stat_activity") ? snapshotRecords() : systemSnapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("source browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.overview` })
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null && document.querySelectorAll('.source-tabs button')[2]?.getAttribute('aria-current') === "page"`, "the explicit PostgreSQL destination", 15_000)
    const unavailable = await cdp.evaluate(`(() => {
      const sourceButtons = document.querySelectorAll('.source-tabs button')
      return {
        pgDisabled: sourceButtons[2].disabled,
        pgPanels: document.querySelectorAll('.pg-tabs, .pg-overview, [data-testid^="pg-"]').length,
        pgHealth: document.querySelector('[data-primary]')?.textContent.includes('PostgreSQL') ?? false,
        view: new URL(location.href).searchParams.get('view'),
      }
    })()`)
    assert.deepEqual(unavailable, { pgDisabled: false, pgHealth: false, pgPanels: 1, view: "pg.overview" })
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "the Host destination", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the host CPU cards", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition"] .u-over') !== null`, "the CPU composition history")
    await cdp.evaluate(`document.querySelector('[data-testid="system-cpu-composition-all"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition-all"]')?.getAttribute("aria-pressed") === "true"`, "the whole CPU composition")
    await cdp.evaluate(`(() => {
      const plot = document.querySelector('[data-testid="system-cpu-composition"] .u-over')
      const bounds = plot.getBoundingClientRect()
      const clientX = bounds.left + (${AT} - ${HOUR}) / ${HOUR_US} * bounds.width
      const clientY = bounds.top + bounds.height / 2
      plot.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, clientX, clientY }))
      plot.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX, clientY }))
    })()`)
    await cdp.waitFor(`(() => {
      const chart = document.querySelector('[data-testid="system-cpu-composition"]')
      const tooltip = chart?.querySelector('.chart-tooltip')
      const host = chart?.querySelector('.uplot-host')
      if (chart === null || tooltip === null || host === null) return false
      window.__kronikaCpuChart = { axes: chart.querySelectorAll('.u-axis').length, label: host.getAttribute('aria-label'), tooltip: tooltip.textContent }
      return true
    })()`, "the CPU composition tooltip")
    const cpuChart = await cdp.evaluate(`window.__kronikaCpuChart`)
    assert.equal(cpuChart.axes, 3)
    for (const label of ["CPU used, cores", "CPU cores", "User CPU", "System CPU", "IRQ", "I/O wait", "Steal", "Idle"]) {
      assert.match(cpuChart.label, new RegExp(label))
      assert.match(cpuChart.tooltip, new RegExp(label))
    }
    assert.equal(await cdp.evaluate(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`), true)
    historical = true
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.overview` })
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null && document.querySelectorAll('.source-tabs button')[2]?.getAttribute('aria-current') === "page"`, "the stored PostgreSQL hour", 15_000)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].disabled`), false)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("PostgreSQL detail dock stays inside the viewport", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  const fixture = viewportActivityRows(new URL("http://fixture/snapshot"))
  assert.equal(fixture[0]?.layout?.logical_name, "pg_stat_activity")
  assert.equal(fixture.length, 121)
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(`${url.pathname}?${url.searchParams}`)
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      return ndjson(response, url.searchParams.get("section") === "pg_stat_activity"
        ? viewportActivityHistory(url)
        : viewportActivityTimeline())
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, viewportActivityRows(url))
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("viewport browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  const measurements = []
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]') !== null`, "the viewport activity table", 15_000)
    await waitForRequests(() => requests.some((value) => value.startsWith(`/api/segments/${SEGMENT}/snapshot?`)))
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "the viewport activity rows", 15_000)

    for (const [width, height] of [[1280, 882], [1366, 768], [960, 882], [390, 480]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await settleLayout(cdp)
      await cdp.evaluate(`document.querySelector('.inspector-close')?.click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null`, `${width}x${height} closed detail`)
      await cdp.evaluate(`(() => {
        const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll')
        scroll.scrollLeft = Math.min(211, scroll.scrollWidth - scroll.clientWidth)
        scroll.scrollTop = Math.min(184, scroll.scrollHeight - scroll.clientHeight)
      })()`)
      await settleLayout(cdp)
      const before = await cdp.evaluate(viewportTableGeometry())
      assert.equal(before.table.scrollAxis, "both", `${width}x${height} dense table axis: ${JSON.stringify(before)}`)
      assert.equal(before.table.overflowY, "auto", `${width}x${height} dense vertical owner: ${JSON.stringify(before)}`)
      assert.equal(before.table.vertical, true, `${width}x${height} dense vertical overflow: ${JSON.stringify(before)}`)
      assert.ok(before.table.scrollHeight > before.table.clientHeight, `${width}x${height} dense scroll range: ${JSON.stringify(before)}`)
      await cdp.evaluate(`(() => {
        const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll')
        const row = [...document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row')].find((candidate) => {
          const bounds = candidate.getBoundingClientRect()
          const viewport = scroll.getBoundingClientRect()
          return bounds.top >= viewport.top && bounds.bottom <= viewport.bottom
        })
        row.click()
      })()`)
      await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null`, `${width}x${height} detail dock`, 15_000)
      await settleLayout(cdp)
      const opened = await cdp.evaluate(viewportDockGeometry())

      assert.equal(opened.selectedPid, before.visiblePid, `${width}x${height} selected activity row`)
      assert.ok(Math.abs(opened.table.scrollLeft - before.table.scrollLeft) <= 1, `${width}x${height} horizontal scroll on open: ${JSON.stringify({ before, opened })}`)
      assert.ok(Math.abs(opened.table.scrollTop - before.table.scrollTop) <= 1, `${width}x${height} vertical scroll on open: ${JSON.stringify({ before, opened })}`)
      assert.ok(Math.abs(opened.layout.height - before.layout.height) <= 1, `${width}x${height} outer row height: ${JSON.stringify({ before, opened })}`)
      assert.ok(Math.abs(opened.table.height - before.table.height) <= 1, `${width}x${height} table height: ${JSON.stringify({ before, opened })}`)
      if (width <= 1000) {
        assert.ok(Math.abs(opened.table.width - before.table.width) <= 1, `${width}x${height} overlay table width: ${JSON.stringify({ before, opened })}`)
      }
      assert.ok(opened.document.scrollHeight <= opened.document.clientHeight + 1, `${width}x${height} document height: ${JSON.stringify(opened)}`)
      assert.ok(opened.table.bottom <= opened.document.clientHeight + 1, `${width}x${height} reachable table rail: ${JSON.stringify(opened)}`)
      assert.ok(opened.table.scrollWidth > opened.table.clientWidth, `${width}x${height} horizontal table: ${JSON.stringify(opened)}`)
      assert.ok(opened.table.railHeight > 0, `${width}x${height} horizontal rail height: ${JSON.stringify(opened)}`)
      assert.ok(opened.dock.top >= -1 && opened.dock.bottom <= height + 1 && opened.dock.left >= -1 && opened.dock.right <= width + 1, `${width}x${height} dock bounds: ${JSON.stringify(opened)}`)
      const detailOverflows = opened.body.scrollHeight > opened.body.clientHeight

      await cdp.evaluate(`(() => { const body = document.querySelector('.inspector-body'); body.scrollTop = body.scrollHeight })()`)
      await settleLayout(cdp)
      const scrolled = await cdp.evaluate(viewportDockGeometry())
      if (detailOverflows) assert.ok(scrolled.body.scrollTop > 0, `${width}x${height} detail scroll: ${JSON.stringify(scrolled)}`)
      assert.ok(Math.abs(scrolled.table.scrollTop - before.table.scrollTop) <= 1, `${width}x${height} independent table scroll: ${JSON.stringify({ before, scrolled })}`)
      assert.ok(scrolled.header.top >= scrolled.dock.top - 1 && scrolled.header.bottom <= scrolled.dock.bottom + 1, `${width}x${height} sticky header: ${JSON.stringify(scrolled)}`)
      assert.ok(scrolled.close.top >= scrolled.dock.top - 1 && scrolled.close.bottom <= scrolled.dock.bottom + 1, `${width}x${height} visible close: ${JSON.stringify(scrolled)}`)
      assert.ok(scrolled.lastDetail.top >= scrolled.body.top - 1 && scrolled.lastDetail.bottom <= scrolled.body.bottom + 1, `${width}x${height} reachable detail fields: ${JSON.stringify(scrolled)}`)

      await cdp.evaluate(`([...document.querySelectorAll('.inspector-tabs button')].find((button) => button.textContent === 'Chart')).click()`)
      await cdp.waitFor(`document.querySelector('.inspector-chart-slot .uplot-figure') !== null`, `${width}x${height} detail chart`, 15_000)
      await settleLayout(cdp)
      const chartTab = await cdp.evaluate(`(() => {
        const body = document.querySelector('.inspector-body').getBoundingClientRect()
        const chart = document.querySelector('.inspector-chart-slot .uplot-figure').getBoundingClientRect()
        return { body: { bottom: body.bottom, left: body.left, right: body.right, top: body.top }, chart: { bottom: chart.bottom, height: chart.height, left: chart.left, right: chart.right, top: chart.top } }
      })()`)
      assert.ok(chartTab.chart.height >= 180 && chartTab.chart.height <= 220, `${width}x${height} chart cap: ${JSON.stringify(chartTab)}`)
      assert.ok(chartTab.chart.left >= chartTab.body.left - 1 && chartTab.chart.right <= chartTab.body.right + 1, `${width}x${height} chart inside the Inspector: ${JSON.stringify(chartTab)}`)

      await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null`, `${width}x${height} detail close`)
      const closed = await cdp.evaluate(viewportTableGeometry())
      assert.ok(Math.abs(closed.table.scrollLeft - before.table.scrollLeft) <= 1, `${width}x${height} horizontal scroll on close: ${JSON.stringify({ before, closed })}`)
      assert.ok(Math.abs(closed.table.scrollTop - before.table.scrollTop) <= 1, `${width}x${height} vertical scroll on close: ${JSON.stringify({ before, closed })}`)
      await cdp.evaluate(`(() => { const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll'); scroll.focus(); scroll.scrollLeft = scroll.scrollWidth })()`)
      const rail = await cdp.evaluate(`(() => { const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll'); return { active: document.activeElement === scroll, clientWidth: scroll.clientWidth, left: scroll.scrollLeft, scrollWidth: scroll.scrollWidth } })()`)
      assert.equal(rail.active, true, `${width}x${height} focusable rail`)
      assert.ok(rail.left >= rail.scrollWidth - rail.clientWidth - 1, `${width}x${height} rightmost columns: ${JSON.stringify(rail)}`)
      measurements.push({ height, width, before, opened, scrolled: { bodyScrollTop: scrolled.body.scrollTop, headerTop: scrolled.header.top } })
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
    process.stdout.write(`${JSON.stringify({ measurements }, null, 2)}\n`)
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("structured search pending state and snapshot targets preserve exact newest results", { timeout: 120_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  let statementAttempts = 0
  let relationAttempts = 0
  let pendingSystemFailure = null
  let pendingActivityFailure = null
  let pendingStatementFailure = null
  let pendingRelationFailure = null
  let pendingProcessSearch = null
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      return ndjson(response, url.searchParams.has("section") ? [] : snapshotTargetTimelineRecords())
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const at = Number(url.searchParams.get("at") ?? AT)
      const sections = url.searchParams.getAll("section")
      if (sections.includes("os_cpu")) {
        if (at === BEFORE_AT) { pendingSystemFailure = response; return }
        return ndjson(response, systemSnapshotRecords(false, at))
      }
      if (sections.includes("os_process")) {
        if (url.searchParams.get("search") === "cpu_cores>1") { pendingProcessSearch = response; return }
        return ndjson(response, snapshotRecords())
      }
      if (sections.includes("pg_stat_activity")) {
        if (at === BEFORE_AT) { pendingActivityFailure = response; return }
        return ndjson(response, targetedActivityRecords("activity_target_A", at))
      }
      if (sections.includes("pg_stat_statements")) {
        if (url.searchParams.get("search") === "target-b") {
          statementAttempts += 1
          if (statementAttempts === 1) { pendingStatementFailure = response; return }
          return ndjson(response, targetedStatementRecords("statement_target_B", 222))
        }
        return ndjson(response, targetedStatementRecords("statement_target_A", 111))
      }
      if (sections.includes("pg_stat_user_tables")) {
        if (url.searchParams.get("group") === "schema") {
          relationAttempts += 1
          if (relationAttempts === 1) { pendingRelationFailure = response; return }
          return ndjson(response, targetedRelationRecords(url, "relation_target_B", 444))
        }
        return ndjson(response, targetedRelationRecords(url, "relation_target_A", 333))
      }
      return ndjson(response, [])
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("snapshot-target browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the host ledger", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    // CPU, memory and the device panel answer on their own sections now.
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "ordinary System target A cpu", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-memory"]') !== null`, "the memory ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-memory"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-mem_available"]') !== null`, "ordinary System target A memory", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "the disk ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-panel-os_diskstats"]')?.textContent.includes("device_target_A") === true`, "ordinary System target A devices", 15_000)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "ordinary System target B")
    await waitForRequests(() => pendingSystemFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"] .loading-ring') !== null`, "ordinary System target B loading", 15_000)
    // The metric chips clear while the new key loads; the rows stay expanded
    // and the device panel keeps its frame, saying it is loading.
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') === null`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-metric-mem_available"]') === null`), true)
    assert.equal(await cdp.evaluate(`(() => {
      const panel = document.querySelector('[data-testid="system-panel-os_diskstats"]')
      return panel !== null && panel.querySelectorAll(".entity-row").length === 0 && panel.textContent.includes("Loading rows")
    })()`), true)
    brokenNdjson(pendingSystemFailure)
    pendingSystemFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]')?.classList.contains("cursor-missing") === true`, "ordinary System target B error", 15_000)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') === null && document.querySelector('[data-testid="system-metric-mem_available"]') === null && document.querySelector('[data-testid="system-panel-os_diskstats"]') === null`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"]') !== null`), true)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]')?.textContent.includes("activity_target_A") === true`, "ordinary PostgreSQL target A", 15_000)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "ordinary PostgreSQL target B")
    await waitForRequests(() => pendingActivityFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"] .loading-ring') !== null`, "ordinary PostgreSQL target B loading", 15_000)
    assert.equal(await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-activity-table"]')
      return table !== null && !table.textContent.includes("activity_target_A") && table.querySelector('.entity-row') === null
    })()`), true)
    brokenNdjson(pendingActivityFailure)
    pendingActivityFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]')?.classList.contains("cursor-missing") === true`, "ordinary PostgreSQL target B error", 15_000)
    assert.equal(await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-activity-table"]')
      return table !== null && !table.textContent.includes("activity_target_A") && table.querySelector('.entity-row') === null
    })()`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"]') !== null`), true)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.statements` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"]')?.textContent.includes("statement_target_A") === true`, "dense statement target A", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 111/)
    const searchGeometry = async () => cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-statements-table"]')
      const status = table.querySelector('[data-testid="table-status"]')
      const scroll = table.querySelector('.entity-scroll')
      const filter = table.querySelector('[data-search-surface]')
      const rect = (node) => { const value = node.getBoundingClientRect(); return { bottom: value.bottom, height: value.height, left: value.left, right: value.right, top: value.top, width: value.width } }
      return {
        busy: table.getAttribute('aria-busy'),
        client: document.documentElement.clientWidth,
        filter: rect(filter),
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        scroll: rect(scroll),
        status: rect(status),
        table: rect(table),
      }
    })()`)
    const readySearchGeometry = new Map()
    for (const locale of ["en", "ru"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
        await settleLayout(cdp)
        readySearchGeometry.set(`${locale}:${width}`, await searchGeometry())
      }
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "target-b AND")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="table-filter"]')?.getAttribute("aria-invalid") === "true"`, "invalid search draft")
    assert.equal(pendingStatementFailure, null)
    assert.equal(statementAttempts, 0)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), null)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "target-b")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await waitForRequests(() => pendingStatementFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"] [role="status"]') !== null`, "dense statement target B search status", 15_000)
    const loadingStatement = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-statements-table"]')
      const request = table.querySelector('[data-testid="table-status"] [role]')
      return {
        busy: table.getAttribute('aria-busy'),
        empty: table.querySelector('.table-empty')?.textContent ?? null,
        live: request?.getAttribute('aria-live'),
        role: request?.getAttribute('role'),
        rows: table.querySelectorAll('.entity-row').length,
        status: table.querySelector('[data-testid="table-status"]').textContent,
        text: table.textContent,
      }
    })()`)
    assert.equal(loadingStatement.rows, 1)
    assert.equal(loadingStatement.busy, "true")
    assert.equal(loadingStatement.live, "polite")
    assert.equal(loadingStatement.role, "status")
    assert.equal(loadingStatement.empty, null)
    assert.match(loadingStatement.text, /statement_target_A/)
    assert.match(loadingStatement.status, /Searching… Rows retained/)
    assert.doesNotMatch(loadingStatement.status, /Loaded|111|0 of 0/)
    for (const locale of ["en", "ru"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
        await settleLayout(cdp)
        const pending = await searchGeometry()
        const ready = readySearchGeometry.get(`${locale}:${width}`)
        assert.ok(Math.abs(pending.status.height - ready.status.height) <= 1, `${locale}:${width}:status ${JSON.stringify({ pending, ready })}`)
        assert.ok(Math.abs(pending.scroll.height - ready.scroll.height) <= 1, `${locale}:${width}:scroll ${JSON.stringify({ pending, ready })}`)
        assert.equal(pending.busy, "true", `${locale}:${width}:busy`)
        assert.equal(pending.overflow, false, `${locale}:${width}:overflow ${JSON.stringify(pending)}`)
        assert.ok(pending.filter.left >= -1 && pending.filter.right <= pending.client + 1, `${locale}:${width}:filter ${JSON.stringify(pending)}`)
        const statusText = await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]').textContent`)
        assert.match(statusText, locale === "ru" ? /Идёт поиск/ : /Searching/)
      }
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    brokenNdjson(pendingStatementFailure)
    pendingStatementFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"] [role="alert"]') !== null`, "dense statement target B error", 15_000)
    const failedStatement = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-statements-table"]')
      return { rows: table.querySelectorAll('.entity-row').length, status: table.querySelector('[data-testid="table-status"]').textContent, text: table.textContent }
    })()`)
    assert.equal(failedStatement.rows, 1)
    assert.match(failedStatement.text, /statement_target_A/)
    assert.match(failedStatement.status, /Search failed. Rows retained./)
    assert.doesNotMatch(failedStatement.status, /Loaded|111/)
    await cdp.evaluate(`document.querySelector('[data-testid="table-paging"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"]')?.textContent.includes("statement_target_B") === true`, "dense statement target B retry", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 222/)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&find=cpu_cores%3E1` })
    await waitForRequests(() => pendingProcessSearch !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="process-table"] [data-testid="table-status"] [role="status"]') !== null`, "initial Process search pending", 15_000)
    const emptyPendingProcess = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      return {
        busy: table.getAttribute('aria-busy'),
        empty: table.querySelector('.table-empty')?.textContent ?? null,
        rows: table.querySelectorAll('.entity-row').length,
        status: table.querySelector('[data-testid="table-status"]').textContent,
        text: table.textContent,
      }
    })()`)
    assert.equal(emptyPendingProcess.busy, "true")
    assert.equal(emptyPendingProcess.rows, 0)
    assert.match(emptyPendingProcess.empty, /Searching/)
    assert.match(emptyPendingProcess.status, /^Searching/)
    assert.doesNotMatch(emptyPendingProcess.text, /No rows match|Loaded 0 of 0/)
    ndjson(pendingProcessSearch, snapshotRecords())
    pendingProcessSearch = null
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 0 && document.querySelector('[data-testid="process-table"] [data-testid="table-status"] [role]') === null`, "initial Process search success", 15_000)

    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null`, "PostgreSQL surface before Activity", 15_000)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Activity'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]') !== null`, "remembered Activity surface", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-tab"]')?.getAttribute('aria-current') === 'page'`, "Processes before scoped search", 15_000)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "cpu_cores>1")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await waitForRequests(() => pendingProcessSearch !== null)
    ndjson(pendingProcessSearch, snapshotRecords())
    pendingProcessSearch = null
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "cpu_cores>1" && document.querySelector('[data-testid="process-table"] [data-testid="table-status"] [role]') === null`, "applied Process expression", 15_000)
    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.activity" && document.querySelector('[data-testid="pg-activity-table"]') !== null`, "Process to Activity navigation", 15_000)
    const activityNavigation = await cdp.evaluate(`(() => ({
      error: document.querySelector('[data-testid="search-error"]')?.textContent ?? null,
      find: new URL(location.href).searchParams.get('find'),
      input: document.querySelector('[data-testid="table-filter"]')?.value ?? null,
    }))()`)
    assert.deepEqual(activityNavigation, { error: null, find: null, input: "" })
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "cpu_cores>1" && document.querySelector('[data-testid="process-tab"]')?.getAttribute('aria-current') === 'page'`, "Back restoring Process expression", 15_000)
    await waitForRequests(() => pendingProcessSearch !== null)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), "cpu_cores>1")
    ndjson(pendingProcessSearch, snapshotRecords())
    pendingProcessSearch = null

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.tables` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"]')?.textContent.includes("relation_target_A") === true`, "relation target A", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 333/)
    await cdp.evaluate(`document.querySelector('nav.lensbar .lens-tabs button:nth-child(2)').click()`)
    await waitForRequests(() => pendingRelationFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="table-paging"] button')?.textContent === "…"`, "relation target B loading", 15_000)
    const loadingRelation = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-tables-table"]')
      return { rows: table.querySelectorAll('.entity-row').length, status: table.querySelector('[data-testid="table-status"]').textContent, text: table.textContent }
    })()`)
    assert.equal(loadingRelation.rows, 0)
    assert.doesNotMatch(loadingRelation.text, /relation_target_A/)
    assert.doesNotMatch(loadingRelation.status, /333/)
    brokenNdjson(pendingRelationFailure)
    pendingRelationFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="table-paging"] button')?.textContent === "↻"`, "relation target B error", 15_000)
    const failedRelation = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-tables-table"]')
      return { rows: table.querySelectorAll('.entity-row').length, status: table.querySelector('[data-testid="table-status"]').textContent, text: table.textContent }
    })()`)
    assert.equal(failedRelation.rows, 0)
    assert.doesNotMatch(failedRelation.text, /relation_target_A/)
    assert.doesNotMatch(failedRelation.status, /333/)
    assert.match(failedRelation.status, /Calculation interval: unavailable/)
    await cdp.evaluate(`document.querySelector('[data-testid="table-paging"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"]')?.textContent.includes("relation_target_B") === true`, "relation target B retry", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 444/)

    assert.equal(statementAttempts, 2)
    assert.equal(relationAttempts, 2)
    assert.deepEqual(page.errors.filter((message) => !message.includes("kronika: snapshot at the cursor failed") && !message.includes("kronika: snapshot page failed")), [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("production health keeps staggered components on one stored evaluation", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const page = { errors: [], external: [], responses: [] }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      if (url.searchParams.has("section")) return ndjson(response, [])
      return ndjson(response, healthContractTimeline(Number(url.searchParams.get("from") ?? HOUR)))
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      return ndjson(response, sections.includes("instance_metadata")
        ? healthMetadataRecords(Number(url.searchParams.get("at") ?? AT))
        : productionSystemSnapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("health contract server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })

    const readAt = async (hour, fragments) => {
      const evaluation = hour + 1_800_000_000
      await cdp.send("Page.navigate", { url: `${origin}/?at=${evaluation}&view=host.overview` })
      await cdp.waitFor(`document.querySelector('[data-primary] [data-testid="lane-reading"]') !== null`, "the health contract reading", 15_000)
      await cdp.waitFor(`(() => {
        const text = document.querySelector('[data-primary] [data-testid="lane-reading"]')?.textContent ?? ''
        return ${JSON.stringify(fragments)}.every((fragment) => text.includes(fragment))
      })()`, "the expected health component values", 15_000)
      const reading = await cdp.evaluate(`document.querySelector('[data-primary] [data-testid="lane-reading"]').textContent`)
      await cdp.evaluate(`(() => {
        const plot = document.querySelector('[data-testid="hour-timeline"] .u-over')
        const bounds = plot.getBoundingClientRect()
        const clientX = bounds.left + (${evaluation} - ${hour}) / ${HOUR_US} * bounds.width
        const clientY = bounds.top + bounds.height / 2
        plot.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, clientX, clientY }))
        plot.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX, clientY }))
      })()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .chart-tooltip') !== null`, "the health contract tooltip")
      const tooltip = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] .chart-tooltip').textContent`)
      return { reading, tooltip }
    }

    const fresh = await readAt(HOUR, ["Overall 70%", "OS 80%", "PostgreSQL 90%"])
    for (const output of [fresh.reading, fresh.tooltip]) {
      assert.match(output, /Overall[^\d]*70%/)
      assert.match(output, /OS[^\d]*80%/)
      assert.match(output, /PostgreSQL[^\d]*90%/)
    }

    const staleHour = HOUR + HOUR_US
    const stale = await readAt(staleHour, ["Overall —", "OS 80%", "PostgreSQL —"])
    for (const output of [stale.reading, stale.tooltip]) {
      assert.match(output, /Overall[^\d]*—/)
      assert.match(output, /OS[^\d]*80%/)
      assert.match(output, /PostgreSQL[^\d]*—/)
      assert.doesNotMatch(output, /PostgreSQL[^\d]*90%/)
    }

    const disabledHour = HOUR + 2 * HOUR_US
    const disabled = await readAt(disabledHour, ["Overall 84%", "OS 84%"])
    for (const output of [disabled.reading, disabled.tooltip]) {
      assert.match(output, /Overall[^\d]*84%/)
      assert.match(output, /OS[^\d]*84%/)
      assert.doesNotMatch(output, /PostgreSQL/)
    }
    assert.equal(await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].disabled`), false)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("production System projections show exact CPU memory and device readings", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  const page = { errors: [], external: [], responses: [] }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const section = url.searchParams.get("section")
      return ndjson(response, section === null ? productionSystemTimeline() : productionSystemHistoryRecords(url))
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, productionSystemSnapshotRecords())
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("System contract server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 1280 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the host ledger", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU chip", 15_000)

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition"] .u-over') !== null`, "the CPU contract chart")
    await waitForRequests(() => requests.some(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_cpu"))
    await cdp.evaluate(`document.querySelector('[data-testid="system-cpu-composition-all"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition-all"]')?.getAttribute("aria-pressed") === "true"`, "the whole CPU contract composition")
    await delay(100)
    await hoverContractChart(cdp, "system-cpu-composition")
    const cpu = await cdp.evaluate(`document.querySelector('[data-testid="system-cpu-composition"] .chart-tooltip').textContent`)
    for (const expected of [
      /CPU used, cores[^\d]*0\.9/, /CPU cores[^\d]*2/, /User CPU[^\d]*25%/,
      /System CPU[^\d]*10%/, /I\/O wait[^\d]*5%/, /Steal[^\d]*5%/,
    ]) assert.match(cpu, expected)

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-memory"]') !== null`, "the memory ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-memory"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-mem_available"]') !== null`, "the memory chip", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-memory"]') !== null`, "the memory ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-memory"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-mem_available"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-memory-composition"] .u-over') !== null`, "the memory contract chart")
    await waitForRequests(() => requests.some(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_meminfo"))
    await cdp.evaluate(`document.querySelector('[data-testid="system-memory-composition-all"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-memory-composition-all"]')?.getAttribute("aria-pressed") === "true"`, "the whole memory contract composition")
    await delay(100)
    await hoverContractChart(cdp, "system-memory-composition")
    const memoryTooltip = await cdp.evaluate(`document.querySelector('[data-testid="system-memory-composition"] .chart-tooltip').textContent`)
    for (const expected of [
      /MemTotal[^\d]*1 MiB/, /AnonPages[^\d]*256 KiB/, /Page cache[^\d]*192 KiB/,
      /Reclaimable slab[^\d]*64 KiB/, /Unreclaimable slab[^\d]*32 KiB/, /MemFree[^\d]*128 KiB/,
      /Other memory[^\d]*352 KiB/,
    ]) assert.match(memoryTooltip, expected)

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "the disk ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="system-os_diskstats"] .entity-row').length === 2`, "the two projected devices", 15_000)
    const devices = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="system-os_diskstats"]')
      const headers = [...table.querySelectorAll('[role="columnheader"]')].map((cell) => cell.querySelector('.entity-sort span')?.textContent.trim() ?? '')
      return [...table.querySelectorAll('.entity-row')].map((row) => Object.fromEntries(
        [...row.querySelectorAll('[role="cell"]')].map((cell, index) => [headers[index], cell.textContent.trim()]),
      ))
    })()`)
    const sda = devices.find((device) => device.Device === "sda")
    const sdb = devices.find((device) => device.Device === "sdb")
    assert.equal(sda["Major:minor"], "8:0")
    assert.equal(sda["Read latency"], "5 ms")
    assert.equal(sda["Write latency"], "7 ms")
    assert.equal(sdb["Major:minor"], "8:1")
    assert.equal(sdb["Read latency"], "—")
    assert.equal(sdb["Write latency"], "—")

    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="system-os_diskstats"] .entity-row')].find((row) => row.textContent.includes('sda'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-os_diskstats-detail"] dl > .detail-row') !== null`, "the device Detail facts")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-os_diskstats-detail"] .uplot-host') === null`), true)
    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot [data-testid="system-os_diskstats-history"]') !== null`, "the device history Chart")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') === null`), true)
    assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('.timeline-preview').getBoundingClientRect().height`) - 124) <= .5)
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="system-os_diskstats-history"] .system-history-selector button')].find((button) => button.textContent.includes('Read latency'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-os_diskstats-history"] .chart-current')?.textContent === "5 ms"`, "the selected read-latency value", 15_000)
    await hoverContractChart(cdp, "system-os_diskstats-history")
    const diskTooltip = await cdp.evaluate(`document.querySelector('[data-testid="system-os_diskstats-history"] .chart-tooltip').textContent`)
    assert.match(diskTooltip, /Read latency[^\d]*5 ms/)
    const diskHistory = requests.map(({ path, query }) => ({ path, query: new URLSearchParams(query) }))
      .find(({ path, query }) => path === "/api/hour" && query.get("section") === "os_diskstats" && query.getAll("field").includes("read_time_ms"))
    assert.notEqual(diskHistory, undefined)
    assert.equal(diskHistory.query.get("where.major"), "8")
    assert.equal(diskHistory.query.get("where.minor"), "0")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

function healthContractTimeline(hour) {
  const evaluation = hour + 1_800_000_000
  const scenario = Math.round((hour - HOUR) / HOUR_US)
  const postgresql = scenario !== 2
  const postgresAt = evaluation - (scenario === 0 ? 5_000_000 : 15_000_000)
  const health = scenario === 2
    ? [
        { record: "point", type_id: "0", series: "os_health", ts: String(evaluation), identity: {}, value: 84 },
        { record: "point", type_id: "0", series: "overall_health", ts: String(evaluation), identity: {}, value: 84 },
      ]
    : [
        { record: "point", type_id: "0", series: "postgres_health", ts: String(postgresAt), identity: {}, value: 90 },
        { record: "point", type_id: "0", series: "os_health", ts: String(evaluation), identity: {}, value: 80 },
        { record: "point", type_id: "0", series: "overall_health", ts: String(evaluation), identity: {}, value: scenario === 0 ? 70 : null },
      ]
  return [
    { record: "hour", from: String(hour), to: String(hour + HOUR_US - 1), available_hours: [HOUR, HOUR + HOUR_US, HOUR + 2 * HOUR_US].map(String) },
    {
      record: "catalog", from: String(hour), to: String(hour + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: postgresql, present: postgresql, metrics_present: postgresql }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(hour), max_ts: String(evaluation),
      sections: [
        { logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001", implementation: "linux", source_family: "system", rows: "3", bytes: "384" },
        ...(postgresql ? [{ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256" }] : []),
      ],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    ...health,
  ]
}

function healthMetadataRecords(at) {
  return [
    {
      record: "layout", rates: [],
      layout: { type_id: "1000001", logical_name: "instance_metadata", columns: [{ name: "postgresql_interval_seconds" }] },
    },
    { record: "row", segment_id: SEGMENT, type_id: "1000001", ordinal: "0", timestamp: String(at), values: [10] },
  ]
}

function productionSystemTimeline() {
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    {
      record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: false, present: false, metrics_present: false }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AT),
      sections: [
        { logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001", implementation: "linux", source_family: "system", rows: "6", bytes: "768" },
        { logical_name: "os_meminfo", physical_name: "os_meminfo", type_id: "1104001", implementation: "linux", source_family: "system", rows: "2", bytes: "256" },
        { logical_name: "os_diskstats", physical_name: "os_diskstats", type_id: "1108001", implementation: "linux", source_family: "system", rows: "4", bytes: "512" },
      ],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(AT), identity: {}, value: 88 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(AT), identity: {}, value: 88 },
  ]
}

function productionSystemSnapshotRecords(at = AT) {
  const cpuColumns = ["ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal", "scope"]
  const cpuRates = ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"]
  const memoryColumns = ["ts", "mem_available", "mem_total", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim", "swap_free", "swap_total"]
  const diskColumns = ["ts", "major", "minor", "device", "reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_in_progress", "io_time_ms", "io_weighted_time_ms", "scope"]
  const diskRates = ["reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_time_ms", "io_weighted_time_ms"]
  return [
    { record: "layout", rates: cpuRates, layout: { type_id: "1102001", logical_name: "os_cpu", columns: cpuColumns.map((name) => ({ name })) } },
    row("1102001", "cpu-all", [String(at), -1, 20, 5, 10, 50, 5, 2, 3, 5, 0], at),
    row("1102001", "cpu-0", [String(at), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    row("1102001", "cpu-1", [String(at), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    { record: "layout", rates: [], layout: { type_id: "1104001", logical_name: "os_meminfo", columns: memoryColumns.map((name) => ({ name })) } },
    row("1104001", "memory", [String(at), 512, 1024, 128, 128, 64, 256, 64, 32, 64, 128], at),
    { record: "layout", rates: diskRates, layout: { type_id: "1108001", logical_name: "os_diskstats", columns: diskColumns.map((name) => ({ name })) } },
    row("1108001", "disk-8-0", [String(at), 8, 0, "sda", 4, 2, 16, 24, 20, 14, 1, 300, 600, 0], at),
    row("1108001", "disk-8-1", [String(at), 8, 1, "sdb", 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
  ]
}

function productionSystemHistoryRecords(url) {
  const section = url.searchParams.get("section")
  const before = AT - 5_000_000
  if (section === "os_cpu") {
    const columns = ["ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal", "scope"]
    return [
      { record: "series_segment", segment: { id: SEGMENT } },
      { record: "layout", rates: [], layout: { type_id: "1102001", logical_name: section, columns: columns.map((name) => ({ name })) } },
      row("1102001", "cpu-all-before", [String(before), -1, 100, 20, 50, 500, 10, 5, 5, 10, 0], before),
      row("1102001", "cpu-0-before", [String(before), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], before),
      row("1102001", "cpu-1-before", [String(before), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], before),
      row("1102001", "cpu-all-current", [String(AT), -1, 120, 25, 60, 550, 15, 7, 8, 15, 0], AT),
      row("1102001", "cpu-0-current", [String(AT), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], AT),
      row("1102001", "cpu-1-current", [String(AT), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], AT),
    ]
  }
  if (section === "os_meminfo") {
    const columns = ["ts", "mem_available", "mem_total", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim", "swap_free", "swap_total"]
    const values = [512, 1024, 128, 128, 64, 256, 64, 32, 64, 128]
    return [
      { record: "series_segment", segment: { id: SEGMENT } },
      { record: "layout", rates: [], layout: { type_id: "1104001", logical_name: section, columns: columns.map((name) => ({ name })) } },
      row("1104001", "memory-before", [String(before), ...values], before),
      row("1104001", "memory-current", [String(AT), ...values], AT),
    ]
  }
  if (section === "os_diskstats") {
    const columns = ["ts", "major", "minor", "device", "reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_in_progress", "io_time_ms", "io_weighted_time_ms", "scope"]
    const major = Number(url.searchParams.get("where.major") ?? 8)
    const minor = Number(url.searchParams.get("where.minor") ?? 0)
    const current = minor === 0
      ? [String(AT), major, minor, "sda", 104, 202, 1016, 2024, 1020, 514, 1, 1300, 2600, 0]
      : [String(AT), major, minor, "sdb", 100, 200, 1000, 2000, 1000, 500, 0, 1000, 2000, 0]
    return [
      { record: "series_segment", segment: { id: SEGMENT } },
      { record: "layout", rates: [], layout: { type_id: "1108001", logical_name: section, columns: columns.map((name) => ({ name })) } },
      row("1108001", "disk-before", [String(before), major, minor, minor === 0 ? "sda" : "sdb", 100, 200, 1000, 2000, 1000, 500, 0, 1000, 2000, 0], before),
      row("1108001", "disk-current", current, AT),
    ]
  }
  return []
}

async function hoverContractChart(cdp, testId) {
  await cdp.evaluate(`(() => {
    const plot = document.querySelector('[data-testid="${testId}"] .u-over')
    const bounds = plot.getBoundingClientRect()
    const clientX = bounds.right - 1
    const clientY = bounds.top + bounds.height / 2
    plot.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, clientX, clientY }))
    plot.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX, clientY }))
  })()`)
  await cdp.waitFor(`document.querySelector('[data-testid="${testId}"] .chart-tooltip') !== null`, `${testId} tooltip`)
}

function processSummaryRecords(hour, count, processes) {
  const fields = [
    "processes", "threads", "runnable", "postgresql", "user_cores", "system_cores", "run_delay_ms_per_second", "context_switches_per_second",
    "resident_kib", "virtual_kib", "swap_kib", "major_faults_per_second", "read_bytes_per_second", "write_bytes_per_second", "read_calls_per_second", "write_calls_per_second",
  ]
  const values = [processes, processes * 2, 2, 1, 1.5, 0.5, 12, 33, 1024, 2048, 0, 1, 4096, 8192, 3, 4]
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    layout("0", "os_process_summary", fields),
    ...Array.from({ length: count }, (_, index) => ({
      record: "row", type_id: "0", ordinal: String(index), timestamp: String(hour + Math.min(3_590_000_000, (index + 1) * Math.max(5_000_000, Math.floor(3_590_000_000 / (count + 1))))), values,
    })),
  ]
}

function viewportActivityTimeline() {
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    { record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1), source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }] },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AFTER_AT),
      sections: [{ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "120", bytes: "4096" }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(AT), identity: {}, value: 81 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(AT), identity: {}, value: 76 },
    { record: "lane", segment_id: SEGMENT, lane: "pg_waiting", ts: String(AT), value: 4 },
  ]
}

function viewportActivityRows(url) {
  const selected = url.searchParams.get("where.pid")
  const [activityLayout, template] = activityFixtureSeed()
  const rows = Array.from({ length: 120 }, (_, index) => viewportActivityRow(template, 3_000 + index, AT, index))
    .filter((record) => selected === null || String(record.values[1]) === selected)
  return [activityLayout, ...rows]
}

function viewportActivityHistory(url) {
  const pid = Number(url.searchParams.get("where.pid") ?? "3000")
  const [activityLayout, template] = activityFixtureSeed()
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    activityLayout,
    ...Array.from({ length: 24 }, (_, index) => {
      const timestamp = HOUR + (index + 1) * 120_000_000
      return viewportActivityRow(template, pid, timestamp, index)
    }),
  ]
}

function activityFixtureSeed() {
  const records = snapshotRecords().filter((record) => (record.record === "layout" ? record.layout.type_id : record.type_id) === "1001004")
  return [records.find(({ record }) => record === "layout"), records.find(({ record }) => record === "row")]
}

function viewportActivityRow(template, pid, timestamp, index) {
  const query = `select pg_sleep(0.01), payload from operator_activity where pid = ${pid} and payload = '${"activity-detail-".repeat(24)}'`
  const values = [
    String(timestamp), pid, 1, 20, "operator_database", "operator_role", "viewport-regression", "192.0.2.42", "client backend",
    "active", "Lock", "transactionid", query, String(9_000_000 + pid), 42 + index, 21 + index,
    String(timestamp - 3_000_000_000), String(timestamp - 900_000_000), String(timestamp - 180_000_000), String(timestamp - 60_000_000),
  ]
  return { ...template, ordinal: String(pid), timestamp: String(timestamp), values }
}

function viewportTableGeometry() {
  return `(() => {
    const layout = document.querySelector('[data-testid="pg-entity-layout"]')
    const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll')
    const visible = [...document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row')].find((row) => {
      const bounds = row.getBoundingClientRect()
      const viewport = scroll.getBoundingClientRect()
      return bounds.top >= viewport.top && bounds.bottom <= viewport.bottom
    })
    const rect = (node) => { const bounds = node.getBoundingClientRect(); return { bottom: bounds.bottom, height: bounds.height, left: bounds.left, right: bounds.right, top: bounds.top, width: bounds.width } }
    const style = getComputedStyle(scroll)
    return {
      document: { clientHeight: document.documentElement.clientHeight, scrollHeight: document.documentElement.scrollHeight },
      layout: rect(layout),
      table: { ...rect(scroll), clientHeight: scroll.clientHeight, clientWidth: scroll.clientWidth, overflowY: style.overflowY, railHeight: scroll.offsetHeight - scroll.clientHeight, scrollAxis: scroll.dataset.scrollAxis, scrollHeight: scroll.scrollHeight, scrollLeft: scroll.scrollLeft, scrollTop: scroll.scrollTop, scrollWidth: scroll.scrollWidth, vertical: scroll.scrollHeight > scroll.clientHeight + 1 },
      visiblePid: visible?.querySelector('[role="cell"]')?.textContent.trim() ?? null,
    }
  })()`
}

function viewportDockGeometry() {
  return `(() => {
    const base = ${viewportTableGeometry()}
    const dock = document.querySelector('[data-testid="inspector"]')
    const body = dock.querySelector('.inspector-body')
    const header = dock.querySelector('.inspector-head')
    const close = dock.querySelector('.inspector-close')
    const chart = body.querySelector('.uplot-figure')
    const zero = { bottom: 0, height: 0, left: 0, right: 0, top: 0, width: 0 }
    const lastDetail = body.querySelector('[data-testid="pg-detail"] dl > div:last-child')
    const rect = (node) => { const bounds = node.getBoundingClientRect(); return { bottom: bounds.bottom, height: bounds.height, left: bounds.left, right: bounds.right, top: bounds.top, width: bounds.width } }
    return {
      ...base,
      body: { ...rect(body), clientHeight: body.clientHeight, scrollHeight: body.scrollHeight, scrollTop: body.scrollTop },
      chart: chart === null ? zero : rect(chart),
      close: rect(close),
      dock: rect(dock),
      header: rect(header),
      lastDetail: rect(lastDetail),
      selectedPid: document.querySelector('[data-testid="pg-activity-table"] .entity-row[aria-selected="true"] [role="cell"]')?.textContent.trim() ?? null,
    }
  })()`
}

function sparsePostgresGeometry() {
  return `(() => {
    const activity = document.querySelector('[data-testid="pg-entity-layout"]')
    const activityScroll = activity.querySelector('.entity-scroll')
    const progress = document.querySelector('[data-pg-section="pg_stat_progress_vacuum"]')
    const progressScroll = progress.querySelector('.entity-scroll')
    const workspace = document.querySelector('.workspace')
    const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
    const measured = (root, scroll) => {
      const scrollBox = rect(scroll)
      const style = getComputedStyle(scroll)
      const rows = [...root.querySelectorAll('.entity-row')].map(rect)
      return {
        ...rect(root),
        allRowsFit: rows.every((row) => row.top >= scrollBox.top - .5 && row.bottom <= scrollBox.top + scroll.clientHeight + .5),
        clientHeight: scroll.clientHeight,
        contentSized: root.dataset.contentSized === 'true',
        horizontal: scroll.scrollWidth > scroll.clientWidth,
        overflowX: style.overflowX,
        overflowY: style.overflowY,
        rows,
        scrollBox,
        scrollAxis: scroll.dataset.scrollAxis,
        scrollHeight: scrollBox.height,
        scrollContentHeight: scroll.scrollHeight,
        scrollTop: scroll.scrollTop,
        vertical: /(auto|scroll)/.test(style.overflowY) && scroll.scrollHeight > scroll.clientHeight + 1,
      }
    }
    const activityRect = measured(activity, activityScroll)
    const progressRect = measured(progress, progressScroll)
    return {
      activity: activityRect,
      gap: progressRect.top - activityRect.bottom,
      progress: progressRect,
      workspace: rect(workspace),
    }
  })()`
}

function sparsePostgresSeamGeometry() {
  return `(() => {
    const table = document.querySelector('[data-testid="pg-activity-table"]')
    const scroll = table.querySelector('.entity-scroll')
    const header = table.querySelector('.entity-head [role="columnheader"]:last-child')
    const cell = table.querySelector('.entity-row [role="cell"]:last-child')
    const splitter = document.querySelector('.inspector-splitter')
    const trigger = document.querySelector('.timeline-open-chart')
    const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
    const scrollRect = rect(scroll)
    const splitterRect = rect(splitter)
    const clientRight = scrollRect.left + scroll.clientLeft + scroll.clientWidth
    const style = getComputedStyle(scroll)
    const rows = [...table.querySelectorAll('.entity-row')].map(rect)
    return {
      allRowsFit: rows.every((row) => row.top >= scrollRect.top - .5 && row.bottom <= scrollRect.top + scroll.clientHeight + .5),
      axis: scroll.dataset.scrollAxis,
      cellEndGap: clientRight - rect(cell).right,
      chartToSplitter: splitterRect.left - rect(trigger).right,
      clientHeight: scroll.clientHeight,
      clientRight,
      headerEndGap: clientRight - rect(header).right,
      horizontal: scroll.scrollWidth > scroll.clientWidth + 1,
      overflowX: style.overflowX,
      overflowY: style.overflowY,
      panel: new URL(location.href).searchParams.get('panel'),
      scroll: scrollRect,
      scrollHeight: scroll.scrollHeight,
      scrollLeft: scroll.scrollLeft,
      scrollWidth: scroll.scrollWidth,
      seamGap: splitterRect.left - scrollRect.right,
      splitter: splitterRect,
      vertical: /(auto|scroll)/.test(style.overflowY) && scroll.scrollHeight > scroll.clientHeight + 1,
    }
  })()`
}

function sourceTimelineRecords(historical) {
  const sections = [{ logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001", implementation: "linux", source_family: "system", rows: "1", bytes: "128" }]
  if (historical) sections.push({ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256" })
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    { record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1), source_families: [{ name: "postgresql", configured: false, present: historical, metrics_present: historical }] },
    { record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AFTER_AT), sections },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "overall_health", ts: String(AT), identity: {}, value: 73 },
    { record: "point", type_id: "0", series: "os_health", ts: String(AT), identity: {}, value: 73 },
    ...systemIndexRecords(String(AT)),
  ]
}

function slowQueryTimelineRecords() {
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    {
      record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AFTER_AT),
      sections: [{
        logical_name: "pg_log_slow_queries", physical_name: "pg_log_slow_queries", type_id: "2004001",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "512",
      }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    {
      record: "finding", logical_name: "pg_log_slow_queries", kind: "known_bad", type_id: "2004001",
      field_ordinal: 4, row_ordinal: "3", ts: String(AT),
    },
  ]
}

function slowQueryRecords() {
  const columns = ["ts", "pattern", "sample", "count", "max_duration_ms", "total_duration_ms"]
  return [
    layout("2004001", "pg_log_slow_queries", columns),
    row("2004001", "3", [String(AT), SLOW_PATTERN, SLOW_QUERY, 3, 6_290, 12_580]),
  ]
}

function detailGeometryExpression() {
  return `(() => {
    const rows = [...document.querySelectorAll('[data-testid="event-detail"] dl > div')]
    const byLabel = (text) => rows.find((row) => row.querySelector("dt")?.textContent.trim().toLocaleUpperCase("ru-RU") === text)
    const bounds = (node) => {
      const rect = node.getBoundingClientRect()
      return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width }
    }
    const measured = (text) => {
      const row = byLabel(text)
      const label = row.querySelector("dt")
      const output = row.querySelector("dd")
      const range = document.createRange()
      range.selectNodeContents(label)
      const lines = new Set([...range.getClientRects()].filter((rect) => rect.width > 0).map((rect) => Math.round(rect.top * 10) / 10)).size
      const style = getComputedStyle(output)
      return {
        label: { ...bounds(label), lines },
        row: bounds(row),
        value: {
          ...bounds(output),
          clientWidth: output.clientWidth,
          lineHeight: Number.parseFloat(style.lineHeight),
          minWidth: style.minWidth,
          scrollWidth: output.scrollWidth,
        },
      }
    }
    const numeric = ["REPEATS", "MAX DURATION", "TOTAL DURATION"].map((text) => {
      const row = byLabel(text)
      const output = row.querySelector("dd")
      const rect = output.getBoundingClientRect()
      return { align: getComputedStyle(output).textAlign, height: row.getBoundingClientRect().height, right: rect.right, text: output.textContent.trim() }
    })
    return {
      chart: {
        current: document.querySelector('[data-testid="event-detail"] .chart-current')?.textContent.trim() ?? "",
        label: document.querySelector('[data-testid="event-detail"] .uplot-host')?.getAttribute("aria-label") ?? "",
      },
      clientWidth: document.documentElement.clientWidth,
      innerWidth: window.innerWidth,
      labels: rows.map((row) => row.querySelector("dt")?.textContent.trim() ?? ""),
      list: bounds(document.querySelector('[data-testid="event-detail"] dl')),
      numeric,
      pattern: measured("PATTERN"),
      sample: measured("SAMPLE"),
      scrollWidth: document.documentElement.scrollWidth,
      text: document.querySelector('[data-testid="event-detail"]')?.textContent ?? "",
    }
  })()`
}

async function settleLayout(cdp) {
  await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
}

async function assertCompactTimelineContained(cdp, followingSelector, label) {
  for (const width of [800, 1280]) {
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
    await settleLayout(cdp)
    const pointer = await cdp.evaluate(`(() => {
      const plot = document.querySelector('[data-testid="hour-timeline"] .u-over').getBoundingClientRect()
      return { x: plot.right - 1, y: plot.top + plot.height / 2 }
    })()`)
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...pointer })
    const geometry = await cdp.evaluate(`(() => {
      const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top } }
      const figure = document.querySelector('[data-testid="hour-timeline"]')
      const shell = figure.closest('.timeline-shell')
      const plot = figure.querySelector('.u-over')
      const cursor = figure.querySelector('.u-cursor-x')
      return {
        axes: [...figure.querySelectorAll('.u-axis')].map(bounds),
        cursor: cursor === null ? null : bounds(cursor),
        figure: { ...bounds(figure), height: figure.getBoundingClientRect().height },
        following: bounds(document.querySelector(${JSON.stringify(followingSelector)})),
        plot: bounds(plot),
        rightReserve: figure.getBoundingClientRect().right - plot.getBoundingClientRect().right,
        shell: bounds(shell),
      }
    })()`)
    assert.ok(geometry.figure.height >= 92 && geometry.figure.height <= 96, `${label} ${width}px compact figure: ${JSON.stringify(geometry)}`)
    for (const axis of geometry.axes) {
      assert.ok(axis.left >= geometry.figure.left - 1 && axis.right <= geometry.figure.right + 1
        && axis.top >= geometry.figure.top - 1 && axis.bottom <= geometry.figure.bottom + 1,
      `${label} ${width}px axis containment: ${JSON.stringify(geometry)}`)
    }
    assert.ok(geometry.cursor !== null && geometry.cursor.left >= geometry.figure.left - 1
      && geometry.cursor.right <= geometry.figure.right + 1 && geometry.cursor.bottom <= geometry.figure.bottom + 1,
    `${label} ${width}px cursor containment: ${JSON.stringify(geometry)}`)
    assert.ok(geometry.rightReserve >= 28, `${label} ${width}px final-label reserve: ${JSON.stringify(geometry)}`)
    assert.ok(geometry.following.top >= geometry.shell.bottom - 1, `${label} ${width}px following-region overlap: ${JSON.stringify(geometry)}`)
  }
}

async function assertDetailRowsDoNotOverlap(cdp, label) {
  for (const locale of ["ru", "en"]) {
    await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${label} ${locale} locale`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
      await settleLayout(cdp)
      const geometry = await cdp.evaluate(`(() => {
        const detail = document.querySelector('[data-testid="pg-detail"]')
        const box = detail.getBoundingClientRect()
        const rows = [...detail.querySelectorAll('dl > .detail-row')].map((row) => {
          const term = row.querySelector('dt').getBoundingClientRect()
          const value = row.querySelector('dd').getBoundingClientRect()
          const horizontal = Math.min(term.right, value.right) - Math.max(term.left, value.left)
          const vertical = Math.min(term.bottom, value.bottom) - Math.max(term.top, value.top)
          return { horizontal, term: { left: term.left, right: term.right }, value: { left: value.left, right: value.right }, vertical }
        })
        return { detail: { left: box.left, right: box.right }, rows, scroll: document.documentElement.scrollWidth, viewport: document.documentElement.clientWidth }
      })()`)
      assert.ok(geometry.rows.length > 0, `${label} ${locale} ${width}px rows`)
      assert.equal(geometry.rows.some(({ horizontal, vertical }) => horizontal > 0.5 && vertical > 0.5), false,
        `${label} ${locale} ${width}px overlap: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.rows.every(({ term, value }) => term.left >= geometry.detail.left - 1 && term.right <= geometry.detail.right + 1
        && value.left >= geometry.detail.left - 1 && value.right <= geometry.detail.right + 1),
      `${label} ${locale} ${width}px containment: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.scroll <= geometry.viewport, `${label} ${locale} ${width}px document overflow: ${JSON.stringify(geometry)}`)
    }
  }
}

async function assertSearchControlContained(cdp, label, selector = '[data-testid="events-console"] [data-search-surface="events"]') {
  for (const locale of ["ru", "en"]) {
    await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${label} ${locale} locale`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
      await settleLayout(cdp)
      const closed = await cdp.evaluate(`(() => {
        const root = document.querySelector(${JSON.stringify(selector)})
        const parts = [root.querySelector('form'), root.querySelector('input'), root.querySelector('[aria-label="' + (${JSON.stringify(locale)} === 'ru' ? 'Применить поиск' : 'Apply search') + '"]'), root.querySelector('[aria-expanded]')]
        return {
          bounds: parts.map((part) => { const box = part.getBoundingClientRect(); return { left: box.left, right: box.right } }),
          scroll: document.documentElement.scrollWidth,
          viewport: document.documentElement.clientWidth,
        }
      })()`)
      assert.ok(closed.bounds.every(({ left, right }) => left >= -1 && right <= closed.viewport + 1)
        && closed.scroll <= closed.viewport, `${label} ${locale} ${width}px control: ${JSON.stringify(closed)}`)
      await cdp.evaluate(`document.querySelector(${JSON.stringify(selector)}).querySelector('[aria-expanded]').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="search-help"] [role="dialog"]') !== null`, `${label} ${locale} ${width}px help`)
      const opened = await cdp.evaluate(`(() => {
        const dialog = document.querySelector('[data-testid="search-help"] [role="dialog"]')
        const box = dialog.getBoundingClientRect()
        return { active: dialog.contains(document.activeElement), bottom: box.bottom, left: box.left, right: box.right, top: box.top, viewportHeight: innerHeight, viewportWidth: innerWidth }
      })()`)
      assert.ok(opened.active && opened.left >= -1 && opened.right <= opened.viewportWidth + 1
        && opened.top >= -1 && opened.bottom <= opened.viewportHeight + 1,
      `${label} ${locale} ${width}px help containment: ${JSON.stringify(opened)}`)
      await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
      await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
      await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, `${label} ${locale} ${width}px help close`)
    }
  }
}

async function assertSearchChipHierarchy(cdp, label) {
  const geometry = await cdp.evaluate(`(() => {
    const root = document.querySelector('[data-testid="search-chips"]')
    const predicates = [...root.querySelectorAll('[data-search-predicate]')]
    const syntax = [...root.querySelectorAll('[data-search-syntax]')]
    const bounds = (node) => {
      const rect = node.getBoundingClientRect()
      return { bottom: rect.bottom, height: rect.height, top: rect.top }
    }
    const predicateStyle = getComputedStyle(predicates[0])
    const syntaxStyles = syntax.map((token) => getComputedStyle(token))
    const children = [...root.children]
    const lineCenters = []
    for (const child of children) {
      const rect = child.getBoundingClientRect()
      const center = rect.top + rect.height / 2
      if (!lineCenters.some((candidate) => Math.abs(candidate - center) <= 1)) lineCenters.push(center)
    }
    const predicateHeight = predicates[0].getBoundingClientRect().height
    const rowGap = Number.parseFloat(getComputedStyle(root).rowGap)
    return {
      allChildrenAreElements: [...root.childNodes].every((node) => node.nodeType === Node.ELEMENT_NODE),
      childHeights: children.map((child) => child.getBoundingClientRect().height),
      lineCount: lineCenters.length,
      predicate: {
        color: predicateStyle.color,
        fontSize: Number.parseFloat(predicateStyle.fontSize),
        height: predicateHeight,
      },
      rootHeight: root.getBoundingClientRect().height,
      rowGap,
      syntax: syntax.map((token, index) => ({
        background: syntaxStyles[index].backgroundColor,
        border: [syntaxStyles[index].borderTopWidth, syntaxStyles[index].borderRightWidth, syntaxStyles[index].borderBottomWidth, syntaxStyles[index].borderLeftWidth],
        color: syntaxStyles[index].color,
        fontSize: Number.parseFloat(syntaxStyles[index].fontSize),
        geometry: bounds(token),
        kind: token.getAttribute("data-search-syntax"),
        text: token.textContent,
      })),
      text: root.textContent,
    }
  })()`)
  assert.equal(geometry.allChildrenAreElements, true, `${label}: ${JSON.stringify(geometry)}`)
  assert.match(geometry.text, /^\(tablespace: fast_ssdORtablespace: archive\)AND\(.+OR.+\)$/)
  assert.deepEqual(geometry.syntax.map(({ text }) => text), ["(", "OR", ")", "AND", "(", "OR", ")"], `${label}: ${JSON.stringify(geometry)}`)
  assert.deepEqual(geometry.syntax.map(({ kind }) => kind), ["parenthesis", "connector", "parenthesis", "connector", "parenthesis", "connector", "parenthesis"], `${label}: ${JSON.stringify(geometry)}`)
  assert.equal(geometry.predicate.fontSize, 11, `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ fontSize }) => fontSize < geometry.predicate.fontSize), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ color }) => color !== geometry.predicate.color), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ background, border }) => background === "rgba(0, 0, 0, 0)" && border.every((width) => width === "0px")), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ geometry: token }) => Math.abs(token.height - geometry.predicate.height) <= 0.5), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.childHeights.every((height) => height <= geometry.predicate.height + 0.5), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.rootHeight <= geometry.lineCount * geometry.predicate.height + Math.max(geometry.lineCount - 1, 0) * geometry.rowGap + 1, `${label}: ${JSON.stringify(geometry)}`)
}

async function assertSearchChipHierarchyMatrix(cdp, label) {
  for (const locale of ["ru", "en"]) {
    await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${label} ${locale} locale`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: width === 360 ? 640 : 768, mobile: false, width })
      await settleLayout(cdp)
      await assertSearchChipHierarchy(cdp, `${label} ${locale} ${width}px`)
    }
  }
}

test("forensic workstation keeps exact preview and one responsive Inspector", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const [activityLayout, activityTemplate] = activityFixtureSeed()
  const forensicSnapshots = [
    ...snapshotRecords().filter((record) => (record.record === "layout" ? record.layout.type_id : record.type_id) !== "1001004"),
    activityLayout,
    ...Array.from({ length: 12 }, (_, index) => viewportActivityRow(activityTemplate, 3_000 + index, AT, index)),
    ...progressVacuumRecords(),
    ...statementRecords(false),
  ]
  const forensicTimeline = [
    ...timelineRecords(HOUR, true).map((record) => record.record !== "finished_segment" ? record : {
      ...record,
      sections: [...record.sections, {
        logical_name: "pg_stat_progress_vacuum", physical_name: "pg_stat_progress_vacuum", type_id: "1012003",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }],
    }),
    { record: "lane", segment_id: SEGMENT, lane: "cpu_busy", ts: String(AT), value: 54 },
    { record: "lane", segment_id: SEGMENT, lane: "pg_running", ts: String(AT), value: 3 },
    { record: "lane", segment_id: SEGMENT, lane: "pg_waiting", ts: String(AT), value: 1 },
  ]
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const section = url.searchParams.get("section")
      if (section === "os_process_summary") return ndjson(response, processSummaryRecords(HOUR, 3, 80))
      if (section === "os_process") return ndjson(response, forensicSnapshots)
      return ndjson(response, section === null ? [...forensicTimeline, {
        record: "finding", logical_name: "pg_log_errors", kind: "event", type_id: "1009001",
        field_ordinal: 1, row_ordinal: "1", ts: String(AFTER_AT),
      }] : [])
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, forensicSnapshots)
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("forensic workstation server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    const viewports = [
      { height: 800, kind: "desktop", mobile: false, width: 1280 },
      { height: 900, kind: "tablet", mobile: false, width: 800 },
      { height: 800, kind: "phone", mobile: true, width: 360 },
    ]
    for (const viewport of viewports) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: viewport.height, mobile: viewport.mobile, width: viewport.width })
      await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=cpu` })
      await cdp.waitFor(`document.querySelector('.process-summary-inline > div:first-child strong')?.textContent === "1.5"`, `${viewport.kind} process summary`, 15_000)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] canvas') !== null && document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, `${viewport.kind} workstation`)
      await settleLayout(cdp)
      const closed = await cdp.evaluate(`(() => {
        const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width } }
        const topbar = document.querySelector('.topbar')
        const top = bounds(topbar)
        const painted = [...topbar.children].filter((node) => getComputedStyle(node).display !== 'none').map(bounds)
        const scroll = document.querySelector('[data-testid="process-table"] .entity-scroll')
        const scrollRect = scroll.getBoundingClientRect()
        const visibleRows = [...document.querySelectorAll('[data-testid="process-table"] .entity-row')].filter((row) => {
          const rect = row.getBoundingClientRect()
          return rect.bottom > scrollRect.top && rect.top < scrollRect.bottom
        }).length
        return {
          documentOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
          inspector: document.querySelector('[data-testid="inspector"]') !== null,
          painted,
          paintedInside: painted.every((rect) => rect.left >= top.left - .5 && rect.right <= top.right + .5 && rect.top >= top.top - .5 && rect.bottom <= top.bottom + .5),
          preview: bounds(document.querySelector('.timeline-preview')),
          scroll: bounds(scroll),
          top,
          visibleRows,
        }
      })()`)
      assert.equal(closed.inspector, false, viewport.kind)
      assert.equal(closed.documentOverflow, false, `${viewport.kind}: ${JSON.stringify(closed)}`)
      assert.equal(closed.paintedInside, true, `${viewport.kind}: ${JSON.stringify(closed)}`)
      assert.ok(Math.abs(closed.preview.height - 124) <= .5, `${viewport.kind}: ${JSON.stringify(closed.preview)}`)

      await cdp.evaluate(`document.querySelector('[data-testid="process-table"] .entity-row').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="inspector"][data-panel="detail"]') !== null`, `${viewport.kind} Detail Inspector`)
      await settleLayout(cdp)
      const opened = await cdp.evaluate(`(() => {
        const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width } }
        const inspector = bounds(document.querySelector('[data-testid="inspector"]'))
        const scroll = document.querySelector('[data-testid="process-table"] .entity-scroll')
        const scrollRect = scroll.getBoundingClientRect()
        const visibleRows = [...document.querySelectorAll('[data-testid="process-table"] .entity-row')].filter((row) => {
          const rect = row.getBoundingClientRect()
          return rect.bottom > scrollRect.top && rect.top < scrollRect.bottom
        }).length
        const query = new URL(location.href).searchParams
        return { body: bounds(document.querySelector('.inspector-body')), inspector, panel: query.get('panel'), row: query.get('row'), scroll: bounds(scroll), visibleRows }
      })()`)
      assert.notEqual(opened.row, null, viewport.kind)
      assert.equal(opened.panel, null, viewport.kind)
      if (viewport.kind === "desktop") {
        assert.ok(Math.abs(opened.inspector.width - 360) <= 1, JSON.stringify(opened))
        assert.ok(Math.abs(opened.scroll.height - closed.scroll.height) <= 1, JSON.stringify({ closed, opened }))
        assert.equal(opened.visibleRows, closed.visibleRows, JSON.stringify({ closed, opened }))
      } else if (viewport.kind === "tablet") {
        assert.ok(Math.abs(opened.inspector.width - 384) <= 1, JSON.stringify(opened))
        assert.ok(Math.abs(opened.scroll.width - closed.scroll.width) <= 1, JSON.stringify({ closed, opened }))
      } else {
        assert.ok(Math.abs(opened.inspector.width - viewport.width) <= 1, JSON.stringify(opened))
        assert.ok(opened.inspector.height <= 480.5, JSON.stringify(opened))
        assert.ok(opened.inspector.height >= 300 && opened.body.height >= 250, JSON.stringify(opened))
      }
      await cdp.evaluate(`([...document.querySelectorAll('.inspector-tabs button')].find((button) => button.textContent === 'Chart')).click()`)
      await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === 'chart' && document.querySelector('[data-testid="inspector-chart"] .inspector-chart-slot [data-testid="process-history"]') !== null`, `${viewport.kind} entity Chart Inspector`)
      await settleLayout(cdp)
      assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') === null`), true, `${viewport.kind} entity chart replaces the shared timeline`)
      assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('.timeline-preview').getBoundingClientRect().height`) - 124) <= .5, `${viewport.kind} preview keeps its figure beside the entity chart`)
      await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="inspector"]') === null`, `${viewport.kind} Inspector closed`)
      await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
      await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === 'chart' && document.querySelector('[data-testid="inspector-chart"] canvas') !== null`, `${viewport.kind} Chart Inspector`)
      await settleLayout(cdp)
      const chartGeometry = await cdp.evaluate(`(() => {
        const dock = document.querySelector('[data-testid="inspector"]')
        const body = dock.querySelector('.inspector-body')
        const header = dock.querySelector('.inspector-head')
        const title = header.querySelector('strong')
        const rail = body.querySelector('.timeline-rail')
        const picker = body.querySelector('.timeline-metric-picker')
        const select = body.querySelector('[data-testid="timeline-metric-select"]')
        const figure = body.querySelector('.uplot-figure')
        const preview = document.querySelector('.timeline-preview')
        const previewRail = preview.querySelector('.timeline-rail')
        const previewLanes = preview.querySelector('.timeline-lanes')
        const previewPicker = preview.querySelector('.timeline-preview-picker')
        const previewTrigger = preview.querySelector('.timeline-open-chart')
        const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
        const bodyRect = rect(body)
        const headerRect = rect(header)
        const next = [...select.options].find((option) => option.value !== select.value)?.value ?? select.value
        select.value = next
        select.dispatchEvent(new Event('change', { bubbles: true }))
        return {
          body: { ...bodyRect, clientWidth: body.clientWidth, overflowX: getComputedStyle(body).overflowX, scrollbarGutter: getComputedStyle(body).scrollbarGutter, scrollWidth: body.scrollWidth },
          figure: rect(figure),
          header: headerRect,
          headerChildrenInside: [...header.children].every((child) => { const box = child.getBoundingClientRect(); return box.left >= headerRect.left - .5 && box.right <= headerRect.right + .5 && box.top >= headerRect.top - .5 && box.bottom <= headerRect.bottom + .5 }),
          laneButtons: body.querySelectorAll('.lane-label').length,
          optionCount: select.options.length,
          picker: rect(picker),
          preview: {
            ...rect(preview),
            action: rect(previewTrigger),
            actionAtEdge: Math.abs(previewTrigger.getBoundingClientRect().right - previewRail.getBoundingClientRect().right) <= .5,
            labelsInside: getComputedStyle(previewLanes).display === 'none' || [...previewLanes.children].every((child) => { const box = child.getBoundingClientRect(); const parent = previewLanes.getBoundingClientRect(); return box.left >= parent.left - .5 && box.right <= parent.right + .5 }),
            lanesDisplay: getComputedStyle(previewLanes).display,
            ownerCount: [preview, ...preview.querySelectorAll('*')].filter((node) => /(auto|scroll)/.test(getComputedStyle(node).overflowX) && node.scrollWidth > node.clientWidth + 1).length,
            pickerDisplay: getComputedStyle(previewPicker).display,
            selectedAccess: previewLanes.querySelector('[data-primary="true"]')?.getAttribute('title') ?? previewPicker.querySelector('[data-testid="timeline-preview-reading"]')?.getAttribute('title'),
          },
          rail: rect(rail),
          selected: next,
          titleFits: title.scrollWidth <= title.clientWidth + 1 && title.scrollHeight <= title.clientHeight + 1,
        }
      })()`)
      await cdp.waitFor(`document.querySelector('[data-testid="timeline-metric-select"]').value === ${JSON.stringify(chartGeometry.selected)}`, `${viewport.kind} metric selection`)
      assert.equal(chartGeometry.body.overflowX, "hidden", `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.match(chartGeometry.body.scrollbarGutter, /stable/, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.body.scrollWidth <= chartGeometry.body.clientWidth + 1, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.equal(chartGeometry.headerChildrenInside, true, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.equal(chartGeometry.titleFits, true, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.optionCount > 1, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.equal(chartGeometry.laneButtons, 0, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.picker.left >= chartGeometry.body.left - .5 && chartGeometry.picker.right <= chartGeometry.body.left + chartGeometry.body.clientWidth + .5, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.figure.right <= chartGeometry.body.left + chartGeometry.body.clientWidth + .5, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(Math.abs(chartGeometry.preview.height - 30) <= .5, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.equal(chartGeometry.preview.ownerCount, 0, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.equal(chartGeometry.preview.actionAtEdge, true, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.equal(chartGeometry.preview.labelsInside, true, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.match(chartGeometry.preview.selectedAccess, /\S/)
      if (viewport.kind === "phone") {
        assert.equal(chartGeometry.preview.lanesDisplay, "none", JSON.stringify(chartGeometry.preview))
        assert.equal(chartGeometry.preview.pickerDisplay, "grid", JSON.stringify(chartGeometry.preview))
        assert.ok(Math.abs(chartGeometry.preview.action.width - 36) <= 1, JSON.stringify(chartGeometry.preview))
      } else {
        assert.equal(chartGeometry.preview.lanesDisplay, "flex", JSON.stringify(chartGeometry.preview))
        assert.equal(chartGeometry.preview.pickerDisplay, "none", JSON.stringify(chartGeometry.preview))
        assert.ok(Math.abs(chartGeometry.preview.action.width - 64) <= 1, JSON.stringify(chartGeometry.preview))
      }
      const legend = await cdp.evaluate(`(() => {
        const figure = document.querySelector('[data-testid="inspector-chart"] .uplot-figure')
        const labels = figure.querySelector('.chart-series-labels')
        const current = figure.querySelector('.chart-current')
        const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
        const labelRect = rect(labels)
        const currentRect = rect(current)
        return {
          childrenInside: [...labels.children].every((child) => { const box = child.getBoundingClientRect(); return box.left >= labelRect.left - .5 && box.right <= labelRect.right + .5 }),
          current: currentRect,
          currentInside: currentRect.left >= rect(figure).left - .5 && currentRect.right <= rect(figure).right + .5,
          labels: labelRect,
          overflowX: getComputedStyle(labels).overflowX,
          rowsSeparated: labelRect.bottom <= currentRect.top + .5,
          scrollWidth: labels.scrollWidth,
          clientWidth: labels.clientWidth,
        }
      })()`)
      assert.equal(legend.overflowX, "hidden", `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.ok(legend.scrollWidth <= legend.clientWidth + 1, `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.equal(legend.childrenInside, true, `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.equal(legend.currentInside, true, `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.equal(legend.rowsSeparated, true, `${viewport.kind}: ${JSON.stringify(legend)}`)
      const fixedHeader = await cdp.evaluate(`(() => { const body = document.querySelector('.inspector-body'); const header = document.querySelector('.inspector-head'); const before = header.getBoundingClientRect().top; body.scrollTop = body.scrollHeight; return { after: header.getBoundingClientRect().top, before } })()`)
      assert.ok(Math.abs(fixedHeader.after - fixedHeader.before) <= .5, `${viewport.kind}: ${JSON.stringify(fixedHeader)}`)
      await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="inspector"]') === null && new URL(location.href).searchParams.get('row') === null && new URL(location.href).searchParams.get('panel') === null`, `${viewport.kind} closed Inspector`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity&panel=chart` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]') !== null && document.querySelector('[data-testid="inspector-chart"] canvas') !== null`, "exact adjacent PostgreSQL Chart state")
    await cdp.evaluate(`(() => {
      const style = document.createElement('style')
      style.textContent = '#classic-scrollbar-sentinel::-webkit-scrollbar,.timeline-lanes::-webkit-scrollbar{height:15px;width:15px}'
      document.head.append(style)
      const sentinel = document.createElement('div')
      sentinel.id = 'classic-scrollbar-sentinel'
      sentinel.style.cssText = 'position:fixed;left:-1000px;top:0;width:40px;height:30px;overflow:scroll'
      sentinel.innerHTML = '<div style="width:80px;height:1px"></div>'
      document.body.append(sentinel)
    })()`)
    await settleLayout(cdp)
    const classicPreview = await cdp.evaluate(`(() => {
      const preview = document.querySelector('.timeline-preview')
      const rail = preview.querySelector('.timeline-rail')
      const lanes = preview.querySelector('.timeline-lanes')
      const trigger = preview.querySelector('.timeline-open-chart')
      const sentinel = document.querySelector('#classic-scrollbar-sentinel')
      const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
      const laneRect = rect(lanes)
      const railRect = rect(rail)
      return {
        actionAtEdge: Math.abs(rect(trigger).right - railRect.right) <= .5,
        actionWidth: rect(trigger).width,
        classicRail: sentinel.offsetHeight - sentinel.clientHeight,
        height: rect(preview).height,
        labelsInside: [...lanes.children].every((child) => { const box = child.getBoundingClientRect(); return box.left >= laneRect.left - .5 && box.right <= laneRect.right + .5 }),
        laneClientWidth: lanes.clientWidth,
        laneOverflowX: getComputedStyle(lanes).overflowX,
        laneScrollWidth: lanes.scrollWidth,
        ownerCount: [preview, ...preview.querySelectorAll('*')].filter((node) => /(auto|scroll)/.test(getComputedStyle(node).overflowX) && node.scrollWidth > node.clientWidth + 1).length,
        panel: new URL(location.href).searchParams.get('panel'),
        url: location.href,
      }
    })()`)
    assert.equal(classicPreview.classicRail, 15, JSON.stringify(classicPreview))
    assert.equal(classicPreview.panel, "chart", JSON.stringify(classicPreview))
    assert.match(classicPreview.url, /view=pg\.activity/)
    assert.ok(Math.abs(classicPreview.height - 30) <= .5, JSON.stringify(classicPreview))
    assert.equal(classicPreview.laneOverflowX, "hidden", JSON.stringify(classicPreview))
    assert.ok(classicPreview.laneScrollWidth <= classicPreview.laneClientWidth + 1, JSON.stringify(classicPreview))
    assert.equal(classicPreview.ownerCount, 0, JSON.stringify(classicPreview))
    assert.equal(classicPreview.labelsInside, true, JSON.stringify(classicPreview))
    assert.equal(classicPreview.actionAtEdge, true, JSON.stringify(classicPreview))
    assert.ok(Math.abs(classicPreview.actionWidth - 64) <= 1, JSON.stringify(classicPreview))
    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[0].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "Host resource table")
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-group-chart-disk"] .uplot-figure') !== null`, "the inline Disk chart")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector"]') === null`), true)
    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`document.querySelectorAll('.pg-tabs button').length > 1`, "PostgreSQL tabs")
    await cdp.evaluate(`document.querySelectorAll('.pg-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "PostgreSQL Activity table")
    await cdp.waitFor(`document.querySelector('[data-pg-section="pg_stat_progress_vacuum"] .entity-row') !== null`, "PostgreSQL VACUUM progress table")
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'pid:3000')
      input.dispatchEvent(new Event('input', { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1 && document.querySelector('[data-testid="pg-entity-layout"]').dataset.contentSized === 'true'`, "filtered sparse PostgreSQL Activity")
    const sparseBefore = await cdp.evaluate(sparsePostgresGeometry())
    assert.equal(sparseBefore.activity.contentSized, true, JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.progress.contentSized, true, JSON.stringify(sparseBefore))
    assert.ok(sparseBefore.activity.scrollHeight <= 72, JSON.stringify(sparseBefore))
    assert.ok(sparseBefore.progress.scrollHeight <= 72, JSON.stringify(sparseBefore))
    assert.ok(sparseBefore.gap >= 7 && sparseBefore.gap <= 10, JSON.stringify(sparseBefore))
    assert.ok(sparseBefore.progress.bottom < sparseBefore.workspace.bottom - 120, JSON.stringify(sparseBefore))
    assert.ok(sparseBefore.activity.horizontal && sparseBefore.progress.horizontal, JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.scrollAxis, "horizontal", JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.progress.scrollAxis, "horizontal", JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.overflowX, "auto", JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.overflowY, "hidden", JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.vertical, false, JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.progress.vertical, false, JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.allRowsFit, true, JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.progress.allRowsFit, true, JSON.stringify(sparseBefore))
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === 'chart' && document.querySelector('[data-testid="inspector-chart"] canvas') !== null`, "sparse PostgreSQL Chart Inspector")
    await cdp.evaluate(`(() => { const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll'); scroll.scrollLeft = scroll.scrollWidth })()`)
    await settleLayout(cdp)
    const sparseSeam = await cdp.evaluate(sparsePostgresSeamGeometry())
    assert.equal(sparseSeam.panel, "chart", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.axis, "horizontal", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.overflowX, "auto", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.overflowY, "hidden", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.vertical, false, JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.horizontal, true, JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.allRowsFit, true, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.scrollLeft >= sparseSeam.scrollWidth - (sparseSeam.clientRight - sparseSeam.scroll.left) - 1, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.headerEndGap >= 7 && sparseSeam.headerEndGap <= 9, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.cellEndGap >= 7 && sparseSeam.cellEndGap <= 9, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.seamGap >= 6, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.chartToSplitter >= 6, JSON.stringify(sparseSeam))
    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === null && document.querySelector('[data-testid="inspector"]') === null`, "sparse Chart Inspector close")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector-detail"] [data-testid="pg-detail"]') !== null`, "PostgreSQL detail in shared Inspector")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="inspector"]').length === 1 && document.querySelector('.workspace [data-testid="pg-detail"]') === null`), true)
    const sparseOpened = await cdp.evaluate(sparsePostgresGeometry())
    assert.ok(Math.abs(sparseOpened.activity.height - sparseBefore.activity.height) <= 1, JSON.stringify({ sparseBefore, sparseOpened }))
    assert.ok(Math.abs(sparseOpened.progress.top - sparseBefore.progress.top) <= 1, JSON.stringify({ sparseBefore, sparseOpened }))
    await cdp.evaluate(`document.querySelector('.inspector-close').click(); document.querySelectorAll('.source-tabs button')[3].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-item"] button') !== null`, "Events list")
    await cdp.evaluate(`document.querySelector('[data-testid="event-item"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector-detail"] [data-testid="event-detail"]') !== null`, "Event detail in shared Inspector")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="inspector"]').length === 1 && document.querySelector('.workspace [data-testid="event-detail"]') === null`), true)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

async function assertHoverGeometryStable(cdp, selector, label) {
  const expression = (position) => `(() => {
    const figure = document.querySelector(${JSON.stringify(selector)})
    const pick = (node) => { const box = node.getBoundingClientRect(); return [box.left, box.top, box.width, box.height] }
    const plot = figure.querySelector('.u-over')
    const box = plot.getBoundingClientRect()
    return {
      point: { x: box.left + box.width * ${position}, y: box.top + box.height * 0.45 },
      boxes: {
        figure: pick(figure), caption: pick(figure.querySelector('figcaption')),
        host: pick(figure.querySelector('.uplot-host')), plot: pick(plot), canvas: pick(figure.querySelector('canvas')),
      },
    }
  })()`
  const before = await cdp.evaluate(expression(0.25))
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...before.point })
  await cdp.waitFor(`document.querySelector(${JSON.stringify(selector)})?.querySelector('[data-testid="chart-hover-readout"]') !== null`, `${label} hover readout`)
  const middle = await cdp.evaluate(expression(0.55))
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...middle.point })
  const after = await cdp.evaluate(expression(0.8))
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...after.point })
  const moved = await cdp.evaluate(expression(0.8))
  for (const key of Object.keys(before.boxes)) {
    for (const current of [middle.boxes[key], moved.boxes[key]]) {
      before.boxes[key].forEach((value, index) => assert.ok(Math.abs(value - current[index]) <= 0.75, `${label} ${key} moved: ${JSON.stringify({ after: moved.boxes, before: before.boxes })}`))
    }
  }
}

function timelineRecords(hour = HOUR, cgroups = false) {
  const shift = hour - HOUR
  const shifted = (timestamp) => String(timestamp + shift)
  return [
    { record: "hour", from: String(hour), to: String(hour + HOUR_US - 1), available_hours: AVAILABLE_HOURS.map(String) },
    {
      record: "catalog", from: String(hour), to: String(hour + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(hour), max_ts: shifted(AFTER_AT),
      sections: [{
        logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "pg_stat_statements", physical_name: "pg_stat_statements", type_id: "1002003",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "512",
      }, {
        logical_name: "pg_store_plans", physical_name: "pg_store_plans", type_id: "1004001",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "512",
      }, {
        logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "128",
      }, {
        logical_name: "os_process", physical_name: "os_process", type_id: "1100001",
        implementation: "linux", source_family: "system", rows: "80", bytes: "16384",
      }, ...(cgroups ? [{
        logical_name: "instance_metadata", physical_name: "instance_metadata", type_id: "1000001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "64",
      }, {
        logical_name: "os_cgroup_context", physical_name: "os_cgroup_context", type_id: "1205001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "128",
      }, {
        logical_name: "os_cgroup_cpu", physical_name: "os_cgroup_cpu", type_id: "1201001",
        implementation: "linux", source_family: "system", rows: "2", bytes: "256",
      }, {
        logical_name: "os_cgroup_memory", physical_name: "os_cgroup_memory", type_id: "1202001",
        implementation: "linux", source_family: "system", rows: "2", bytes: "256",
      }, {
        logical_name: "os_cgroup_io", physical_name: "os_cgroup_io", type_id: "1203002",
        implementation: "linux", source_family: "system", rows: "4", bytes: "512",
      }] : []), {
        logical_name: "pg_stat_user_tables", physical_name: "pg_stat_user_tables", type_id: "1013005",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "pg_stat_user_indexes", physical_name: "pg_stat_user_indexes", type_id: "1014004",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(QUARTER_PREVIOUS), identity: {}, value: 71 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(QUARTER_PREVIOUS), identity: {}, value: 61 },
    { record: "point", type_id: "0", series: "postgres_health", ts: shifted(QUARTER_PREVIOUS), identity: {}, value: 90 },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(QUARTER_NEXT), identity: {}, value: 73 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(QUARTER_NEXT), identity: {}, value: 63 },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(BEFORE_AT), identity: {}, value: null },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(BEFORE_AT), identity: {}, value: null },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(AT), identity: {}, value: 82 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(AT), identity: {}, value: 46 },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(AFTER_AT), identity: {}, value: 84 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(AFTER_AT), identity: {}, value: 48 },
    { record: "point", type_id: "0", series: "postgres_health", ts: shifted(AT), identity: {}, value: 64 },
    ...systemIndexRecords(shifted(AT)),
    { record: "lane", segment_id: SEGMENT, lane: "disk_busy", ts: shifted(BEFORE_AT), value: 34 },
    { record: "lane", segment_id: SEGMENT, lane: "disk_busy", ts: shifted(AT), value: 42 },
    {
      record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
      field_ordinal: 11, row_ordinal: "91", ts: shifted(AT),
    },
  ]
}

function snapshotTargetTimelineRecords() {
  return timelineRecords().map((record) => record.record !== "finished_segment"
    ? record
    : {
        ...record,
        sections: [...record.sections, {
          logical_name: "os_meminfo", physical_name: "os_meminfo", type_id: "1104001",
          implementation: "linux", source_family: "system", rows: "1", bytes: "128",
        }, {
          logical_name: "os_diskstats", physical_name: "os_diskstats", type_id: "1108001",
          implementation: "linux", source_family: "system", rows: "1", bytes: "256",
        }],
      })
}

function systemIndexRecords(timestamp) {
  return [
    ["os_cpu_busy_percent", 42],
    ["os_mem_available_percent", 58],
    ["os_oom_kills", 0],
    ["os_min_filesystem_free_percent", 61],
    ["os_device_count", 3],
    ["os_device_active_io", 1],
    ["os_filesystem_count", 8],
    ["os_interface_count", 4],
    ["os_network_rx", 2_400_000],
    ["os_network_tx", 1_700_000],
    ["os_network_errors", 0],
    ["os_network_drops", 0],
  ].map(([series, value]) => ({ record: "point", type_id: "0", series, ts: timestamp, identity: {}, value }))
}

function systemSnapshotRecords(cgroupContext = false, at = AT) {
  const cpuColumns = ["ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal", "scope"]
  const cgroupPaths = at === BEFORE_AT
    ? ["/collector/cpu-before", "/collector/memory-before", "/collector/io-before"]
    : ["/collector/cpu", "/collector/memory", "/collector/io"]
  return [
    { record: "layout", rates: ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"], layout: { type_id: "1102001", logical_name: "os_cpu", columns: cpuColumns.map((name) => ({ name })) } },
    row("1102001", "cpu-all", [String(at), -1, 20, 5, 10, 50, 5, 2, 3, 5, 0], at),
    row("1102001", "cpu-0", [String(at), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    row("1102001", "cpu-1", [String(at), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    layout("1113001", "os_topology", ["ts", "cpu_id", "model_name", "mhz_max", "core_id", "socket_id", "numa_node", "scope"]),
    row("1113001", "topology-0", [String(at), 0, "Artifact CPU", null, 0, 0, 0, 0], at),
    layout("1103001", "os_stat", ["ts", "ctxt", "procs_running", "procs_blocked"]),
    row("1103001", "1", [String(at), 1_234_567, 3, 1], at),
    layout("1105001", "os_loadavg", ["ts", "load1", "load5", "load15", "running", "total"]),
    row("1105001", "2", [String(at), 1.25, 1.1, 0.9, 3, 214], at),
    layout("1104001", "os_meminfo", ["ts", "mem_available", "mem_total", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim", "swap_free", "swap_total"]),
    row("1104001", "3", [String(at), 8_388_608, 16_777_216, 1_048_576, 4_194_304, 131_072, 5_242_880, 524_288, 262_144, 1_048_576, 2_097_152], at),
    layout("1107001", "os_psi", ["ts", "resource", "some_avg10"]),
    row("1107001", "4", [String(at), 0, 2.5], at),
    row("1107001", "5", [String(at), 1, 1.2], at),
    row("1107001", "6", [String(at), 2, 0.4], at),
    layout("1108001", "os_diskstats", ["ts", "major", "minor", "device", "reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_time_ms", "io_weighted_time_ms", "io_in_progress", "scope"]),
    row("1108001", "7", [String(at), 8, 0, "device_target_A", 100, 50, 200, 150, 1_000, 500, 2_000, 2_500, 1, 0], at),
    ...(cgroupContext ? [
      layout("1000001", "instance_metadata", ["environment"]),
      row("1000001", "metadata", [1], at),
      layout("1205001", "os_cgroup_context", [
        "ts", "cgroup_version", "cpu_path", "memory_path", "io_path", "cpuset_cpus",
        "effective_cpu_quota_usec", "effective_cpu_period_usec", "effective_memory_max", "scope",
      ]),
      row("1205001", "context", [String(at), 1, ...cgroupPaths, 2, -1, 100_000, null, 3], at),
    ] : []),
  ]
}

function cgroupSnapshotRecords(url) {
  const section = url.searchParams.get("section")
  const fields = url.searchParams.getAll("field")
  const at = Number(url.searchParams.get("at") ?? AT)
  const path = url.searchParams.get("where.cgroup_path")
  const definitions = {
    os_cgroup_cpu: {
      typeId: "1201001",
      values: { ts: at, cgroup_path: path, usage_usec: 1_500_000, user_usec: 1_000_000, system_usec: 400_000, throttled_usec: 0, nr_throttled: 0, quota_usec: 400_000, period_usec: 100_000, scope: 3 },
    },
    os_cgroup_memory: {
      typeId: "1202001",
      values: { ts: at, cgroup_path: path, current: 1024, max: 4096, anon: 512, file: 256, kernel: 128, slab: 64, low_events: 0, high_events: 0, max_events: 0, oom_events: 0, oom_kill: 0, scope: 3 },
    },
    os_cgroup_io: {
      typeId: "1203002",
      values: { ts: at, cgroup_path: path, major: 8, minor: 0, rbytes: 1024, wbytes: 2048, rios: 2, wios: 3, scope: 3 },
    },
  }
  const definition = definitions[section]
  if (definition === undefined) return []
  return [
    layout(definition.typeId, section, fields),
    row(definition.typeId, section, fields.map((field) => definition.values[field] ?? null), at),
  ]
}

function layout(typeId, logicalName, columns) {
  return { record: "layout", rates: [], layout: { type_id: typeId, logical_name: logicalName, columns: columns.map((name) => ({ name })) } }
}

function row(typeId, ordinal, values, timestamp = AT) {
  return { record: "row", segment_id: SEGMENT, type_id: typeId, ordinal, timestamp: String(timestamp), values }
}

function relationRecords(url, mode) {
  const indexes = url.searchParams.getAll("section").includes("pg_stat_user_indexes")
  const logicalName = indexes ? "pg_stat_user_indexes" : "pg_stat_user_tables"
  const group = url.searchParams.get("group") ?? "object"
  const state = url.searchParams.getAll("field").includes("invalid_count")
  const requestedFields = url.searchParams.getAll("field")
  const sized = requestedFields.includes("reltuples") || requestedFields.includes("main_fork_bytes")
  const page = url.searchParams.get("cursor")
  const count = mode === "long" ? page === null ? 200 : 5 : mode === "short" ? 3 : 1
  const offset = page === null ? 0 : 200
  const baseKey = group === "database"
    ? { datid: "42", datname: "artifact_db" }
    : group === "schema"
      ? { datid: "42", datname: "artifact_db", schemaname: "public" }
      : group === "tablespace"
        ? { tablespace_oid: "1663" }
      : indexes
        ? { datid: "42", datname: "artifact_db", schemaname: "public", relid: "73", relname: "artifact_table", indexrelid: "74", indexrelname: "artifact_index" }
        : { datid: "42", datname: "artifact_db", schemaname: "public", relid: "73", relname: "artifact_table" }
  const columns = indexes
    ? state
      ? group === "object"
        ? [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("indisvalid", "bool", "none"), wire("indisready", "bool", "none"), wire("indisunique", "bool", "none"), wire("indisprimary", "bool", "none")]
        : [...(group === "tablespace" ? [wire("tablespace", "text", "none")] : []), wire("index_count"), wire("invalid_count"), wire("unready_count"), wire("unique_count"), wire("primary_count"), wire("exclusion_count")]
      : sized
        ? [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("main_fork_bytes", "number", "bytes"), wire("idx_blks_read", "number", "per_second"), wire("idx_blks_hit", "number", "per_second"), wire("buffer_hit_pct", "number", "percent")]
        : [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("idx_scan", "number", "per_second")]
    : sized
      ? [wire("tablespace", "text", "none"), wire("main_fork_bytes", "number", "bytes"), wire("toast_bytes", "number", "bytes"), wire("reltuples"), wire("toast_n_live_tup"), wire("toast_n_dead_tup")]
      : group === "tablespace"
        ? [wire("tablespace", "text", "none"), wire("table_count"), wire("seq_scan", "number", "per_second")]
        : [wire("tablespace", "text", "none"), wire("seq_scan", "number", "per_second")]
  const baseValues = indexes
    ? state
      ? group === "object"
        ? { tablespace: "pg_default", amname: "btree", indisvalid: true, indisready: true, indisunique: true, indisprimary: true }
        : { ...(group === "tablespace" ? { tablespace: "fast_ssd" } : {}), index_count: 363, invalid_count: 0, unready_count: 0, unique_count: 223, primary_count: 111, exclusion_count: 0 }
      : sized
        ? { tablespace: "pg_default", amname: "btree", main_fork_bytes: 524_288, idx_blks_read: 2, idx_blks_hit: 14, buffer_hit_pct: 87.5 }
        : { tablespace: "pg_default", amname: "btree", idx_scan: 3 }
    : sized
      ? { tablespace: "pg_default", main_fork_bytes: 1_048_576, toast_bytes: 131_072, reltuples: "9007199254740993", toast_n_live_tup: "713456", toast_n_dead_tup: "12876" }
      : { tablespace: group === "tablespace" ? "fast_ssd" : "pg_default", ...(group === "tablespace" ? { table_count: 17 } : {}), seq_scan: 3 }
  const rows = Array.from({ length: count }, (_, local) => {
    const index = offset + local
    const key = group !== "object" ? baseKey : indexes
      ? { ...baseKey, indexrelid: String(74 + index), indexrelname: index === 0 ? "artifact_index" : `artifact_index_${index}` }
      : { ...baseKey, relid: String(73 + index), relname: index === 0 ? "artifact_table" : `artifact_table_${index}` }
    return {
      record: "relation", logical_name: logicalName, group, key, values: baseValues,
      sample_from: String(AT - 5_000_000), sample_to: String(AT),
      source: group === "object" ? { segment_id: SEGMENT, type_id: indexes ? "1014004" : "1013005", ordinal: String((indexes ? 8 : 7) + index), timestamp: String(AT) } : null,
    }
  })
  const hasMore = mode === "long" && page === null
  return [
    {
      record: "relation_layout", logical_name: logicalName, group, columns,
    },
    ...rows,
    {
      record: "snapshot_page", logical_name: logicalName, group,
      eligible: String(mode === "long" ? 205 : count), returned: String(count), has_more: hasMore, truncated: false, next_cursor: hasMore ? "viewport-page-two" : null,
      page_size: 200, order_by: url.searchParams.getAll("by"), order_direction: url.searchParams.get("order") ?? "desc",
      from: String(AT - 5_000_000), to: String(AT),
    },
  ]
}

function aggregateRelationHistoryRecords(url) {
  const logicalName = url.searchParams.get("section")
  const group = url.searchParams.get("group")
  const fields = url.searchParams.getAll("field")
  const values = (offset) => Object.fromEntries(fields.map((field) => [field, ({
    index_count: 363,
    invalid_count: offset,
    unready_count: 0,
    unique_count: 223 + offset,
    primary_count: 111,
    exclusion_count: 0,
  })[field] ?? 0]))
  return [
    { record: "hour", from: String(HOUR), to: String(AT), available_hours: [String(HOUR)] },
    { record: "relation_layout", logical_name: logicalName, group, columns: fields.map((field) => wire(field)) },
    { record: "series_segment", segment: { id: SEGMENT } },
    {
      record: "relation", logical_name: logicalName, group,
      key: group === "tablespace" ? { tablespace_oid: "1663" } : { datid: "42", datname: "artifact_db" }, values: values(0),
      sample_from: String(BEFORE_AT - 5_000_000), sample_to: String(BEFORE_AT), source: null,
    },
    {
      record: "relation", logical_name: logicalName, group,
      key: group === "tablespace" ? { tablespace_oid: "1663" } : { datid: "42", datname: "artifact_db" }, values: values(1),
      sample_from: String(BEFORE_AT), sample_to: String(AT), source: null,
    },
  ]
}

function wire(name, kind = "number", unit = "count") {
  return { name, kind, unit, nullable: true }
}

function exactIndexRecords() {
  const columns = ["ts", "datid", "datname", "schemaname", "relid", "relname", "indexrelid", "indexrelname", "indexdef", "idx_scan"]
  return [{ record: "layout", rates: ["idx_scan"], layout: { type_id: "1014004", logical_name: "pg_stat_user_indexes", columns: columns.map((name) => ({ name })) } }, {
    record: "row", segment_id: SEGMENT, type_id: "1014004", ordinal: "8", timestamp: String(AT),
    values: [String(AT), "42", "artifact_db", "public", "73", "artifact_table", "74", "artifact_index", "CREATE UNIQUE INDEX artifact_index ON public.artifact_table USING btree (id)", 15],
  }]
}

function snapshotRecords() {
  const processColumns = [
    "ts", "pid", "comm", "cmdline", "ppid", "uid", "euid", "gid", "egid", "num_threads", "tty", "exit_signal",
    "state", "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "curcpu", "nice", "prio", "rtprio", "policy",
    "rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt", "read_bytes", "write_bytes", "syscr", "syscw", "rchar", "wchar", "cancelled_write_bytes",
    "user", "effective_user",
  ]
  const columns = [
    "ts", "pid", "leader_pid", "datid", "datname", "usename", "application_name", "client_addr", "backend_type",
    "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age", "backend_xmin_age",
    "backend_start", "xact_start", "query_start", "state_change",
  ]
  const processValues = (pid, index) => [
    String(AT), pid, pid === 2_686_712 ? "postgres" : `worker-${index}`, pid === 2_686_712 ? "postgres: artifact_db artifact_role 192.0.2.72" : null,
    1, pid === 2_686_712 ? 26 : 1000, pid === 2_686_712 ? 27 : 1000, 1000, 1000, 2 + index % 4, 0, 17, index % 3 === 0 ? 82 : 83,
    1000 + index, 300 + index, 5_000_000 + index, 12 + index, 50 + index, 3 + index, index % 8, -5, 15, 0, 0,
    1024 + index, 4096 + index, index % 3, 20 + index, 2 + index, 4096 + index, 8192 + index, 4 + index, 5 + index, 16_384 + index, 32_768 + index, 0,
    pid === 2_686_712 ? "postgres" : "app", pid === 2_686_712 ? "postgres-worker" : "app",
  ]
  return [
    layout("1100001", "os_process", processColumns),
    ...Array.from({ length: 80 }, (_, index) => {
      const pid = index === 27 ? 2_686_712 : 2_686_800 + index
      return row("1100001", String(index), processValues(pid, index), AT)
    }),
    {
      record: "layout", rates: [],
      layout: { type_id: "1001004", logical_name: "pg_stat_activity", columns: columns.map((name) => ({ name })) },
    },
    {
      record: "row", segment_id: SEGMENT, type_id: "1001004", ordinal: "73", timestamp: String(AT),
      values: [
        String(AT), 4242, null, 20, "operators", "kronika", "artifact-test", "127.0.0.1", "client backend",
        "active", null, null, "select artifact_wire_contract", "991", null, "7",
        String(AT - 60_000_000), String(AT - 30_000_000), String(AT - 5_000_000), String(AT - 1_000_000),
      ],
    },
    {
      record: "row", segment_id: SEGMENT, type_id: "1001004", ordinal: "74", timestamp: String(AT - 2_000_000),
      values: [
        String(AT - 2_000_000), 2_686_712, null, 21, "artifact_db", "artifact_role", "offset-activity", "192.0.2.72", "client backend",
        "active", "IO", "DataFileRead", "select activity_for_2686712", "992", "3", "8",
        String(AT - 120_000_000), String(AT - 45_000_000), String(AT - 7_000_000), String(AT - 2_500_000),
      ],
    },
  ]
}

function progressVacuumRecords() {
  const columns = [
    "ts", "pid", "datid", "datname", "relid", "is_autovacuum", "phase", "heap_blks_total", "heap_blks_scanned",
    "heap_blks_vacuumed", "index_vacuum_count", "max_dead_tuple_bytes", "dead_tuple_bytes", "num_dead_item_ids",
    "indexes_total", "indexes_processed", "delay_time",
  ]
  return [layout("1012003", "pg_stat_progress_vacuum", columns), row("1012003", "1", [
    String(AT), 4343, 20, "operators", 73, true, "vacuuming heap", 8000, 3200, 1200, 1, 67_108_864, 4_194_304, 2400, 5, 2, 17.5,
  ], AT)]
}

function activityHistoryRecords(url) {
  const fields = url.searchParams.getAll("field")
  const samples = [
    { pid: 4242, query_start: String(BEFORE_AT - 5_000_000), state: "active", timestamp: BEFORE_AT, xact_start: String(BEFORE_AT - 12_000_000) },
    { pid: 4242, query_start: String(AT - 5_000_000), state: "active", timestamp: AT, xact_start: String(AT - 30_000_000) },
  ]
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    layout("1001004", "pg_stat_activity", fields),
    ...samples.map((sample, index) => row("1001004", String(70 + index), fields.map((field) => sample[field] ?? null), sample.timestamp)),
  ]
}

function statementRecords(page, eligible = 1, hasMore = false, rowCount = eligible) {
  const columns = ["ts", "queryid", "userid", "dbid", "toplevel", "datname", "usename", "query", "calls", "rows", "total_exec_time"]
  return [
    {
      record: "layout", rates: ["calls", "rows", "total_exec_time"],
      layout: { type_id: "1002003", logical_name: "pg_stat_statements", columns: columns.map((name) => ({ name })) },
    },
    ...Array.from({ length: rowCount }, (_, index) => ({
      record: "row", segment_id: SEGMENT, type_id: "1002003", ordinal: String(91 + index), timestamp: String(AT),
      values: [String(AT), String(9_007_199_254_740_991n - BigInt(index)), 10, 20, true, "operators", "reporter", index === 0 ? "select artifact_exact_context" : `select artifact_page_${index}`, 2 + index, 1, 7.5 + index],
    })),
    ...(page ? [{
      record: "snapshot_page", logical_name: "pg_stat_statements", eligible: String(eligible), returned: String(rowCount),
      has_more: hasMore, truncated: hasMore, next_cursor: hasMore ? "next-statement-page" : null, page_size: 200,
      order_by: ["total_exec_time", "calls"], order_direction: "desc", from: String(AT - 5_000_000), to: String(AT),
    }] : []),
  ]
}

const VADV_TEXT_PLAN = [
  "Merge Join  (cost=0.85..81.42 rows=12 width=64)",
  "  Merge Cond: (orders.customer_id = customers.id)",
  "  ->  Seq Scan on orders  (cost=0.00..20.00 rows=400 width=32)",
  "        Filter: (status = 'open'::text)",
  "  ->  Index Scan using customers_pkey on customers  (cost=0.42..8.44 rows=1 width=32)",
  "        Index Cond: (id > 0)",
].join("\n")

const INLINE_QUERY_PRIMARY = [
  "  SELECT jobs.id, jobs.payload",
  "  FROM jobs",
  "  WHERE jobs.state = 'ready'",
  ...Array.from({ length: 70 }, (_, index) => `    AND jobs.partition_${index} = ${index}`),
  "  ORDER BY jobs.created_at",
].join("\n")
function planRecords() {
  const columns = ["ts", "userid", "dbid", "queryid", "planid", "queryid_stat_statements", "datname", "usename", "plan", "calls", "total_time", "rows"]
  return [
    { record: "layout", rates: ["calls", "total_time", "rows"], layout: { type_id: "1004001", logical_name: "pg_store_plans", columns: columns.map((name) => ({ name })) } },
    { record: "row", segment_id: SEGMENT, type_id: "1004001", ordinal: "201", timestamp: String(AT), values: [String(AT), 10, 20, 0, 77, 42, "operators", "reporter", VADV_TEXT_PLAN, 3, 12, 4] },
    { record: "snapshot_page", logical_name: "pg_store_plans", eligible: "1", returned: "1", has_more: false, truncated: false, next_cursor: null, page_size: 200, order_by: ["total_time", "calls"], order_direction: "desc", from: String(AT - 5_000_000), to: String(AT) },
  ]
}

function planStatementRecords() {
  return statementRecords(true).map((record) => record.record !== "row" ? record : {
    ...record,
    values: record.values.map((stored, index) => index === 1 ? "42" : index === 7 ? "select from plan_navigation" : stored),
  })
}

function inlinePlanQueryRecords() {
  const columns = ["query"]
  return [
    { record: "layout", layout: { type_id: "1002003", logical_name: "pg_stat_statements", columns: columns.map((name) => ({ name })) } },
    { record: "row", segment_id: SEGMENT, type_id: "1002003", ordinal: "301", timestamp: String(AT), values: [INLINE_QUERY_PRIMARY] },
    { record: "snapshot_page", logical_name: "pg_stat_statements", eligible: "1", returned: "1", has_more: false, truncated: false, next_cursor: null, page_size: 1, order_by: [], order_direction: "desc", from: null, to: String(AT) },
  ]
}

function emptyPlanQueryRecords() {
  return [{
    record: "snapshot_page", logical_name: "pg_stat_statements", eligible: "0", returned: "0", has_more: false,
    truncated: false, next_cursor: null, page_size: 1, order_by: [], order_direction: "desc", from: null, to: null,
  }]
}

function targetedActivityRecords(query, timestamp) {
  return snapshotRecords().map((record) => record.record !== "row"
    ? record
    : {
        ...record,
        timestamp: String(timestamp),
        values: record.values.map((value, index) => index === 0 ? String(timestamp) : index === 12 ? query : value),
      })
}

function targetedStatementRecords(query, eligible) {
  return statementRecords(true, eligible, false, 1).map((record) => record.record === "row"
    ? { ...record, values: record.values.map((value, index) => index === 7 ? query : value) }
    : record)
}

function targetedRelationRecords(url, label, eligible) {
  return relationRecords(url, "single").map((record) => {
    if (record.record === "snapshot_page") return { ...record, eligible: String(eligible) }
    if (record.record !== "relation") return record
    const key = record.group === "database"
      ? { ...record.key, datname: label }
      : record.group === "schema"
        ? { ...record.key, schemaname: label }
        : record.group === "tablespace"
          ? record.key
        : { ...record.key, relname: label }
    return record.group === "tablespace" ? { ...record, key, values: { ...record.values, tablespace: label } } : { ...record, key }
  })
}


// Every focused server answers the activity ledger's ranking request with one
// small deterministic heatmap so the Statements page renders without noise.
function answerHeatmap(url, response) {
  const from = Number(url.searchParams.get("from") ?? "0")
  const to = Number(url.searchParams.get("to") ?? "0")
  const columns = Number(url.searchParams.get("columns") ?? "60")
  const labels = url.searchParams.getAll("label")
  const span = to - from + 1
  const intervals = Array.from({ length: columns }, (_at, index) => ({
    start: String(from + Math.floor((index * span) / columns)),
    end: String(from + Math.floor(((index + 1) * span) / columns) - 1),
  }))
  const cells = Array.from({ length: columns }, (_at, index) => index < 3 ? (index + 1) * 0.5 : null)
  return ndjson(response, [
    {
      record: "heatmap", from: String(from), to: String(to), section: "pg_stat_statements",
      fields: url.searchParams.getAll("field"), class: "cumulative", labels,
      top: 2, entity_count: 3, others_count: 1, out_of_order: "0", intervals,
    },
    { record: "heatmap_row", type_id: "1002006", identity: ["101", "10", "5", "true"], labels: labels.map(() => "demo"), total: 120, cells },
    { record: "heatmap_row", type_id: "1002006", identity: ["102", "10", "5", "true"], labels: labels.map(() => "demo"), total: 60, cells },
    { record: "heatmap_band", band: "totals", total: 200, cells },
    { record: "heatmap_band", band: "others", total: 20, cells },
  ])
}

function ndjson(response, records) {
  response.writeHead(200, {
    "Cache-Control": "no-store",
    "Content-Type": "application/x-ndjson; charset=utf-8",
  })
  response.end(records.map((record) => JSON.stringify(record)).join("\n") + (records.length === 0 ? "" : "\n"))
}

function brokenNdjson(response) {
  response.writeHead(200, {
    "Cache-Control": "no-store",
    "Content-Type": "application/x-ndjson; charset=utf-8",
  })
  response.end("{")
}

function requestRecord(request, url) {
  return {
    authorization: request.headers.authorization ?? null,
    cookie: request.headers.cookie ?? null,
    marker: request.headers["x-kronika-ui"] ?? null,
    method: request.method ?? "GET",
    path: url.pathname,
    query: url.search,
  }
}

function answerSession(request, response, state) {
  const headers = { "Cache-Control": "no-store" }
  if (request.method === "GET") {
    response.writeHead(browserIsAuthenticated(request, state) ? 204 : 401, headers)
    response.end()
    return
  }
  if (request.method === "POST") {
    if (request.headers.authorization !== "Basic YXJ0aWZhY3Q6d2lyZQ==") {
      response.writeHead(401, headers)
      response.end()
      return
    }
    state.valid = true
    response.writeHead(204, {
      ...headers,
      "Set-Cookie": `${SESSION_COOKIE}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000`,
    })
    response.end()
    return
  }
  if (request.method === "DELETE") {
    state.valid = false
    response.writeHead(204, {
      ...headers,
      "Set-Cookie": "kronika_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
    })
    response.end()
    return
  }
  response.writeHead(405, { ...headers, Allow: "GET, POST, DELETE" })
  response.end()
}

function browserIsAuthenticated(request, state) {
  return state.valid
    && request.headers.cookie?.split(";").map((value) => value.trim()).includes(SESSION_COOKIE) === true
    && request.headers["x-kronika-ui"] === "1"
}

function unauthorized(response) {
  response.writeHead(401, { "Cache-Control": "no-store", "Content-Type": "application/json" })
  response.end('{"error":"unauthorized"}')
}

function trackPage(socket, origin, result) {
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data)
    if (message.method === "Runtime.exceptionThrown") {
      result.errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text)
    }
    if (message.method === "Runtime.consoleAPICalled" && ["assert", "error"].includes(message.params.type)) {
      result.errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "))
    }
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") {
      if (!expectedUnauthorizedLog(message.params.entry.text)) result.errors.push(message.params.entry.text)
    }
    if (message.method === "Network.loadingFailed"
      && message.params.canceled !== true
      && message.params.errorText !== "net::ERR_ABORTED") {
      result.errors.push(message.params.errorText)
    }
    if (message.method === "Network.responseReceived") {
      const response = message.params.response
      const url = new URL(response.url)
      result.responses.push({
        challenge: response.headers["www-authenticate"] ?? response.headers["WWW-Authenticate"] ?? null,
        path: url.pathname,
        status: response.status,
      })
      if (response.status >= 400) {
        const expected = response.status === 401 && url.origin === origin
          && (url.pathname === "/auth/session" || url.pathname.startsWith("/api/"))
        if (!expected) result.errors.push(`${response.status}:${response.url}`)
      }
    }
    if (message.method === "Network.requestWillBeSent") {
      const requested = message.params.request.url
      if (/^https?:/.test(requested) && new URL(requested).origin !== origin) result.external.push(requested)
    }
  })
}

function enablePage(cdp) {
  return Promise.all([
    cdp.send("Page.enable"),
    cdp.send("Runtime.enable"),
    cdp.send("Network.enable"),
    cdp.send("Log.enable"),
  ])
}

async function submitLogin(cdp) {
  await cdp.evaluate(`(() => {
    const set = (name, value) => {
      const input = document.querySelector('[name="' + name + '"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, value)
      input.dispatchEvent(new Event("input", { bubbles: true }))
    }
    set("username", "artifact")
    set("password", "wire")
    document.querySelector("form").requestSubmit()
  })()`)
}

function assertBootstrapBeforeApi(requests, restored) {
  const bootstrap = requests.findIndex(({ method, path }) => method === "GET" && path === "/auth/session")
  const firstApi = requests.findIndex(({ path }) => path.startsWith("/api/"))
  assert.ok(bootstrap >= 0, JSON.stringify(requests, null, 2))
  if (!restored) {
    assert.equal(firstApi, -1, JSON.stringify(requests, null, 2))
    return
  }
  assert.ok(firstApi > bootstrap, JSON.stringify(requests, null, 2))
  assert.equal(requests.slice(0, bootstrap).some(({ path }) => path.startsWith("/api/")), false)
  assert.equal(requests.some(({ method, path }) => method === "POST" && path === "/auth/session"), false)
}

function expectedUnauthorizedLog(message) {
  return /^Failed to load resource: the server responded with a status of 401 \(Unauthorized\)$/.test(message)
}

async function waitForRequests(predicate) {
  const started = Date.now()
  while (Date.now() - started < 5_000) {
    if (predicate()) return
    await delay(20)
  }
  throw new Error("timed out waiting for request")
}

function launchBrowser(profile) {
  const executable = browserExecutable()
  const browser = spawn(executable, [
    "--headless",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-dev-shm-usage",
    "--disable-extensions",
    "--disable-gpu",
    "--metrics-recording-only",
    "--no-first-run",
    "--no-sandbox",
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] })
  browser.diagnostics = ""
  browser.stderr.setEncoding("utf8")
  browser.stderr.on("data", (chunk) => { browser.diagnostics = `${browser.diagnostics}${chunk}`.slice(-12_000) })
  return browser
}

function browserExecutable() {
  const candidates = [
    process.env.CHROME_BIN,
    "chromium-browser",
    "chromium",
    "google-chrome-stable",
    "google-chrome",
  ].filter((candidate) => candidate !== undefined && candidate !== "")
  for (const candidate of candidates) {
    const result = spawnSync(candidate, ["--version"], { stdio: "ignore" })
    if (result.status === 0) return candidate
  }
  throw new Error(`no Chromium executable found (tried ${candidates.join(", ")})`)
}

async function browserDebugPort(profile, browser) {
  const activePort = join(profile, "DevToolsActivePort")
  const started = Date.now()
  while (Date.now() - started < 10_000) {
    if (browser.exitCode !== null) throw new Error(`Chromium exited before startup:\n${browser.diagnostics}`)
    try {
      const [port] = (await readFile(activePort, "utf8")).split("\n")
      if (/^\d+$/.test(port ?? "")) return port
    } catch (reason) {
      if (reason?.code !== "ENOENT") throw reason
    }
    await delay(40)
  }
  throw new Error(`timed out starting Chromium:\n${browser.diagnostics}`)
}

async function pageSocket(port, targetId) {
  const started = Date.now()
  while (Date.now() - started < 5_000) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => response.json())
      const target = targets.find((candidate) => (
        candidate.type === "page" && (targetId === undefined || candidate.id === targetId)
      ))
      if (target !== undefined) {
        const socket = new WebSocket(target.webSocketDebuggerUrl)
        await new Promise((resolve, reject) => {
          socket.addEventListener("open", resolve, { once: true })
          socket.addEventListener("error", reject, { once: true })
        })
        return socket
      }
    } catch {}
    await delay(40)
  }
  throw new Error("Chromium did not expose a page target")
}

function cdpSession(socket) {
  let sequence = 0
  const pending = new Map()
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data)
    if (message.id === undefined) return
    const request = pending.get(message.id)
    if (request === undefined) return
    pending.delete(message.id)
    if (message.error === undefined) request.resolve(message.result)
    else request.reject(new Error(`${request.method}: ${message.error.message}`))
  })
  const send = (method, params = {}) => {
    const id = ++sequence
    return new Promise((resolve, reject) => {
      pending.set(id, { method, reject, resolve })
      socket.send(JSON.stringify({ id, method, params }))
    })
  }
  const evaluate = async (expression) => {
    const response = await send("Runtime.evaluate", {
      awaitPromise: true,
      expression,
      returnByValue: true,
      userGesture: true,
    })
    if (response.exceptionDetails !== undefined) {
      throw new Error(response.exceptionDetails.exception?.description ?? response.exceptionDetails.text)
    }
    return response.result.value
  }
  const waitFor = async (expression, description, timeout = 10_000) => {
    const started = Date.now()
    while (Date.now() - started < timeout) {
      if (await evaluate(expression)) return
      await delay(40)
    }
    throw new Error(`timed out waiting for ${description}`)
  }
  return { evaluate, send, waitFor }
}

async function stopBrowser(browser) {
  if (browser.exitCode !== null) return
  browser.kill("SIGTERM")
  await Promise.race([
    new Promise((resolve) => browser.once("exit", resolve)),
    delay(2_000).then(() => { if (browser.exitCode === null) browser.kill("SIGKILL") }),
  ])
}

async function removeBrowserProfile(profile) {
  await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 })
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}

test("mixed-cadence shared cursor uses one exact domain for pointer and both keyboard paths", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const page = { errors: [], external: [], responses: [] }
  const base = HOUR + 1_800_000_000
  const activityOne = base + 1
  const activityTwo = base + 4_000
  const five = base + 5_000_000
  const ten = base + 10_000_000
  const activityRecords = snapshotRecords().filter((record) => record.record === "layout"
    ? record.layout.logical_name === "pg_stat_activity"
    : record.record === "row" && record.type_id === "1001004")
    .map((record, index) => {
      if (record.record !== "row") return record
      const timestamp = index === 1 ? activityOne : activityTwo
      return { ...record, timestamp: String(timestamp), values: [String(timestamp), ...record.values.slice(1)] }
    })
  const timeline = [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    { record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1), source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }] },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(HOUR + HOUR_US - 1),
      sections: [{ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "2", bytes: "256" }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(base), identity: {}, value: 80 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(base), identity: {}, value: 75 },
    ...[-30_000_000, 0, 30_000_000].map((offset, index) => ({ record: "lane", segment_id: SEGMENT, lane: "pg_waiting", ts: String(base + offset), value: index + 1 })),
    ...[0, 5_000_000, 10_000_000, 15_000_000].map((offset, index) => ({ record: "lane", segment_id: SEGMENT, lane: "cpu_busy", ts: String(base + offset), value: 20 + index })),
  ]
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, url.searchParams.has("section") ? [] : timeline)
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, activityRecords)
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("mixed-cadence browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    const waitAt = async (timestamp, label) => {
      await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${timestamp}"`, label, 15_000)
      await cdp.waitFor(
        `document.querySelector('[data-testid="cursor-behind"]') === null && document.querySelector('[data-testid="hour-timeline"]')?.dataset.navigationCount === "8"`,
        `${label} snapshot`,
        15_000,
      )
      assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"]')?.dataset.selectedTimestamp`), String(timestamp))
    }
    for (const width of [800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
      await cdp.send("Page.navigate", { url: `${origin}/?at=${ten}&view=pg.activity` })
      await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, `${width}px Activity rows`, 15_000)
      if (await cdp.evaluate(`document.documentElement.lang !== "ru"`)) {
        await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
        await cdp.waitFor(`document.documentElement.lang === "ru"`, `${width}px RU locale`)
      }
      await waitAt(ten, `${width}px initial cursor`)
      const geometry = await cdp.evaluate(`(() => {
        const figure = document.querySelector('[data-testid="hour-timeline"]')
        const plot = figure.querySelector('.u-over')
        const tabs = document.querySelector('.pg-tabs')
        const figureBox = figure.getBoundingClientRect()
        const plotBox = plot.getBoundingClientRect()
        const tabsBox = tabs.getBoundingClientRect()
        return {
          count: Number(figure.dataset.navigationCount),
          figureBottom: figureBox.bottom,
          figureLeft: figureBox.left,
          figureRight: figureBox.right,
          plotBottom: plotBox.bottom,
          plotLeft: plotBox.left,
          plotRight: plotBox.right,
          tabsTop: tabsBox.top,
        }
      })()`)
      assert.equal(geometry.count, 8, `${width}:${JSON.stringify(geometry)}`)
      assert.ok(geometry.figureBottom <= geometry.tabsTop + 0.75, `${width}:${JSON.stringify(geometry)}`)
      assert.ok(geometry.plotLeft >= geometry.figureLeft && geometry.plotRight <= geometry.figureRight && geometry.plotBottom <= geometry.figureBottom, `${width}:${JSON.stringify(geometry)}`)

      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowLeft' }))`)
      await waitAt(five, `${width}px global five-second cursor`)
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowLeft' }))`)
      await waitAt(activityTwo, `${width}px exact second Activity cursor`)
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowLeft' }))`)
      await waitAt(activityOne, `${width}px exact first Activity cursor`)

      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').focus()`)
      await cdp.send("Input.dispatchKeyEvent", { code: "ArrowRight", key: "ArrowRight", nativeVirtualKeyCode: 39, type: "keyDown", windowsVirtualKeyCode: 39 })
      await cdp.send("Input.dispatchKeyEvent", { code: "ArrowRight", key: "ArrowRight", nativeVirtualKeyCode: 39, type: "keyUp", windowsVirtualKeyCode: 39 })
      await waitAt(activityTwo, `${width}px chart keyboard cursor`)

      const point = await cdp.evaluate(`(() => {
        const figure = document.querySelector('[data-testid="hour-timeline"]')
        const host = figure.querySelector('.uplot-host').getBoundingClientRect()
        const plot = figure.querySelector('.u-over').getBoundingClientRect()
        const extendedEnd = ${HOUR + HOUR_US} + ${HOUR_US} * 38 / Math.max(1, host.width - 84)
        return {
          x: plot.left + (${ten} - ${HOUR}) / (extendedEnd - ${HOUR}) * plot.width,
          y: plot.top + plot.height / 2,
        }
      })()`)
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...point })
      await cdp.send("Input.dispatchMouseEvent", { button: "left", buttons: 1, clickCount: 1, type: "mousePressed", ...point })
      await cdp.send("Input.dispatchMouseEvent", { button: "left", buttons: 0, clickCount: 1, type: "mouseReleased", ...point })
      await waitAt(ten, `${width}px pointer cursor`)
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("narrow controls stay contained and help never changes selection", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  let historyFailure = false
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      if (url.searchParams.has("section") && historyFailure) return ndjson(response, [{ record: "error", error: "history unavailable" }])
      return ndjson(response, url.searchParams.has("section") ? [] : [
        ...timelineRecords(HOUR),
        { record: "lane", segment_id: SEGMENT, lane: "cpu_busy", ts: String(AT), value: 42 },
      ])
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      return ndjson(response, url.searchParams.getAll("section").includes("pg_stat_activity") ? snapshotRecords() : systemSnapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("narrow browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 480 })
    await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the host ledger", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the narrow System view", 15_000)
    await settleLayout(cdp)

    const closed = await cdp.evaluate(`(() => {
      const actions = document.querySelector('.top-actions').getBoundingClientRect()
      return {
        actions: { left: actions.left, right: actions.right, width: actions.width },
        client: document.documentElement.clientWidth,
        coarse: matchMedia('(pointer: coarse)').matches,
        nested: document.querySelectorAll('button button').length,
        scroll: document.documentElement.scrollWidth,
        topbarScroll: document.querySelector('.topbar').scrollWidth,
      }
    })()`)
    assert.equal(closed.coarse, true)
    assert.equal(closed.nested, 0)
    assert.ok(closed.actions.left >= 0 && closed.actions.right <= closed.client, JSON.stringify(closed))
    assert.ok(closed.scroll <= closed.client && closed.topbarScroll <= closed.client, JSON.stringify(closed))

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await settleLayout(cdp)
    const russianActions = await cdp.evaluate(`(() => {
      const actions = document.querySelector('.top-actions').getBoundingClientRect()
      return {
        client: document.documentElement.clientWidth,
        left: actions.left,
        right: actions.right,
        scroll: document.documentElement.scrollWidth,
        topbarScroll: document.querySelector('.topbar').scrollWidth,
      }
    })()`)
    assert.ok(russianActions.left >= 0 && russianActions.right <= russianActions.client, JSON.stringify(russianActions))
    assert.ok(russianActions.scroll <= russianActions.client && russianActions.topbarScroll <= russianActions.client, JSON.stringify(russianActions))
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await settleLayout(cdp)

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "the narrow hour popover")
    await settleLayout(cdp)
    const popover = await cdp.evaluate(`(() => {
      const rect = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
      return { client: document.documentElement.clientWidth, left: rect.left, right: rect.right, scroll: document.documentElement.scrollWidth }
    })()`)
    assert.ok(popover.left >= 0 && popover.right <= popover.client && popover.scroll <= popover.client, JSON.stringify(popover))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "Escape closing the hour popover")
    assert.equal(await cdp.evaluate(`document.activeElement?.dataset.testid`), "hour-picker-trigger")

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU chips")
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    const selectedMetric = await cdp.evaluate(`document.querySelector('[data-testid="system-group-chart-cpu"] button[aria-pressed="true"]')?.dataset.testid`)
    await cdp.evaluate(`document.querySelector('.use-cell .help-dot').click()`)
    await cdp.waitFor(`document.querySelector('[role="tooltip"]') !== null`, "the System metric help")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-group-chart-cpu"] button[aria-pressed="true"]')?.dataset.testid`), selectedMetric)
    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)

    const selectedLane = await cdp.evaluate(`document.querySelector('.lane-select[aria-pressed="true"]')?.textContent`)
    await cdp.evaluate(`(() => {
      const other = [...document.querySelectorAll('.lane-label')].find((lane) => lane.querySelector('.lane-select').getAttribute('aria-pressed') !== 'true')
      other.querySelector('.help-dot').click()
    })()`)
    await cdp.waitFor(`document.querySelector('[role="tooltip"]') !== null`, "the timeline lane help")
    assert.equal(await cdp.evaluate(`document.querySelector('.lane-select[aria-pressed="true"]')?.textContent`), selectedLane)
    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)

    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null`, "the PostgreSQL tabs", 15_000)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Activity')) ?? document.querySelectorAll('.pg-tabs button')[1]).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-header-cell > .label-help .help-dot') !== null`, "the Activity header help", 15_000)
    const headerHelp = await cdp.evaluate(`(() => {
      const button = document.querySelector('[data-testid="pg-activity-table"] .entity-header-cell > .label-help .help-dot')
      button.scrollIntoView({ block: "nearest", inline: "nearest" })
      const rect = button.getBoundingClientRect()
      const style = getComputedStyle(button.closest('.label-help'))
      const after = getComputedStyle(button, "::after")
      const x = rect.left + rect.width / 2
      const y = rect.top + rect.height / 2
      // The mark stays small; the touch target grows invisibly via ::after,
      // and every side of it has to survive the cell and the scroll port.
      const reach = (dx, dy) => {
        let grown = 0
        for (let step = 1; step <= 15; step += 1) {
          if (document.elementFromPoint(x + dx * step, y + dy * step) !== button) break
          grown = step
        }
        return grown
      }
      button.click()
      return {
        afterContent: after.content,
        afterInset: after.inset,
        height: rect.height,
        opacity: style.opacity,
        pointerEvents: style.pointerEvents,
        reach: { down: reach(0, 1), left: reach(-1, 0), right: reach(1, 0), up: reach(0, -1) },
        width: rect.width,
      }
    })()`)
    assert.deepEqual(
      [headerHelp.afterContent, headerHelp.afterInset, headerHelp.height, headerHelp.opacity, headerHelp.pointerEvents, headerHelp.width],
      ['""', "-11px", 14, "1", "auto", 14],
      JSON.stringify(headerHelp),
    )
    // 36x36 around a 14px mark, reached from every side. Flush against the
    // table's scroll port the right half used to be clipped to 6px; the mark
    // now steps in and the port keeps scroll padding, and nothing of the
    // neighbouring column is taken.
    for (const [side, reach] of Object.entries(headerHelp.reach)) {
      assert.ok(reach >= 14 && reach <= 15, `${side} touch reach: ${JSON.stringify(headerHelp)}`)
    }
    assert.ok(
      headerHelp.width + headerHelp.reach.left + headerHelp.reach.right >= 42
        && headerHelp.height + headerHelp.reach.up + headerHelp.reach.down >= 42,
      `complete touch target: ${JSON.stringify(headerHelp)}`,
    )
    await cdp.waitFor(`document.querySelector('[role="tooltip"]') !== null`, "the first-touch table help")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('button button').length`), 0)

    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)
    historyFailure = true
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null`, "the Activity detail")
    await cdp.evaluate(`([...document.querySelectorAll('.inspector-tabs button')].find((button) => button.textContent === 'Chart')).click()`)
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot [data-testid="series-status"][data-status="error"]') !== null`, "the Activity history error")
    assert.equal(await cdp.evaluate(`document.querySelector('.inspector-chart-slot [data-testid="series-status"][data-status="error"]').textContent`), "Could not load history")
    const failedHistory = await cdp.evaluate(`(() => {
      const detail = document.querySelector('.inspector-chart-slot')
      const status = detail.querySelector('[data-testid="series-status"][data-status="error"]').getBoundingClientRect()
      return { chart: detail.querySelector('.uplot-host') !== null, statusHeight: status.height, text: detail.querySelector('.series-chart')?.textContent ?? '' }
    })()`)
    assert.equal(failedHistory.chart, true, JSON.stringify(failedHistory))
    assert.ok(failedHistory.statusHeight <= 40, JSON.stringify(failedHistory))
    historyFailure = false

    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 960 })
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the 960px System view", 15_000)
    await settleLayout(cdp)
    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "the 960px hour popover")
    const standard = await cdp.evaluate(`(() => {
      const actions = document.querySelector('.top-actions').getBoundingClientRect()
      const popover = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
      const client = document.documentElement.clientWidth
      return { actionsLeft: actions.left, actionsRight: actions.right, client, popoverLeft: popover.left, popoverRight: popover.right, scroll: document.documentElement.scrollWidth }
    })()`)
    assert.ok(standard.actionsLeft >= 0 && standard.actionsRight <= standard.client, JSON.stringify(standard))
    assert.ok(standard.popoverLeft >= 0 && standard.popoverRight <= standard.client && standard.scroll <= standard.client, JSON.stringify(standard))
    for (const width of [521, 580, 600, 760, 761]) {
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' }))`)
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
      await settleLayout(cdp)
      await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, `the ${width}px hour popover`)
      const intermediate = await cdp.evaluate(`(() => {
        const rect = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
        const client = document.documentElement.clientWidth
        return { client, left: rect.left, right: rect.right, scroll: document.documentElement.scrollWidth }
      })()`)
      assert.ok(intermediate.left >= 0 && intermediate.right <= intermediate.client && intermediate.scroll <= intermediate.client, `${width}:${JSON.stringify(intermediate)}`)
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})
