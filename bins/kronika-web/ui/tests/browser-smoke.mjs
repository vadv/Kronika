import assert from "node:assert/strict"

const [debugPort, pageUrl] = process.argv.slice(2)
if (debugPort === undefined || pageUrl === undefined) {
  throw new Error("usage: node tests/browser-smoke.mjs DEBUG_PORT PAGE_URL")
}

const requestedUrl = new URL(pageUrl)
const origin = requestedUrl.origin
const reportMode = requestedUrl.protocol === "file:"
const targets = await fetch(`http://127.0.0.1:${debugPort}/json/list`).then((response) => response.json())
const target = targets.find((candidate) => candidate.type === "page")
if (target === undefined) throw new Error("Chromium did not expose a page target")

const socket = new WebSocket(target.webSocketDebuggerUrl)
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true })
  socket.addEventListener("error", reject, { once: true })
})
let sequence = 0
const pending = new Map()
socket.addEventListener("close", () => {
  for (const request of pending.values()) request.reject(new Error("Chromium closed the page connection"))
  pending.clear()
})
const errors = []
const external = []
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data)
  if (message.id !== undefined) {
    const request = pending.get(message.id)
    if (request === undefined) return
    pending.delete(message.id)
    if (message.error === undefined) request.resolve(message.result)
    else request.reject(new Error(`${request.method}: ${message.error.message}`))
    return
  }
  if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text)
  if (message.method === "Runtime.consoleAPICalled" && ["error", "assert"].includes(message.params.type)) {
    errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "))
  }
  if (message.method === "Log.entryAdded" && message.params.entry.level === "error") errors.push(message.params.entry.text)
  if (message.method === "Network.loadingFailed" && message.params.canceled !== true && message.params.errorText !== "net::ERR_ABORTED") errors.push(message.params.errorText)
  if (message.method === "Network.responseReceived" && message.params.response.status >= 400) {
    const response = message.params.response
    const url = new URL(response.url)
    const unsignedBootstrap = response.status === 401 && url.origin === origin && url.pathname === "/auth/session"
    if (!unsignedBootstrap) errors.push(`${response.status}:${response.url}`)
  }
  if (message.method === "Network.requestWillBeSent") {
    const rawUrl = message.params.request.url
    const url = new URL(rawUrl)
    const reportDocument = reportMode
      && url.protocol === "file:"
      && url.pathname === requestedUrl.pathname
    if (!rawUrl.startsWith("data:") && !rawUrl.startsWith("blob:")
        && (reportMode ? !reportDocument : url.origin !== origin)) external.push(rawUrl)
  }
})

function send(method, params = {}) {
  const id = ++sequence
  return new Promise((resolve, reject) => {
    pending.set(id, { method, reject, resolve })
    socket.send(JSON.stringify({ id, method, params }))
  })
}

async function evaluate(expression) {
  const response = await send("Runtime.evaluate", { awaitPromise: true, expression, returnByValue: true, userGesture: true })
  if (response.exceptionDetails !== undefined) throw new Error(response.exceptionDetails.text)
  return response.result.value
}

async function waitFor(expression, description, timeout = 10_000) {
  const started = Date.now()
  while (Date.now() - started < timeout) {
    if (await evaluate(expression)) return
    await new Promise((resolve) => setTimeout(resolve, 40))
  }
  throw new Error(`timed out waiting for ${description}: ${JSON.stringify(errors)}`)
}

