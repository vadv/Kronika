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
  const profile = await mkdtemp(join(tmpdir(), "kronika-timezone-browser-"))
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
    const browserMode = await cdp.evaluate(`(() => ({
      at: new URL(location.href).searchParams.get("at"),
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent ?? "",
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent ?? "",
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth || document.querySelector(".topbar").scrollWidth > document.querySelector(".topbar").clientWidth,
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      updated: document.querySelector('[data-testid="updated-time"]')?.textContent ?? "",
      zone: document.querySelector('[data-testid="timezone-select"]')?.value,
      zoneLabel: document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent ?? "",
      zoneSelectors: document.querySelectorAll('[data-testid="timezone-select"]').length,
    }))()`)
    assert.equal(browserMode.at, String(AT))
    assert.equal(browserMode.zone, "browser")
    assert.equal(browserMode.zoneLabel, "Browser local")
    assert.equal(browserMode.zoneSelectors, 1)
    assert.match(browserMode.cursor, /08:30:00/)
    assert.match(browserMode.hour, /08:00–09:00/)
    assert.match(browserMode.status, /08:30:00/)
    assert.match(browserMode.updated, /\d{2}:\d{2}:\d{2}/)
    for (const output of [browserMode.cursor, browserMode.hour, browserMode.status, browserMode.updated]) {
      assert.doesNotMatch(output, /GMT|UTC|\.\d{3}/)
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
    assert.match(tooltip, /08:30:00/)
    assert.match(tooltip, /41\.7%/)
    assert.doesNotMatch(tooltip, /41\.729068|GMT|UTC|\.000/)
    const apiBeforeSwitch = requests.filter(({ path }) => path.startsWith("/api/")).length
    await cdp.evaluate(`(() => {
      const select = document.querySelector('[data-testid="timezone-select"]')
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(select, "utc")
      select.dispatchEvent(new Event("change", { bubbles: true }))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-select"]')?.value === "utc" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("05:30:00") === true`, "the UTC display mode")
    await hover()
    const utcMode = await cdp.evaluate(`(() => ({
      at: new URL(location.href).searchParams.get("at"),
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent ?? "",
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent ?? "",
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      tooltip: document.querySelector('[data-testid="hour-timeline"] .chart-tooltip')?.textContent ?? "",
      updated: document.querySelector('[data-testid="updated-time"]')?.textContent ?? "",
      zone: document.querySelector('[data-testid="timezone-select"]')?.value,
      zoneLabel: document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent ?? "",
    }))()`)
    assert.equal(utcMode.at, String(AT))
    assert.equal(utcMode.zone, "utc")
    assert.equal(utcMode.zoneLabel, "UTC")
    assert.match(utcMode.cursor, /05:30:00/)
    assert.match(utcMode.hour, /05:00–06:00/)
    assert.match(utcMode.status, /05:30:00/)
    assert.match(utcMode.tooltip, /05:30:00/)
    assert.match(utcMode.updated, /\d{2}:\d{2}:\d{2}/)
    for (const output of [utcMode.cursor, utcMode.hour, utcMode.status, utcMode.tooltip, utcMode.updated]) {
      assert.doesNotMatch(output, /GMT|UTC|\.\d{3}/)
    }
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/")).length, apiBeforeSwitch)
    await cdp.evaluate(`(() => {
      const select = document.querySelector('[data-testid="timezone-select"]')
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(select, "browser")
      select.dispatchEvent(new Event("change", { bubbles: true }))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent === "Browser local" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("08:30:00") === true`, "the Browser display restore")
    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-select"]')?.value === "browser" && document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent === "Browser local" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("08:30:00") === true`, "the persisted Browser display", 15_000)

    await cdp.evaluate(`document.querySelector(".source-tabs button:first-child").click()`)
    await cdp.waitFor(`document.querySelector(".source-tabs button:first-child")?.getAttribute("aria-current") === "page"`, "the Host history destination")
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
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="timezone-select"]')?.value`), "browser")
    assert.equal(await cdp.evaluate(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`), true)
    assert.deepEqual(result.errors, [])
    assert.deepEqual(result.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await rm(profile, { recursive: true, force: true })
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
      if (url.searchParams.has("row_ordinal") && sections.includes("pg_stat_user_indexes")) {
        ndjson(response, exactIndexRecords())
      } else if (url.searchParams.has("row_ordinal")) {
        ndjson(response, statementRecords(false))
      } else if (sections.includes("pg_stat_statements")) {
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
  const profile = await mkdtemp(join(tmpdir(), "kronika-artifact-browser-"))
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
    await cdp.waitFor(`document.querySelector(".login-card") !== null`, "login form")
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
    assert.doesNotMatch(localClocks.cursor, /GMT|UTC|\.\d{3}/)
    assert.equal(localClocks.cursorSecondary, null)
    assert.equal(localClocks.hour, "01:00–02:00")
    assert.match(localClocks.hourContext, /08\/13\/2026/)
    assert.doesNotMatch(localClocks.hourContext, /GMT|UTC/)
    assert.match(localClocks.updated, /\d{2}:\d{2}:\d{2}/)
    assert.doesNotMatch(localClocks.updated, /GMT|UTC|\.\d{3}/)
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
      currentDay: document.querySelector('.day-cell[data-day="2026-08-13"]')?.getAttribute('aria-pressed'),
      currentHour: document.querySelector('.hour-cell[data-instant="${HOUR}"]')?.getAttribute('aria-pressed'),
      boundaryDayDisabled: document.querySelector('.day-cell[data-day="2026-08-09"]')?.disabled,
      boundaryDayVisible: document.querySelector('.day-cell[data-day="2026-08-09"]')?.getBoundingClientRect().height > 0,
      headerToggle: document.querySelector('[data-testid="hour-popover"] > header button') !== null,
      hourCount: document.querySelectorAll('.hour-cell').length,
      popovers: document.querySelectorAll('[data-testid="hour-popover"]').length,
      separateControls: document.querySelector('[data-testid="hour-popover"]').querySelectorAll('input, select').length,
      unavailableDay: document.querySelector('.day-cell[data-day="2026-08-12"]')?.disabled,
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
        const calendar = document.querySelector('.day-picker').getBoundingClientRect()
        const hours = document.querySelector('.hour-grid').getBoundingClientRect()
        const boundaryDay = document.querySelector('.day-cell[data-day="2026-08-09"]').getBoundingClientRect()
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
    const narrow = await cdp.evaluate(`(() => {
      const calendar = document.querySelector('.day-picker').getBoundingClientRect()
      const hours = document.querySelector('.hour-grid').getBoundingClientRect()
      const popover = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
      return { calendarBottom: calendar.bottom, clientWidth: document.documentElement.clientWidth, hoursTop: hours.top, popoverLeft: popover.left, popoverRight: popover.right, scrollWidth: document.documentElement.scrollWidth }
    })()`)
    assert.ok(narrow.calendarBottom <= narrow.hoursTop, `narrow picker stack: ${JSON.stringify(narrow)}`)
    assert.ok(narrow.popoverLeft >= 0 && narrow.popoverRight <= narrow.clientWidth && narrow.scrollWidth <= narrow.clientWidth, `narrow picker bounds: ${JSON.stringify(narrow)}`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('.hour-cell[data-instant="${HOUR}"]').dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowRight' }))`)
    assert.equal(await cdp.evaluate(`document.activeElement?.dataset.instant`), String(HOUR + HOUR_US))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "picker Escape close")
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`document.documentElement.dataset.theme === ${JSON.stringify(initialTheme)}`, "the initial theme")

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('.day-cell[data-day="2026-08-09"]')?.getBoundingClientRect().height > 0`, "boundary day immediately visible")
    await cdp.evaluate(`document.querySelector('.day-cell[data-day="2026-08-09"]').click()`)
    await cdp.waitFor(`document.querySelector('.day-cell[data-day="2026-08-09"]')?.getAttribute('aria-pressed') === "true"`, "the local August 9 hours")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('.hour-cell').length`), 1)
    await cdp.evaluate(`document.querySelector('.hour-cell[data-instant="${AUGUST_HOUR}"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "23:00–00:00"`, "the exact local boundary hour")
    await cdp.waitFor(`Math.floor(Number(new URLSearchParams(location.search).get("at")) / ${HOUR_US}) * ${HOUR_US} === ${AUGUST_HOUR}`, "the exact boundary address")
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)
    const augustRequest = requests.find(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("from") === String(AUGUST_HOUR))
    assert.notEqual(augustRequest, undefined)
    assert.equal(new URLSearchParams(augustRequest.query).get("to"), String(AUGUST_HOUR + HOUR_US - 1))

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.evaluate(`document.querySelector('.day-cell[data-day="2026-08-13"]').click()`)
    await cdp.waitFor(`document.querySelector('.hour-cell[data-instant="${HOUR + HOUR_US}"]') !== null`, "the recorded August 13 hour")
    await cdp.evaluate(`document.querySelector('.hour-cell[data-instant="${HOUR + HOUR_US}"]').click()`)
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
      context: document.querySelector('.hour-popover > header > span')?.textContent ?? null,
      day: document.querySelector('.day-cell[data-day="2026-08-13"]')?.getAttribute('aria-label'),
      text: document.querySelector('[data-testid="hour-popover"]')?.textContent ?? "",
      zoneLabel: document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent ?? "",
    }))()`)
    assert.equal(russianPicker.context, null)
    assert.match(russianPicker.day, /13\.08\.2026/)
    assert.doesNotMatch(russianPicker.text, /GMT|UTC/)
    assert.equal(russianPicker.zoneLabel, "Локальное время браузера")
    assert.equal(await cdp.evaluate(`document.querySelector('.cursor-time')?.textContent.includes('Отсчёт')`), false)
    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'mouse' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "picker outside close")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)

    await cdp.evaluate(`(() => {
      document.querySelector('[data-testid="locale-ru"]').click()
      const select = document.querySelector('[data-testid="timezone-select"]')
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(select, "utc")
      select.dispatchEvent(new Event("change", { bubbles: true }))
    })()`)
    await cdp.waitFor(`document.documentElement.lang === "ru" && document.querySelector('[data-testid="timezone-select"]')?.value === "utc" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("05:30:00")`, "the UTC re-render")
    const utcClocks = await cdp.evaluate(`(() => ({
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent,
      cursorSecondary: document.querySelector('[data-testid="cursor-time"] small') !== null,
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent,
      hourZoneSuffix: document.querySelector('[data-testid="hour-picker-trigger"] small')?.textContent.includes('UTC') ?? false,
      updated: document.querySelector('[data-testid="updated-time"]')?.textContent ?? "",
      updatedSecondary: document.querySelector('[data-testid="updated-time"] small') !== null,
      zoneLabel: document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent ?? "",
    }))()`)
    assert.equal(utcClocks.zoneLabel, "UTC")
    assert.match(utcClocks.cursor, /05:30:00/)
    assert.match(utcClocks.hour, /05:00–06:00/)
    assert.doesNotMatch(utcClocks.cursor, /GMT|UTC|\.\d{3}/)
    assert.doesNotMatch(utcClocks.hour, /GMT|UTC|\.\d{3}/)
    assert.match(utcClocks.updated, /\d{2}:\d{2}:\d{2}/)
    assert.doesNotMatch(utcClocks.updated, /GMT|UTC|\.\d{3}/)
    assert.equal(utcClocks.cursorSecondary, false)
    assert.equal(utcClocks.hourZoneSuffix, false)
    assert.equal(utcClocks.updatedSecondary, false)
    await cdp.evaluate(`(() => {
      document.querySelector('[data-testid="locale-en"]').click()
      const select = document.querySelector('[data-testid="timezone-select"]')
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(select, "browser")
      select.dispatchEvent(new Event("change", { bubbles: true }))
    })()`)
    await cdp.waitFor(`document.documentElement.lang === "en" && document.querySelector('[data-testid="timezone-select"]')?.value === "browser" && document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent === "Browser local" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("01:30:00")`, "the local-time restore")

    await cdp.evaluate(`([...document.querySelectorAll(".pg-tabs button")].find((button) => button.textContent === "Tables")).click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row').length === 1`, "the relation wire row")
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
    const tableDetail = await cdp.evaluate(`(() => ({
      labels: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dt')].map((label) => label.textContent),
      values: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dd')].map((value) => value.textContent),
    }))()`)
    assert.doesNotMatch(tableDetail.labels.join(" "), /Database ID|Table OID|Index OID/)
    assert.equal(tableDetail.values.includes("42"), false)
    assert.equal(tableDetail.values.includes("73"), false)
    await cdp.evaluate(`document.querySelector(".pg-detail header button").click()`)

    relationMode = "long"
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "Size and buffers")).click()`)
    await cdp.waitFor(`(() => { const node = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); return node !== null && node.scrollWidth > node.clientWidth })()`, "the wide size and buffers table")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] .virtual-body')?.style.height === "4600px"`, "the long virtual relation table")
    const estimate = await cdp.evaluate(`(() => {
      const node = [...document.querySelectorAll('[data-testid="pg-tables-table"] [title]')].find((cell) => cell.title.includes('9,007,199,254,740,993'))
      return node === undefined ? null : { label: node.getAttribute('aria-label'), text: node.textContent, title: node.title }
    })()`)
    assert.deepEqual(estimate, { label: "≈9,007,199,254,740,993 rows", text: "≈9.01E15 rows", title: "≈9,007,199,254,740,993 rows" })
    for (const [width, height, minimumVisible] of [[1920, 1080, 24], [1366, 768, 11], [1024, 768, 10], [1024, 1366, 35]]) {
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
          stickyLeft: sticky.getBoundingClientRect().left,
          tableLeft: bodyRect.left,
          virtualHeight: table.querySelector('.virtual-body').getBoundingClientRect().height,
          visibleRows: [...table.querySelectorAll('.entity-row')].filter((row) => {
            const rect = row.getBoundingClientRect()
            return rect.bottom > bodyRect.top && rect.top < bodyRect.bottom
          }).length,
        }
      })()`)
      assert.ok(initial.bodyScrollWidth > initial.bodyClientWidth, `${width}px wide table: ${JSON.stringify(initial)}`)
      assert.ok(initial.visibleRows >= minimumVisible, `${width}x${height} visible rows: ${JSON.stringify(initial)}`)
      assert.equal(initial.virtualHeight, 4600, `${width}x${height} virtual height`)
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
      assert.ok(end.lastRight <= end.viewportRight + 1 && end.lastRight >= end.viewportRight - 2, `${width}px rightmost column access: ${JSON.stringify(end)}`)
      await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').scrollLeft = 0`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`(() => { const body = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); body.scrollTop = body.scrollHeight })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] .virtual-body')?.style.height === "4715px"`, "the one guarded relation cursor page")
    const cursorPages = requests.filter(({ query }) => new URLSearchParams(query).get("cursor") === "viewport-page-two")
    assert.equal(cursorPages.length, 1, JSON.stringify(cursorPages))
    await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').scrollTop = 0`)
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row')].some((row) => row.getAttribute('aria-label') === 'artifact_db.public.artifact_table')`, "the first virtual relation row")

    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row')].find((row) => row.getAttribute('aria-label') === 'artifact_db.public.artifact_table')).click()`)
    const alignedDetail = await cdp.evaluate(`(() => {
      const layout = document.querySelector('.pg-entity-layout').getBoundingClientRect()
      const detail = document.querySelector('[data-testid="pg-relation-detail"]').getBoundingClientRect()
      return { detailBottom: detail.bottom, detailTop: detail.top, layoutBottom: layout.bottom, layoutTop: layout.top }
    })()`)
    assert.ok(Math.abs(alignedDetail.detailTop - alignedDetail.layoutTop) <= 1 && Math.abs(alignedDetail.detailBottom - alignedDetail.layoutBottom) <= 1, JSON.stringify(alignedDetail))
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the Russian estimate labels")
    const russianEstimate = await cdp.evaluate(`(() => {
      const nodes = [...document.querySelectorAll('[data-testid="pg-relation-detail"] [title]')]
      const exact = nodes.find((node) => node.title.includes('9 007 199 254 740 993'))
      const toast = nodes.find((node) => node.title.includes('713 456'))
      return { exact: exact?.title ?? null, toast: toast?.textContent ?? null, toastExact: toast?.title ?? null }
    })()`)
    assert.deepEqual(russianEstimate, { exact: "≈9 007 199 254 740 993 строки", toast: "≈713 тыс. строк", toastExact: "≈713 456 строк" })
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
      labels: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dt')].map((label) => label.textContent),
      values: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dd')].map((value) => value.textContent),
    }))()`)
    assert.doesNotMatch(indexDetail.labels.join(" "), /Database ID|Table OID|Index OID/)
    for (const oid of ["42", "73", "74"]) assert.equal(indexDetail.values.includes(oid), false)
    await cdp.evaluate(`document.querySelector(".pg-detail header button").click()`)
    relationMode = "short"
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "State")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .virtual-body')?.style.height === "69px"`, "the short relation set")
    const shortTable = await cdp.evaluate(`(() => {
      const body = document.querySelector('[data-testid="pg-indexes-table"] .entity-scroll')
      return { height: body.getBoundingClientRect().height, rows: document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length, virtual: document.querySelector('[data-testid="pg-indexes-table"] .virtual-body').getBoundingClientRect().height }
    })()`)
    assert.equal(shortTable.rows, 3)
    assert.equal(shortTable.virtual, 69)
    assert.ok(shortTable.height >= 100 && shortTable.height <= 112, JSON.stringify(shortTable))

    const beforeOidSearch = requests.length
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "74")
      input.dispatchEvent(new Event("input", { bubbles: true }))
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
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "Состояние")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("363") === true`, "Russian categorical counts")
    const russianCounts = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    assert.doesNotMatch(russianCounts, /(?:363|223|111|0)\/с/)
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const size = await cdp.evaluate(`({ clientWidth: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth })`)
      assert.ok(size.scrollWidth <= size.clientWidth, `${width}px relation overflow: ${JSON.stringify(size)}`)
    }
    await cdp.evaluate(`document.querySelector(".source-tabs button:first-child").click(); document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.querySelector(".section-tabs") !== null`, "the host tabs")
    await cdp.evaluate(`document.querySelector('.section-tabs [role="tab"]:first-child').click()`)
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
    await cdp.waitFor(`document.querySelector(".event-item button") !== null`, "the statement finding")
    await cdp.evaluate(`document.querySelector(".event-item button").click()`)
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
    assert.match(preview.search, /Filter rows by text/)
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
    await cdp.evaluate(`document.querySelector(".pg-detail header button").click(); document.querySelector('[data-testid="entity-context-filter"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent.includes("Loaded 50 of 4,807") === true`, "the paged full statement set")
    await cdp.waitFor(`document.querySelector('[data-testid="table-paging"]') !== null`, "active statement paging")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector(".pg-detail") !== null`, "detail beside active paging")
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const placement = await cdp.evaluate(`(() => {
        const layout = document.querySelector('[data-testid="pg-entity-layout"]')
        const main = layout.querySelector('.pg-entity-main').getBoundingClientRect()
        const table = layout.querySelector('[data-testid="pg-statements-table"]').getBoundingClientRect()
        const paging = layout.querySelector('[data-testid="table-paging"]').getBoundingClientRect()
        const detail = layout.querySelector('[data-testid="pg-detail"]').getBoundingClientRect()
        return {
          detail: { left: detail.left, top: detail.top },
          main: { right: main.right, top: main.top },
          paging: { top: paging.top },
          table: { bottom: table.bottom },
        }
      })()`)
      assert.ok(Math.abs(placement.detail.top - placement.main.top) <= 1, `${width}px detail row: ${JSON.stringify(placement)}`)
      assert.ok(placement.detail.left >= placement.main.right - 1, `${width}px detail column: ${JSON.stringify(placement)}`)
      assert.ok(placement.paging.top >= placement.table.bottom - 1, `${width}px paging below table: ${JSON.stringify(placement)}`)
    }

    const hostClick = await cdp.evaluate(`(() => {
      const button = document.querySelector(".source-tabs button:first-child")
      button.click()
      return button.textContent
    })()`)
    assert.equal(hostClick, "Host")
    await cdp.evaluate(`document.querySelector('.section-tabs [role="tab"]:first-child').click()`)
    await cdp.waitFor(`document.querySelector(".system-console") !== null`, "the System view")
    const system = await cdp.evaluate(`(() => ({
      buttons: [...document.querySelectorAll('[data-testid^="system-metric-"]')].map((button) => [button.dataset.testid, button.getAttribute("aria-pressed")]),
      lane: document.querySelector(".lane-primary")?.textContent ?? null,
      source: document.querySelector(".source-active")?.textContent ?? null,
    }))()`)
    assert.equal(system.source, "Host")
    assert.equal(system.buttons.some(([id, pressed]) => id === "system-metric-health" && pressed === "true"), true, JSON.stringify(system))
    assert.match(system.lane ?? "", /Health/)
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the System resource table")
    await cdp.waitFor(`document.querySelector(".metric-history .uplot-host canvas") !== null`, "the System uPlot chart")
    for (const [width, height] of [[1920, 1080], [1366, 768], [1280, 431], [1024, 768], [1024, 1366]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      const layout = await cdp.evaluate(`document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => {
        window.scrollTo(0, 0)
        const bounds = (node) => {
          const rect = node.getBoundingClientRect()
          return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width }
        }
        const consolePanel = document.querySelector(".system-console")
        const history = document.querySelector(".metric-history")
        const chart = history.querySelector(".uplot-figure")
        const canvas = chart.querySelector("canvas")
        const host = chart.querySelector(".uplot-host")
        const plot = chart.querySelector(".u-over")
        const resource = document.querySelector('[data-testid="use-table"]')
        const timeline = document.querySelector('[data-testid="hour-timeline"]')
        const columns = [...document.querySelectorAll(".metric-column")]
        const columnBottoms = columns.map((column) => Math.max(...[...column.querySelectorAll(".metric-group")].map((group) => group.getBoundingClientRect().bottom)))
        const summaryBottom = Math.max(...columnBottoms)
        const contentBottom = Math.max(summaryBottom, history.getBoundingClientRect().bottom)
        const panels = [document.querySelector(".timeline-shell"), ...document.querySelectorAll(".metric-group"), history, resource]
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
          columnSpread: Math.max(...columnBottoms) - Math.min(...columnBottoms),
          console: bounds(consolePanel),
          contentBottom,
          documentClientWidth: document.documentElement.clientWidth,
          documentScrollWidth: document.documentElement.scrollWidth,
          history: historyBounds,
          historyTail: historyBounds.bottom - chartBounds.bottom,
          host: bounds(host),
          overlaps,
          plot: bounds(plot),
          resource: bounds(resource),
          resourceSeparation: resource.getBoundingClientRect().top - contentBottom,
          timeline: {
            figure: bounds(timeline),
            host: bounds(timeline.querySelector(".uplot-host")),
            plot: bounds(timeline.querySelector(".u-over")),
          },
        })
      }))))`)
      assert.ok(layout.chart.height >= 180 && layout.chart.height <= 220, `${width}x${height} System chart height: ${JSON.stringify(layout)}`)
      assert.ok(layout.host.height >= 150, `${width}x${height} System chart host height: ${JSON.stringify(layout)}`)
      assert.ok(layout.plot.height >= 80, `${width}x${height} System plot height: ${JSON.stringify(layout)}`)
      assert.ok(layout.timeline.figure.height >= 230 && layout.timeline.host.height >= 190 && layout.timeline.plot.height >= 100,
        `${width}x${height} timeline plot height: ${JSON.stringify(layout)}`)
      assert.deepEqual(layout.chartAccess.canvasAriaHidden, "true")
      assert.equal(layout.chartAccess.canvasCount, 1)
      assert.equal(layout.chartAccess.hostRole, "img")
      assert.match(layout.chartAccess.hostLabel, /%/)
      assert.equal(layout.chartAccess.navigator, true)
      assert.ok(layout.chartAccess.summary.length > 0)
      assert.ok(layout.history.height <= 300 && layout.historyTail <= 24, `${width}x${height} compact System history: ${JSON.stringify(layout)}`)
      assert.ok(layout.columnSpread <= 220, `${width}x${height} balanced System summary: ${JSON.stringify(layout)}`)
      assert.ok(Math.abs(layout.console.bottom - layout.contentBottom) <= 1.5, `${width}x${height} content-sized System console: ${JSON.stringify(layout)}`)
      assert.ok(layout.resourceSeparation >= 7 && layout.resourceSeparation <= 10, `${width}x${height} System resource separation: ${JSON.stringify(layout)}`)
      assert.ok(layout.chart.left >= layout.history.left - 1 && layout.chart.right <= layout.history.right + 1
        && layout.chart.top >= layout.history.top - 1 && layout.chart.bottom <= layout.history.bottom + 1,
      `${width}x${height} System chart containment: ${JSON.stringify(layout)}`)
      assert.ok(layout.documentScrollWidth <= layout.documentClientWidth, `${width}x${height} System document overflow: ${JSON.stringify(layout)}`)
      assert.deepEqual(layout.overlaps, [], `${width}x${height} System panel overlaps`)
    }
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
        const state = await cdp.evaluate(`document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => {
          const figure = document.querySelector(".metric-history .uplot-figure")
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
        }))))`)
        assert.equal(state.canvasAriaHidden, "true", `${theme} ${width}px canvas accessibility: ${JSON.stringify(state)}`)
        assert.equal(state.hostRole, "img", `${theme} ${width}px chart role: ${JSON.stringify(state)}`)
        assert.match(state.hostLabel, /%/, `${theme} ${width}px chart unit: ${JSON.stringify(state)}`)
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
    assert.equal(axisText.some((text) => /GMT|UTC|\.\d{3}/.test(text)), false, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => text.includes("%")), true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /^0%?$/.test(text)), true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /^100%?$/.test(text)), true, JSON.stringify(axisText))

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 2, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`(() => {
      const canvas = document.querySelector(".metric-history .uplot-host canvas")
      return canvas !== null && canvas.width / canvas.getBoundingClientRect().width >= 1.9
    })()`, "the DPR 2 chart backing store")
    const dprTwo = await cdp.evaluate(`(() => {
      const canvas = document.querySelector(".metric-history .uplot-host canvas")
      return { ratio: canvas.width / canvas.getBoundingClientRect().width, screen: devicePixelRatio }
    })()`)
    assert.ok(dprTwo.ratio >= 1.9 && dprTwo.ratio <= 2.1, JSON.stringify(dprTwo))
    assert.equal(dprTwo.screen, 2)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`(() => {
      const canvas = document.querySelector(".metric-history .uplot-host canvas")
      const ratio = canvas?.width / canvas?.getBoundingClientRect().width
      return ratio >= 0.95 && ratio <= 1.05
    })()`, "the restored DPR 1 chart backing store")

    const chartRequestsBeforeExpand = requests.filter(({ path }) => path.startsWith("/api/")).length
    await cdp.evaluate(`document.querySelector(".metric-history .chart-expand").click()`)
    await cdp.waitFor(`document.querySelector('.metric-history [role="dialog"][aria-modal="true"].uplot-expanded') !== null`, "the expanded chart dialog")
    const expanded = await cdp.evaluate(`(() => {
      const dialog = document.querySelector('.metric-history [role="dialog"]')
      const rect = dialog.getBoundingClientRect()
      return {
        active: document.activeElement?.className ?? "",
        clientHeight: document.documentElement.clientHeight,
        clientWidth: document.documentElement.clientWidth,
        fullscreen: document.fullscreenElement !== null,
        height: rect.height,
        hostHeight: dialog.querySelector('.uplot-host').getBoundingClientRect().height,
        left: rect.left,
        top: rect.top,
        width: rect.width,
      }
    })()`)
    assert.match(expanded.active, /chart-close/)
    assert.equal(expanded.fullscreen, false)
    assert.ok(Math.abs(expanded.left) <= 1 && Math.abs(expanded.top) <= 1, JSON.stringify(expanded))
    assert.ok(expanded.width >= expanded.clientWidth - 1 && expanded.height >= expanded.clientHeight - 1, JSON.stringify(expanded))
    assert.ok(expanded.hostHeight > 500, JSON.stringify(expanded))
    await cdp.evaluate(`(() => {
      const dialog = document.querySelector('.metric-history [role="dialog"]')
      const last = dialog.querySelector('input.chart-navigator')
      last.focus()
      last.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Tab" }))
    })()`)
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('.metric-history [role="dialog"] .chart-series-labels .help-dot')`), true)
    await cdp.evaluate(`(() => {
      const first = document.querySelector('.metric-history [role="dialog"] .chart-series-labels .help-dot')
      first.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Tab", shiftKey: true }))
    })()`)
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('.metric-history [role="dialog"] input.chart-navigator')`), true)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('.metric-history [role="dialog"]') === null`, "chart dialog Escape close")
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector(".metric-history .chart-expand")`), true)
    await delay(120)
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/")).length, chartRequestsBeforeExpand)

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`(() => {
      const plot = document.querySelector('[data-testid="hour-timeline"] .u-over')
      const bounds = plot.getBoundingClientRect()
      const clientX = bounds.left + (${AT + 3_000_000} - ${HOUR}) / ${HOUR_US} * bounds.width
      const clientY = bounds.top + bounds.height / 2
      plot.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, clientX, clientY }))
      plot.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX, clientY }))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .chart-tooltip') !== null`, "the exact chart tooltip")
    const tooltip = await cdp.evaluate(`(() => {
      const tooltip = document.querySelector('[data-testid="hour-timeline"] .chart-tooltip')
      return {
        primary: tooltip.querySelector("time strong")?.textContent ?? "",
        secondary: tooltip.querySelector("time small")?.textContent ?? "",
        values: [...tooltip.querySelectorAll(":scope > span")].map((node) => node.textContent),
      }
    })()`)
    assert.equal(tooltip.primary, "01:30:00")
    assert.doesNotMatch(tooltip.primary, /GMT|UTC|\.\d{3}/)
    assert.equal(tooltip.secondary, "")
    assert.equal(tooltip.values.length, 2)
    assert.equal(tooltip.values.some((text) => text.includes("82") && text.includes("%")), true, JSON.stringify(tooltip))
    assert.equal(tooltip.values.some((text) => text.includes("64") && text.includes("%")), true, JSON.stringify(tooltip))

    await cdp.evaluate(`(() => {
      const navigator = document.querySelector('[data-testid="hour-timeline"] input.chart-navigator')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(navigator, "3")
      navigator.dispatchEvent(new Event("input", { bubbles: true }))
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}"`, "keyboard sample address")
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
        const clientX = bounds.left + (${target} - ${HOUR}) / ${HOUR_US} * bounds.width
        plot.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, clientX, isPrimary: true, pointerId: 7, pointerType: "mouse" }))
      })()`)
      await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${expected}"`, `pointer snap to ${expected}`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${expected}"`, `pointer exact sample ${expected}`)
    }
    await point(QUARTER + 3_000_000, QUARTER_NEXT)
    await point(QUARTER, QUARTER_PREVIOUS)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="hour-timeline"] .uplot').length`), 1)

    await cdp.evaluate(`(() => {
      document.querySelector('[data-testid="locale-ru"]').click()
      const select = document.querySelector('[data-testid="timezone-select"]')
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(select, "utc")
      select.dispatchEvent(new Event("change", { bubbles: true }))
    })()`)
    await cdp.waitFor(`document.documentElement.lang === "ru" && document.querySelector('[data-testid="timezone-select"]')?.value === "utc" && document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent === "UTC" && document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")?.startsWith("05:14:55;")`, "the chart UTC render")
    const utcSample = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(utcSample, /^05:14:55;/)
    assert.doesNotMatch(utcSample, /GMT|UTC|\.\d{3}/)
    await cdp.evaluate(`(() => {
      document.querySelector('[data-testid="locale-en"]').click()
      const select = document.querySelector('[data-testid="timezone-select"]')
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(select, "browser")
      select.dispatchEvent(new Event("change", { bubbles: true }))
    })()`)
    await cdp.waitFor(`document.documentElement.lang === "en" && document.querySelector('[data-testid="timezone-select"]')?.value === "browser" && document.querySelector('[data-testid="timezone-select"]')?.selectedOptions[0]?.textContent === "Browser local" && document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")?.startsWith("01:14:55;")`, "the chart local-time restore")

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
    await rm(profile, { recursive: true, force: true })
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
  const profile = await mkdtemp(join(tmpdir(), "kronika-session-browser-"))
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
    await cdp.waitFor(`document.querySelector(".login-card") !== null`, "initial login")
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

    socket.close()
    socket = undefined
    await stopBrowser(browser)
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
    await cdp.waitFor(`document.querySelector(".login-card") !== null`, "logout")
    assert.equal(requests.slice(started).filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length, 1)
    started = requests.length
    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector(".login-card") !== null`, "signed-out reload")
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
    await cdp.waitFor(`document.querySelector(".login-card .login-message")?.textContent.includes("session ended") === true`, "one expired-session transition")
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
    await rm(profile, { recursive: true, force: true })
  }
})

test("the slow-query detail keeps readable labels and contained values", { timeout: 60_000 }, async () => {
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
  const profile = await mkdtemp(join(tmpdir(), "kronika-detail-browser-"))
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
    await cdp.waitFor(`document.querySelector(".login-card") !== null`, "login form")
    await submitLogin(cdp)
    await cdp.waitFor(`document.querySelector(".event-item button") !== null`, "the slow-query event")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click(); document.querySelector(".event-item button").click()`)
    await cdp.waitFor(
      `[...document.querySelectorAll(".event-detail dt")].some((label) => label.textContent.trim().toLocaleUpperCase("ru-RU") === "ЗАПИСЬ")`,
      "the resolved slow-query detail",
    )
    await settleLayout(cdp)

    const landscape = await cdp.evaluate(detailGeometryExpression())
    assert.equal(landscape.innerWidth, 1280)
    assert.ok(landscape.scrollWidth <= landscape.clientWidth, JSON.stringify(landscape))
    assert.ok(landscape.sample.label.width >= 160, JSON.stringify(landscape.sample))
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
    assert.match(landscape.numeric[1]?.text ?? "", /3,83[\s\u00a0]?тыс\.\s*мс/)
    assert.match(landscape.numeric[2]?.text ?? "", /7,66[\s\u00a0]?тыс\.\s*мс/)

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
    assert.equal(narrow.numeric.every(({ align }) => align === "left"), true)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await rm(profile, { recursive: true, force: true })
  }
})

