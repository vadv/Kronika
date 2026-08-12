import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { mkdtemp, readFile, rm } from "node:fs/promises"
import { createServer } from "node:http"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { gunzipSync } from "node:zlib"

const HOUR = 1_800_000_000_000_000
const AT = HOUR + 1_800_000_000
const SEGMENT = "artifact-wire-segment"
const ARTIFACT = process.env.KRONIKA_UI_ARTIFACT ?? new URL("../kronika-ui.html.gz", import.meta.url)
const BEFORE_AT = AT - 5_000_000
const AFTER_AT = AT + 7_000_000
const QUARTER = HOUR + 900_000_000
const QUARTER_PREVIOUS = QUARTER - 5_000_000
const QUARTER_NEXT = QUARTER + 5_000_000

test("the production artifact preserves wire keys and exact finding page state", { timeout: 30_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  let heldContextPage = null
  let contextPageRequested
  const contextPage = new Promise((resolve) => { contextPageRequested = resolve })
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/api/catalog") {
      requests.push({ authorization: request.headers.authorization, path: url.pathname, query: url.search })
      ndjson(response, [])
      return
    }
    if (url.pathname === "/api/hour") {
      requests.push({ authorization: request.headers.authorization, path: url.pathname, query: url.search })
      ndjson(response, timelineRecords())
      return
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      requests.push({ authorization: request.headers.authorization, path: url.pathname, query: url.search })
      const sections = url.searchParams.getAll("section")
      if (url.searchParams.has("row_ordinal")) {
        ndjson(response, statementRecords(false))
      } else if (sections.includes("pg_stat_statements")) {
        const filtered = ["queryid", "userid", "dbid", "toplevel"].every((field) => url.searchParams.has(`where.${field}`))
        if (filtered && heldContextPage === null) {
          heldContextPage = response
          contextPageRequested()
        } else {
          ndjson(response, statementRecords(true, filtered ? 1 : 0))
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
        errors.push(message.params.entry.text)
      }
      if (message.method === "Network.loadingFailed"
        && message.params.canceled !== true
        && message.params.errorText !== "net::ERR_ABORTED") {
        errors.push(message.params.errorText)
      }
      if (message.method === "Network.responseReceived" && message.params.response.status >= 400) {
        errors.push(`${message.params.response.status}:${message.params.response.url}`)
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
    assert.ok(requests.every(({ authorization }) => authorization === "Basic YXJ0aWZhY3Q6d2lyZQ=="))

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

    ndjson(heldContextPage, statementRecords(true, 1))
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
    const arrow = async (key, expected) => {
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: ${JSON.stringify(key)} }))`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"]')?.getAttribute("aria-valuenow") === "${expected}"`, `${key} to ${expected}`)
    }
    await arrow("ArrowLeft", BEFORE_AT)
    await arrow("ArrowRight", AT)
    await arrow("ArrowRight", AFTER_AT)
    const drag = async (target, expected) => {
      const mapped = await cdp.evaluate(`(() => {
        const plot = document.querySelector(".timeline-plot")
        const slider = document.querySelector('[data-testid="hour-timeline"]')
        const bounds = plot.getBoundingClientRect()
        const clientX = bounds.left + (${target} - ${HOUR}) / 3600000000 * bounds.width
        const mapped = Math.min(${HOUR + 3_600_000_000 - 1_000}, Math.round(${HOUR} + (clientX - bounds.left) / bounds.width * 3600000000))
        slider.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, buttons: 1, clientX, isPrimary: true, pointerId: 7, pointerType: "mouse" }))
        return mapped
      })()`)
      assert.equal(mapped, target)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"]')?.getAttribute("aria-valuenow") === "${expected}"`, `pointer snap to ${expected}`)
    }
    await drag(QUARTER + 3_000_000, QUARTER_NEXT)
    await drag(QUARTER, QUARTER_PREVIOUS)
    assert.deepEqual(errors, [])
    assert.deepEqual(external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await rm(profile, { recursive: true, force: true })
  }
})

function timelineRecords() {
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + 3_600_000_000 - 1), available_hours: [String(HOUR)] },
    {
      record: "catalog", from: String(HOUR), to: String(HOUR + 3_600_000_000 - 1),
      source_families: [{ name: "postgresql", configured: true, present: true }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AFTER_AT),
      sections: [{
        logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001003",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "pg_stat_statements", physical_name: "pg_stat_statements", type_id: "1002003",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "512",
      }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(QUARTER_PREVIOUS), identity: {}, value: 71 },
    { record: "point", type_id: "0", series: "os_health", ts: String(QUARTER_NEXT), identity: {}, value: 73 },
    { record: "point", type_id: "0", series: "os_health", ts: String(BEFORE_AT), identity: {}, value: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(AT), identity: {}, value: 82 },
    { record: "point", type_id: "0", series: "os_health", ts: String(AFTER_AT), identity: {}, value: 84 },
    {
      record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
      field_ordinal: 11, row_ordinal: "91", ts: String(AT),
    },
  ]
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

function statementRecords(page, eligible = 1) {
  const columns = ["ts", "queryid", "userid", "dbid", "toplevel", "datname", "usename", "query", "calls", "rows", "total_exec_time"]
  return [
    {
      record: "layout", rates: ["calls", "rows", "total_exec_time"],
      layout: { type_id: "1002003", logical_name: "pg_stat_statements", columns: columns.map((name) => ({ name })) },
    },
    ...(eligible === 0 ? [] : [{
      record: "row", type_id: "1002003", ordinal: "91", timestamp: String(AT),
      values: [String(AT), "9007199254740991", 10, 20, true, "operators", "reporter", "select artifact_exact_context", 2, 1, 7.5],
    }]),
    ...(page ? [{
      record: "snapshot_page", logical_name: "pg_stat_statements", eligible: String(eligible), returned: String(eligible),
      has_more: false, truncated: false, next_cursor: null, page_size: 200,
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

async function pageSocket(port) {
  const started = Date.now()
  while (Date.now() - started < 5_000) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => response.json())
      const target = targets.find((candidate) => candidate.type === "page")
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