await Promise.all([send("Page.enable"), send("Runtime.enable"), send("Network.enable"), send("Log.enable")])
errors.length = 0
external.length = 0
await send("Page.navigate", { url: pageUrl })
await waitFor(`document.readyState === "complete" && (document.querySelector('[data-testid="login-card"]') !== null || document.querySelector('[data-testid="hour-picker-trigger"]') !== null)`, "login or application")
if (await evaluate(`document.querySelector('[data-testid="login-card"]') !== null`)) {
  await evaluate(`(() => {
    const set = (name, stored) => {
      const input = document.querySelector('[name="' + name + '"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, stored)
      input.dispatchEvent(new Event("input", { bubbles: true }))
    }
    set("username", "smoke")
    set("password", "smoke")
    document.querySelector("form").requestSubmit()
  })()`)
}
await waitFor(`document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "application")
if (reportMode) {
  const beforeReload = await evaluate(`location.href`)
  const reportState = await evaluate(`(() => ({
    login: document.querySelector('[data-testid="login-card"]') !== null,
    mcp: document.querySelector('[data-testid="mcp-trigger"]') !== null,
    refresh: document.querySelector('[data-testid="refresh-action"]') !== null,
    logout: document.querySelector('[data-testid="logout-action"]') !== null,
    pathname: location.pathname,
  }))()`)
  assert.equal(reportState.login, false)
  assert.equal(reportState.mcp, false)
  assert.equal(reportState.refresh, false)
  assert.equal(reportState.logout, false)
  assert.equal(reportState.pathname, requestedUrl.pathname)
  await send("Page.reload")
  await waitFor(`document.readyState === "complete" && document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "reloaded report", 30_000)
  await waitFor(`location.href === ${JSON.stringify(beforeReload)}`, "reloaded report address", 30_000)
  assert.equal(await evaluate(`location.href`), beforeReload)
  assert.equal(await evaluate(`location.pathname`), requestedUrl.pathname)
}
await evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
await waitFor(`document.querySelectorAll(".process-table .entity-row").length > 0`, "process rows", reportMode ? 30_000 : 10_000)
await evaluate(`document.querySelector(".process-table .entity-row").click()`)
let result
if (reportMode) {
  // A standalone segment need not have a predecessor-derived process rate,
  // so its real fixture can open Detail without drawing a history line.
  await waitFor(`document.querySelector('[data-testid="process-dock"]') !== null`, "process detail", 30_000)
  result = await evaluate(`(() => ({
    command: document.querySelector('[data-testid="process-cmdline"]')?.textContent ?? "",
    detailRows: document.querySelectorAll('[data-testid="process-dock"] dl > div').length,
    pathname: location.pathname,
    rows: document.querySelectorAll(".process-table .entity-row").length,
  }))()`)
  assert.ok(result.command.length > 0)
  assert.ok(result.detailRows > 0)
  assert.equal(result.pathname, requestedUrl.pathname)
  assert.ok(result.rows > 0)
} else {
  await waitFor(`document.querySelector('[data-testid="process-history"] .uplot-host canvas') !== null`, "process history")
  result = await evaluate(`(() => ({
    backingRatio: (() => {
      const canvas = document.querySelector('[data-testid="process-history"] .uplot-host canvas')
      return canvas.width / canvas.getBoundingClientRect().width
    })(),
    canvasAriaHidden: document.querySelector('[data-testid="process-history"] .uplot-host canvas').getAttribute("aria-hidden"),
    canvases: document.querySelectorAll('[data-testid="process-history"] .uplot-host canvas').length,
    chartHeight: document.querySelector('[data-testid="process-history"] .uplot-figure').getBoundingClientRect().height,
    hostLabel: document.querySelector('[data-testid="process-history"] .uplot-host').getAttribute("aria-label"),
    hostRole: document.querySelector('[data-testid="process-history"] .uplot-host').getAttribute("role"),
    navigators: document.querySelectorAll('[data-testid="process-history"] input.chart-navigator[type="range"]').length,
    rows: document.querySelectorAll(".process-table .entity-row").length,
    summaries: document.querySelectorAll('[data-testid="process-history"] .chart-summary').length,
  }))()`)
  assert.equal(result.chartHeight, 200)
  assert.equal(result.canvasAriaHidden, "true")
  assert.equal(result.canvases, 1)
  assert.equal(result.hostRole, "img")
  assert.ok(result.hostLabel.length > 0)
  assert.equal(result.navigators, 1)
  assert.ok(result.rows > 0)
  assert.equal(result.summaries, 1)
  assert.ok(result.backingRatio >= 1)
}
assert.deepEqual(errors, [])
assert.deepEqual(external, [])
socket.close()
process.stdout.write(`${JSON.stringify({ ...result, externalRequests: external.length, pageErrors: errors.length })}\n`)