test("aggregate relation detail charts exact server history", { timeout: 60_000 }, async () => {
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
  const profile = await mkdtemp(join(tmpdir(), "kronika-relation-chart-browser-"))
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
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.indexes&level=database&pg_lens=state` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length === 1`, "the database index aggregate")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"] .uplot-host canvas') !== null`, "the aggregate history chart")
    await settleLayout(cdp)

    const historyRequests = requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).has("group"))
    assert.equal(historyRequests.length, 1, JSON.stringify(historyRequests))
    const query = new URLSearchParams(historyRequests[0].query)
    assert.equal(query.get("group"), "database")
    assert.equal(query.get("where.datid"), "42")
    assert.equal(query.get("where.schemaname"), null)
    assert.equal(query.get("type_id"), null)
    assert.deepEqual(query.getAll("field"), ["index_count", "invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"])
    const layout = await cdp.evaluate(`(() => {
      const detail = document.querySelector('[data-testid="pg-relation-detail"]')
      const chart = detail.querySelector('.uplot-host')
      const plot = detail.querySelector('.u-over')
      const table = document.querySelector('[data-testid="pg-indexes-table"]')
      const selectors = [...detail.querySelectorAll('.process-history-selector button')]
      return {
        chartWidth: chart.getBoundingClientRect().width,
        detailWidth: detail.getBoundingClientRect().width,
        plotWidth: plot.getBoundingClientRect().width,
        tableWidth: table.getBoundingClientRect().width,
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        selectors: selectors.map((button) => button.textContent),
      }
    })()`)
    assert.ok(layout.chartWidth > 250 && layout.chartWidth <= layout.detailWidth, JSON.stringify(layout))
    assert.ok(layout.plotWidth > 250, JSON.stringify(layout))
    assert.ok(layout.tableWidth >= 500, JSON.stringify(layout))
    assert.equal(layout.overflow, false)
    assert.equal(layout.selectors.length, 6)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-detail"] .chart-expand').click()`)
    await cdp.waitFor(`document.querySelector('[role="dialog"] .uplot-host canvas') !== null`, "the expanded aggregate history")
    assert.ok(await cdp.evaluate(`document.querySelector('[role="dialog"] .uplot-host').getBoundingClientRect().width > 900`))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))`)
    await cdp.waitFor(`document.querySelector('[role="dialog"]') === null`, "the aggregate history close")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await rm(profile, { recursive: true, force: true })
  }
})

test("chart preference and process summary lifecycle work in the production artifact", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  let summaryMode = "initial"
  const heldSummaries = []
  const heldCgroups = []
  let holdCgroups = false
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
  const profile = await mkdtemp(join(tmpdir(), "kronika-chart-browser-"))
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
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.processes&lens=generic` })
    await cdp.waitFor(`document.querySelector('.process-summary > button strong')?.textContent === "719"`, "719 process-summary rows", 15_000)
    await cdp.waitFor(`document.querySelector('.process-summary-history .uplot-host canvas') !== null`, "the process-summary chart")
    await settleLayout(cdp)
    const shownProcessHeight = await cdp.evaluate(`document.querySelector('.process-table .entity-scroll').getBoundingClientRect().height`)
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty') === null`, "all Process charts hidden")
    await settleLayout(cdp)
    const hiddenProcessHeight = await cdp.evaluate(`document.querySelector('.process-table .entity-scroll').getBoundingClientRect().height`)
    assert.ok(hiddenProcessHeight > shownProcessHeight, JSON.stringify({ hiddenProcessHeight, shownProcessHeight }))
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').textContent`), "Show charts")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.process-summary-history .uplot-host canvas') !== null`, "the restored process-summary chart")

    summaryMode = "fail"
    await cdp.evaluate(`document.querySelectorAll('.section-tabs [role="tab"]')[0].click()`)
    await cdp.waitFor(`document.querySelector('.system-console') !== null`, "System before same-hour summary remount")
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => document.querySelector('[data-testid="system-' + section + '"] .entity-row') !== null)`, "the exact collector cgroup rows", 15_000)
    const systemSnapshots = requests.filter(({ path }) => path === `/api/segments/${SEGMENT}/snapshot`).map(({ query }) => new URLSearchParams(query))
    const primarySystem = systemSnapshots.find((query) => query.getAll("section").includes("os_cgroup_context"))
    assert.notEqual(primarySystem, undefined)
    assert.deepEqual(primarySystem.getAll("section").filter((section) => section.startsWith("os_cgroup_") && section !== "os_cgroup_context"), [])
    const expectedCgroups = {
      os_cgroup_cpu: "/collector/cpu",
      os_cgroup_memory: "/collector/memory",
      os_cgroup_io: "/collector/io",
    }
    for (const [section, path] of Object.entries(expectedCgroups)) {
      const matches = systemSnapshots.filter((query) => query.getAll("section").includes(section))
      assert.ok(matches.length >= 1, section)
      for (const query of matches) {
        assert.deepEqual(query.getAll("section"), [section])
        assert.equal(query.get("where.cgroup_path"), path)
        assert.equal(query.get("where.scope"), "3")
      }
    }
    holdCgroups = true
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "the changed System cursor")
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => document.querySelector('[data-testid="system-' + section + '"]') === null)`, "prior cgroup rows cleared while the new key loads")
    await waitForRequests(() => heldCgroups.length === 3)
    const replacementPaths = {
      os_cgroup_cpu: "/collector/cpu-before",
      os_cgroup_memory: "/collector/memory-before",
      os_cgroup_io: "/collector/io-before",
    }
    assert.deepEqual(Object.fromEntries(heldCgroups.map(({ url }) => [url.searchParams.get("section"), url.searchParams.get("where.cgroup_path")])), replacementPaths)
    assert.equal(heldCgroups.every(({ url }) => url.searchParams.get("at") === String(BEFORE_AT) && url.searchParams.get("where.scope") === "3"), true)
    holdCgroups = false
    for (const held of heldCgroups.splice(0)) if (!held.response.destroyed) ndjson(held.response, cgroupSnapshotRecords(held.url))
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => document.querySelector('[data-testid="system-' + section + '"] .entity-row') !== null)`, "the replacement collector cgroup rows", 15_000)
    holdCgroups = true
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowRight" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}"`, "the failed System cursor")
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => document.querySelector('[data-testid="system-' + section + '"]') === null)`, "prior cgroup rows cleared before a failed exact load")
    await waitForRequests(() => heldCgroups.length === 3)
    holdCgroups = false
    for (const held of heldCgroups.splice(0)) {
      if (held.response.destroyed) continue
      held.response.writeHead(200, { "Content-Type": "application/x-ndjson; charset=utf-8" })
      held.response.end("{")
    }
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]') === null && ["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => document.querySelector('[data-testid="system-' + section + '"]') === null)`, "no stale cgroup rows after exact-load failures", 15_000)
    await cdp.evaluate(`document.querySelectorAll('.section-tabs [role="tab"]')[1].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Could not load process totals" && document.querySelector('.process-summary > button strong')?.textContent === "719"`, "same-hour error with retained totals", 15_000)

    summaryMode = "hold"
    await cdp.evaluate(`document.querySelectorAll('.section-tabs [role="tab"]')[0].click()`)
    await cdp.waitFor(`document.querySelector('.system-console') !== null`, "System before held same-hour summary remount")
    await cdp.evaluate(`document.querySelectorAll('.section-tabs [role="tab"]')[1].click()`)
    await waitForRequests(() => heldSummaries.length !== 0)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Loading process totals…" && document.querySelector('.process-summary > button strong')?.textContent === "719"`, "same-hour loading with retained totals", 15_000)
    const sameHourSummaries = heldSummaries.splice(0)
    for (const held of sameHourSummaries) if (!held.destroyed) ndjson(held, processSummaryRecords(HOUR, 2, 720))
    await cdp.waitFor(`document.querySelector('.process-summary > button strong')?.textContent === "720" && document.querySelector('[data-testid="process-summary-status"]') === null`, "same-hour replacement totals", 15_000)

    summaryMode = "hold"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
    await waitForRequests(() => heldSummaries.length !== 0)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Loading process totals…" && document.querySelector('.process-summary > button strong')?.textContent === "—" && document.querySelector('.process-summary-history') === null`, "cleared totals during a cross-hour load", 15_000)
    summaryMode = "good"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('.process-summary > button strong')?.textContent === "721" && document.querySelector('[data-testid="process-summary-status"]') === null`, "replacement totals after the aborted request", 15_000)
    for (const held of heldSummaries) if (!held.destroyed) ndjson(held, processSummaryRecords(HOUR + HOUR_US, 2, 999))
    await delay(100)
    assert.equal(await cdp.evaluate(`document.querySelector('.process-summary > button strong')?.textContent`), "721")

    summaryMode = "fail"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Could not load process totals" && document.querySelector('.process-summary > button strong')?.textContent === "—" && document.querySelector('.process-summary-history') === null`, "cross-hour summary request failure without prior totals", 15_000)
    summaryMode = "empty"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "No data in the selected hour" && document.querySelector('.process-summary > button strong')?.textContent === "—"`, "successful empty process totals", 15_000)
    assert.equal(await cdp.evaluate(`document.querySelector('.process-summary-history') === null`), true)

    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null`, "PostgreSQL navigation")
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Activity")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "the activity table", 15_000)
    await settleLayout(cdp)
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
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.timeline-shell') !== null`, "activity charts restored")
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Tables")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] .entity-row') !== null`, "the relation table", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('.pg-metric-history') !== null`, "the relation history panel")
    await settleLayout(cdp)
    const shownRelationHeight = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').getBoundingClientRect().height`)
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .pg-metric-history') === null`, "all relation charts hidden")
    await settleLayout(cdp)
    const hiddenRelationHeight = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').getBoundingClientRect().height`)
    assert.ok(hiddenRelationHeight > shownRelationHeight + 100, JSON.stringify({ hiddenRelationHeight, shownRelationHeight }))

    await cdp.evaluate(`document.querySelector('.source-tabs button:first-child').click()`)
    await cdp.waitFor(`document.querySelector('.section-tabs [role="tab"]:first-child') !== null`, "Host sections")
    await cdp.evaluate(`document.querySelector('.section-tabs [role="tab"]:first-child').click()`)
    await cdp.waitFor(`document.querySelector('.system-console') !== null`, "System with charts hidden")
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .metric-history, .use-history, .system-entity-history') === null`), true)
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('.process-table') !== null`, "Processes with charts hidden")
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .process-summary-history, .process-history') === null`), true)
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent === "Events")).click()`)
    await cdp.waitFor(`document.querySelector('.events-console') !== null`, "Events with charts hidden")
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty') === null`), true)

    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector('[data-testid="charts-toggle"]')?.textContent === "Show charts" && document.querySelector('.events-console') !== null`, "the persisted hidden preference", 15_000)
    assert.equal(await cdp.evaluate(`localStorage.getItem("kronika.charts")`), "0")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.timeline-shell') !== null`, "charts shown again")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await rm(profile, { recursive: true, force: true })
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
  const profile = await mkdtemp(join(tmpdir(), "kronika-source-browser-"))
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
    await cdp.waitFor(`document.querySelector('.system-console') !== null && document.querySelector('.source-tabs button:first-child')?.getAttribute('aria-current') === "page"`, "the synchronous Host destination", 15_000)
    await cdp.waitFor(`new URL(location.href).searchParams.get('view') === "host.system"`, "the canonical Host address", 15_000)
    const unavailable = await cdp.evaluate(`(() => {
      const sourceButtons = document.querySelectorAll('.source-tabs button')
      return {
        pgDisabled: sourceButtons[1].disabled,
        pgPanels: document.querySelectorAll('.pg-tabs, .pg-overview, [data-testid^="pg-"]').length,
        pgHealth: document.querySelector('.lane-primary')?.textContent.includes('PostgreSQL') ?? false,
        view: new URL(location.href).searchParams.get('view'),
      }
    })()`)
    assert.deepEqual(unavailable, { pgDisabled: true, pgHealth: false, pgPanels: 0, view: "host.system" })
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the host CPU cards", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition"] .u-over') !== null`, "the CPU composition history")
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
    for (const label of ["Host CPU used", "Host logical CPUs", "Host user CPU", "Host system CPU", "Host I/O wait", "Host stolen CPU", "Host idle CPU"]) {
      assert.match(cpuChart.label, new RegExp(label))
      assert.match(cpuChart.tooltip, new RegExp(label))
    }
    assert.equal(await cdp.evaluate(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`), true)
    const firstPageRequests = requests.slice()
    assert.equal(firstPageRequests.some(({ path, query }) => path.includes("/snapshot") && new URLSearchParams(query).getAll("section").some((section) => section.startsWith("pg_"))), false)

    historical = true
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.overview` })
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null && document.querySelectorAll('.source-tabs button')[1]?.getAttribute('aria-current') === "page"`, "the stored PostgreSQL hour", 15_000)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[1].disabled`), false)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await rm(profile, { recursive: true, force: true })
  }
})

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

