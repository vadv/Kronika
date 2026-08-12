import assert from "node:assert/strict"
import test from "node:test"

type SessionModule = typeof import("../src/session.ts")

test("import is inert and Basic sign-in probes once with UTF-8 credentials", async () => {
  const originalFetch = globalThis.fetch
  const calls: Array<{ input: RequestInfo | URL, init?: RequestInit }> = []
  let cancelled = false
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    return {
      body: { cancel: async () => { cancelled = true } },
      ok: true,
      status: 200,
    } as Response
  }

  try {
    const session = await isolatedSession("inert")
    assert.equal(calls.length, 0)

    const signal = new AbortController().signal
    assert.equal(await session.signInBasic("Jörg", "пароль", signal), "signed-in")
    assert.equal(calls.length, 1)
    assert.equal(calls[0]?.input, "/api/catalog?from=0&to=0")
    assert.equal(calls[0]?.init?.credentials, "omit")
    assert.equal(calls[0]?.init?.signal, signal)
    const headers = new Headers(calls[0]?.init?.headers)
    assert.equal(headers.get("Accept"), "application/x-ndjson")
    assert.equal(headers.get("Authorization"), utf8Basic("Jörg", "пароль"))
    assert.equal(cancelled, true)
    assert.equal(session.getSessionSnapshot(), "signed-in")
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a rejected Basic candidate remains absent", async () => {
  const session = await isolatedSession("invalid")
  const originalFetch = globalThis.fetch
  let fetches = 0
  globalThis.fetch = async () => {
    fetches += 1
    return new Response(null, { status: 401 })
  }

  try {
    assert.equal(await session.signInBasic("user", "wrong", new AbortController().signal), "invalid")
    assert.equal(session.getSessionSnapshot(), "signed-out")
    await assert.rejects(session.apiFetch("/api/hour"), /signed-in session/)
    assert.equal(fetches, 1)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("apiFetch preserves caller headers, injects authorization, and logout blocks network", async () => {
  const session = await isolatedSession("injection")
  const originalFetch = globalThis.fetch
  const calls: Array<{ input: RequestInfo | URL, init?: RequestInit }> = []
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    return new Response(null, { status: 204 })
  }

  try {
    await session.signInBasic("operator", "secret", new AbortController().signal)
    await session.apiFetch("/api/hour", {
      credentials: "include",
      headers: { Accept: "application/json", "X-Kronika-Test": "kept" },
    })

    assert.equal(calls.length, 2)
    assert.equal(calls[1]?.input, "/api/hour")
    assert.equal(calls[1]?.init?.credentials, "omit")
    const headers = new Headers(calls[1]?.init?.headers)
    assert.equal(headers.get("Accept"), "application/json")
    assert.equal(headers.get("X-Kronika-Test"), "kept")
    assert.equal(headers.get("Authorization"), utf8Basic("operator", "secret"))

    session.logout()
    assert.equal(session.getSessionSnapshot(), "signed-out")
    await assert.rejects(session.apiFetch("/api/hour"), /signed-in session/)
    assert.equal(calls.length, 2)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a current session 401 expires the session and notifies subscribers", async () => {
  const session = await isolatedSession("expired")
  const originalFetch = globalThis.fetch
  const snapshots: string[] = []
  let fetches = 0
  globalThis.fetch = async () => {
    fetches += 1
    return new Response(null, { status: fetches === 1 ? 204 : 401 })
  }
  const unsubscribe = session.subscribeSession(() => snapshots.push(session.getSessionSnapshot()))

  try {
    await session.signInBasic("operator", "secret", new AbortController().signal)
    assert.equal((await session.apiFetch("/api/hour")).status, 401)
    assert.equal(session.getSessionSnapshot(), "expired")
    assert.deepEqual(snapshots, ["signed-in", "expired"])
    await assert.rejects(session.apiFetch("/api/hour"), /signed-in session/)
    assert.equal(fetches, 2)
  } finally {
    unsubscribe()
    globalThis.fetch = originalFetch
  }
})

test("a delayed old 401 cannot clear a replacement session", async () => {
  const session = await isolatedSession("stale")
  const originalFetch = globalThis.fetch
  let resolveOld: ((response: Response) => void) | undefined
  const oldResponse = new Promise<Response>((resolve) => { resolveOld = resolve })
  const authorizations: Array<string | null> = []
  let fetches = 0
  globalThis.fetch = async (_input, init) => {
    fetches += 1
    authorizations.push(new Headers(init?.headers).get("Authorization"))
    if (fetches === 2) return oldResponse
    return new Response(null, { status: 204 })
  }

  try {
    await session.signInBasic("old", "credential", new AbortController().signal)
    const delayed = session.apiFetch("/api/hour")
    await session.signInBasic("new", "credential", new AbortController().signal)
    resolveOld?.(new Response(null, { status: 401 }))
    assert.equal((await delayed).status, 401)

    assert.equal(session.getSessionSnapshot(), "signed-in")
    await session.apiFetch("/api/hour")
    assert.deepEqual(authorizations, [
      utf8Basic("old", "credential"),
      utf8Basic("old", "credential"),
      utf8Basic("new", "credential"),
      utf8Basic("new", "credential"),
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

async function isolatedSession(name: string): Promise<SessionModule> {
  return import(`../src/session.ts?test=${name}`)
}

function utf8Basic(user: string, password: string): string {
  return `Basic ${Buffer.from(`${user}:${password}`, "utf8").toString("base64")}`
}
