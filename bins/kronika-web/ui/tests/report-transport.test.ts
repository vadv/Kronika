import assert from "node:assert/strict"
import test from "node:test"

import { reportFetch } from "../src/report-transport.ts"

type Result = {
  readonly status: number
  readonly code: string | undefined
  readonly parameter: string | undefined
  readonly message: string | undefined
  takeBody(): Uint8Array<ArrayBuffer>
  free(): void
}

type Session = {
  request(path: string, query: string): Result
}

function installRuntime(session: Session): () => void {
  const location = Object.getOwnPropertyDescriptor(globalThis, "location")
  const runtime = Object.getOwnPropertyDescriptor(globalThis, "__KRONIKA_REPORT_RUNTIME__")
  Object.defineProperty(globalThis, "location", {
    configurable: true,
    value: { href: "file:///tmp/kronika-report.html" },
  })
  Object.defineProperty(globalThis, "__KRONIKA_REPORT_RUNTIME__", {
    configurable: true,
    value: { ready: Promise.resolve(session) },
  })
  return () => {
    if (location === undefined) Reflect.deleteProperty(globalThis, "location")
    else Object.defineProperty(globalThis, "location", location)
    if (runtime === undefined) Reflect.deleteProperty(globalThis, "__KRONIKA_REPORT_RUNTIME__")
    else Object.defineProperty(globalThis, "__KRONIKA_REPORT_RUNTIME__", runtime)
  }
}

test("report transport returns unchanged NDJSON and releases the response", async () => {
  const expected = new TextEncoder().encode('{"record":"hour"}\n')
  let freed = 0
  const restore = installRuntime({
    request(path, query) {
      assert.equal(path, "/api/hour")
      assert.equal(query, "part=base&segments=42")
      return {
        status: 200,
        code: undefined,
        parameter: undefined,
        message: undefined,
        takeBody: () => expected,
        free: () => { freed += 1 },
      }
    },
  })

  try {
    const response = await reportFetch("/api/hour?part=base&segments=42")
    assert.equal(response.status, 200)
    assert.equal(response.headers.get("Content-Type"), "application/x-ndjson")
    assert.deepEqual(new Uint8Array(await response.arrayBuffer()), expected)
    assert.equal(freed, 1)
  } finally {
    restore()
  }
})

test("report transport preserves stable refusal fields", async () => {
  let freed = false
  const restore = installRuntime({
    request() {
      return {
        status: 400,
        code: "bad_parameter",
        parameter: "from",
        message: "invalid parameter from",
        takeBody: () => new Uint8Array(),
        free: () => { freed = true },
      }
    },
  })

  try {
    const response = await reportFetch("/api/hour?from=bad")
    assert.equal(response.status, 400)
    assert.equal(response.headers.get("Content-Type"), "application/json")
    assert.deepEqual(await response.json(), {
      error: "bad_parameter",
      parameter: "from",
      message: "invalid parameter from",
    })
    assert.equal(freed, true)
  } finally {
    restore()
  }
})

test("report transport refuses methods without calling the engine", async () => {
  const response = await reportFetch("/api/hour", { method: "POST" })
  assert.equal(response.status, 405)
})

test("report transport observes an abort after the synchronous query returns", async () => {
  const abort = new AbortController()
  let freed = false
  const restore = installRuntime({
    request() {
      return {
        status: 200,
        code: undefined,
        parameter: undefined,
        message: undefined,
        takeBody() {
          abort.abort()
          return new Uint8Array([1])
        },
        free: () => { freed = true },
      }
    },
  })

  try {
    await assert.rejects(reportFetch("/api/catalog", { signal: abort.signal }), { name: "AbortError" })
    assert.equal(freed, true)
  } finally {
    restore()
  }
})