function sourceTimelineRecords(historical) {
  const sections = [{ logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001", implementation: "linux", source_family: "system", rows: "1", bytes: "128" }]
  if (historical) sections.push({ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001003", implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256" })
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    { record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1), source_families: [{ name: "postgresql", configured: false, present: historical }] },
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
      source_families: [{ name: "postgresql", configured: true, present: true }],
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
      record: "finding", logical_name: "pg_log_slow_queries", kind: "event", type_id: "2004001",
      field_ordinal: 0, row_ordinal: "3", ts: String(AT),
    },
  ]
}

function slowQueryRecords() {
  const columns = ["ts", "pattern", "sample", "count", "max_duration_ms", "total_duration_ms"]
  return [
    layout("2004001", "pg_log_slow_queries", columns),
    row("2004001", "3", [String(AT), SLOW_PATTERN, SLOW_QUERY, 3, 3_831, 7_662]),
  ]
}

function detailGeometryExpression() {
  return `(() => {
    const rows = [...document.querySelectorAll(".event-detail dl > div")]
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
    const numeric = ["ПОВТОРЕНИЯ", "МАКСИМАЛЬНАЯ ДЛИТЕЛЬНОСТЬ, МС", "СУММАРНАЯ ДЛИТЕЛЬНОСТЬ, МС"].map((text) => {
      const row = byLabel(text)
      const output = row.querySelector("dd")
      const rect = output.getBoundingClientRect()
      return { align: getComputedStyle(output).textAlign, height: row.getBoundingClientRect().height, right: rect.right, text: output.textContent.trim() }
    })
    return {
      clientWidth: document.documentElement.clientWidth,
      innerWidth: window.innerWidth,
      list: bounds(document.querySelector(".event-detail dl")),
      numeric,
      pattern: measured("ШАБЛОН"),
      sample: measured("ЗАПИСЬ"),
      scrollWidth: document.documentElement.scrollWidth,
    }
  })()`
}

