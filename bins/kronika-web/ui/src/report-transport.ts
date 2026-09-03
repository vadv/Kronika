interface ReportResult {
  readonly status: number
  readonly code: string | undefined
  readonly parameter: string | undefined
  readonly message: string | undefined
  takeBody(): Uint8Array<ArrayBuffer>
  free(): void
}

interface ReportSession {
  request(path: string, query: string): ReportResult
}

interface ReportRuntime {
  readonly ready: Promise<ReportSession>
}

function runtime(): ReportRuntime {
  const value = (globalThis as { __KRONIKA_REPORT_RUNTIME__?: ReportRuntime }).__KRONIKA_REPORT_RUNTIME__
  if (value === undefined) throw new Error("Kronika report runtime is missing")
  return value
}

export async function reportFetch(input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  const signal = init.signal
  signal?.throwIfAborted()
  const method = init.method ?? (input instanceof Request ? input.method : "GET")
  if (method !== "GET") return new Response(null, { status: 405 })
  const url = new URL(input instanceof Request ? input.url : input, globalThis.location.href)
  const session = await runtime().ready
  signal?.throwIfAborted()
  const result = session.request(url.pathname, url.search.startsWith("?") ? url.search.slice(1) : url.search)
  try {
    const status = result.status
    if (status === 200) {
      const body = result.takeBody()
      signal?.throwIfAborted()
      return new Response(body, {
        status,
        headers: { "Content-Type": "application/x-ndjson" },
      })
    }
    const body = JSON.stringify({
      error: result.code ?? "unreadable",
      ...(result.parameter === undefined ? {} : { parameter: result.parameter }),
      ...(result.message === undefined ? {} : { message: result.message }),
    })
    return new Response(body, {
      status,
      headers: { "Content-Type": "application/json" },
    })
  } finally {
    result.free()
  }
}