async function settleLayout(cdp) {
  await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
}

function timelineRecords(hour = HOUR, cgroups = false) {
  const shift = hour - HOUR
  const shifted = (timestamp) => String(timestamp + shift)
  return [
    { record: "hour", from: String(hour), to: String(hour + HOUR_US - 1), available_hours: AVAILABLE_HOURS.map(String) },
    {
      record: "catalog", from: String(hour), to: String(hour + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: true, present: true }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(hour), max_ts: shifted(AFTER_AT),
      sections: [{
        logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001003",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "pg_stat_statements", physical_name: "pg_stat_statements", type_id: "1002003",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "512",
      }, {
        logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "128",
      }, ...(cgroups ? [{
        logical_name: "os_cgroup_context", physical_name: "os_cgroup_context", type_id: "1205001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "128",
      }, {
        logical_name: "os_cgroup_cpu", physical_name: "os_cgroup_cpu", type_id: "1201001",
        implementation: "linux", source_family: "system", rows: "2", bytes: "256",
      }, {
        logical_name: "os_cgroup_memory", physical_name: "os_cgroup_memory", type_id: "1202001",
        implementation: "linux", source_family: "system", rows: "2", bytes: "256",
      }, {
        logical_name: "os_cgroup_io", physical_name: "os_cgroup_io", type_id: "1203001",
        implementation: "linux", source_family: "system", rows: "4", bytes: "512",
      }] : []), {
        logical_name: "pg_stat_user_tables", physical_name: "pg_stat_user_tables", type_id: "1013001",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "pg_stat_user_indexes", physical_name: "pg_stat_user_indexes", type_id: "1014002",
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
    ...(cgroupContext ? [
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
      typeId: "1203001",
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
  return { record: "row", type_id: typeId, ordinal, timestamp: String(timestamp), values }
}

function relationRecords(url, mode) {
  const indexes = url.searchParams.getAll("section").includes("pg_stat_user_indexes")
  const logicalName = indexes ? "pg_stat_user_indexes" : "pg_stat_user_tables"
  const group = url.searchParams.get("group") ?? "object"
  const state = url.searchParams.getAll("field").includes("invalid_count")
  const sized = url.searchParams.getAll("field").includes("reltuples")
  const page = url.searchParams.get("cursor")
  const count = mode === "long" ? page === null ? 200 : 5 : mode === "short" ? 3 : 1
  const offset = page === null ? 0 : 200
  const baseKey = group === "database"
    ? { datid: "42", datname: "artifact_db" }
    : group === "schema"
      ? { datid: "42", datname: "artifact_db", schemaname: "public" }
      : indexes
        ? { datid: "42", datname: "artifact_db", schemaname: "public", relid: "73", relname: "artifact_table", indexrelid: "74", indexrelname: "artifact_index" }
        : { datid: "42", datname: "artifact_db", schemaname: "public", relid: "73", relname: "artifact_table" }
  const columns = indexes
    ? state
      ? group === "object"
        ? [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("indisvalid", "bool", "none"), wire("indisready", "bool", "none"), wire("indisunique", "bool", "none"), wire("indisprimary", "bool", "none")]
        : [wire("index_count"), wire("invalid_count"), wire("unready_count"), wire("unique_count"), wire("primary_count"), wire("exclusion_count")]
      : [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("idx_scan", "number", "per_second")]
    : sized
      ? [wire("tablespace", "text", "none"), wire("main_fork_bytes", "number", "bytes"), wire("toast_bytes", "number", "bytes"), wire("reltuples"), wire("toast_n_live_tup"), wire("toast_n_dead_tup")]
      : [wire("tablespace", "text", "none"), wire("seq_scan", "number", "per_second")]
  const baseValues = indexes
    ? state
      ? group === "object"
        ? { tablespace: "pg_default", amname: "btree", indisvalid: true, indisready: true, indisunique: true, indisprimary: true }
        : { index_count: 363, invalid_count: 0, unready_count: 0, unique_count: 223, primary_count: 111, exclusion_count: 0 }
      : { tablespace: "pg_default", amname: "btree", idx_scan: 3 }
    : sized
      ? { tablespace: "pg_default", main_fork_bytes: 1_048_576, toast_bytes: 131_072, reltuples: "9007199254740993", toast_n_live_tup: "713456", toast_n_dead_tup: "12876" }
      : { tablespace: "pg_default", seq_scan: 3 }
  const rows = Array.from({ length: count }, (_, local) => {
    const index = offset + local
    const key = group !== "object" ? baseKey : indexes
      ? { ...baseKey, indexrelid: String(74 + index), indexrelname: index === 0 ? "artifact_index" : `artifact_index_${index}` }
      : { ...baseKey, relid: String(73 + index), relname: index === 0 ? "artifact_table" : `artifact_table_${index}` }
    return {
      record: "relation", logical_name: logicalName, group, key, values: baseValues,
      sample_from: String(AT - 5_000_000), sample_to: String(AT),
      source: group === "object" ? { segment_id: SEGMENT, type_id: indexes ? "1014002" : "1013001", ordinal: String((indexes ? 8 : 7) + index), timestamp: String(AT) } : null,
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
      key: { datid: "42", datname: "artifact_db" }, values: values(0),
      sample_from: String(BEFORE_AT - 5_000_000), sample_to: String(BEFORE_AT), source: null,
    },
    {
      record: "relation", logical_name: logicalName, group,
      key: { datid: "42", datname: "artifact_db" }, values: values(1),
      sample_from: String(BEFORE_AT), sample_to: String(AT), source: null,
    },
  ]
}

function wire(name, kind = "number", unit = "count") {
  return { name, kind, unit, nullable: true }
}

function exactIndexRecords() {
  const columns = ["ts", "datid", "datname", "schemaname", "relid", "relname", "indexrelid", "indexrelname", "indexdef", "idx_scan"]
  return [{ record: "layout", rates: ["idx_scan"], layout: { type_id: "1014002", logical_name: "pg_stat_user_indexes", columns: columns.map((name) => ({ name })) } }, {
    record: "row", type_id: "1014002", ordinal: "8", timestamp: String(AT),
    values: [String(AT), "42", "artifact_db", "public", "73", "artifact_table", "74", "artifact_index", "CREATE UNIQUE INDEX artifact_index ON public.artifact_table USING btree (id)", 15],
  }]
}

function snapshotRecords() {
  const columns = [
    "ts", "pid", "leader_pid", "datname", "usename", "application_name", "client_addr", "backend_type",
    "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age", "backend_xmin_age",
    "backend_start", "xact_start", "query_start", "state_change",
  ]
  return [
    {
      record: "layout", rates: [],
      layout: { type_id: "1001003", logical_name: "pg_stat_activity", columns: columns.map((name) => ({ name })) },
    },
    {
      record: "row", type_id: "1001003", ordinal: "73", timestamp: String(AT),
      values: [
        String(AT), 4242, null, "operators", "kronika", "artifact-test", "127.0.0.1", "client backend",
        "active", null, null, "select artifact_wire_contract", "991", null, "7",
        String(AT - 60_000_000), String(AT - 30_000_000), String(AT - 5_000_000), String(AT - 1_000_000),
      ],
    },
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
      record: "row", type_id: "1002003", ordinal: String(91 + index), timestamp: String(AT),
      values: [String(AT), String(9_007_199_254_740_991n - BigInt(index)), 10, 20, true, "operators", "reporter", index === 0 ? "select artifact_exact_context" : `select artifact_page_${index}`, 2 + index, 1, 7.5 + index],
    })),
    ...(page ? [{
      record: "snapshot_page", logical_name: "pg_stat_statements", eligible: String(eligible), returned: String(rowCount),
      has_more: hasMore, truncated: hasMore, next_cursor: hasMore ? "next-statement-page" : null, page_size: 200,
      order_by: ["total_exec_time", "calls"], order_direction: "desc", from: String(AT - 5_000_000), to: String(AT),
    }] : []),
  ]
}

function ndjson(response, records) {
  response.writeHead(200, {
    "Cache-Control": "no-store",
    "Content-Type": "application/x-ndjson; charset=utf-8",
  })
  response.end(records.map((record) => JSON.stringify(record)).join("\n") + (records.length === 0 ? "" : "\n"))
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
    if (response.exceptionDetails !== undefined) throw new Error(response.exceptionDetails.text)
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

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}
