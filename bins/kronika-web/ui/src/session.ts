export type SessionSnapshot = "signed-out" | "signed-in" | "expired"

interface Session {
  readonly authorization: string
}

type SessionListener = () => void

const listeners = new Set<SessionListener>()

let currentSession: Session | null = null
let currentSnapshot: SessionSnapshot = "signed-out"

export function getSessionSnapshot(): SessionSnapshot {
  return currentSnapshot
}

export function subscribeSession(listener: SessionListener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export async function signInBasic(
  user: string,
  password: string,
  signal: AbortSignal,
): Promise<"signed-in" | "invalid"> {
  const candidate: Session = { authorization: basicAuthorization(user, password) }
  const response = await fetch("/api/catalog?from=0&to=0", {
    credentials: "omit",
    headers: {
      Accept: "application/x-ndjson",
      Authorization: candidate.authorization,
    },
    signal,
  })

  await cancelBody(response)
  if (response.status === 401) return "invalid"
  if (!response.ok) throw new Error(`Sign-in probe failed with HTTP ${response.status}`)

  transition(candidate, "signed-in")
  return "signed-in"
}

export async function apiFetch(input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  const captured = currentSession
  if (captured === null) throw new Error("API request requires a signed-in session")

  const headers = typeof Request !== "undefined" && input instanceof Request
    ? new Headers(input.headers)
    : new Headers()
  if (init.headers !== undefined) {
    new Headers(init.headers).forEach((value, name) => headers.set(name, value))
  }
  headers.set("Authorization", captured.authorization)

  const response = await fetch(input, { ...init, credentials: "omit", headers })
  if (response.status === 401 && currentSession === captured) transition(null, "expired")
  return response
}

export function logout(): void {
  transition(null, "signed-out")
}

function transition(session: Session | null, snapshot: SessionSnapshot): void {
  const changed = currentSession !== session || currentSnapshot !== snapshot
  currentSession = session
  currentSnapshot = snapshot
  if (!changed) return
  for (const listener of listeners) listener()
}

function basicAuthorization(user: string, password: string): string {
  const bytes = new TextEncoder().encode(`${user}:${password}`)
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return `Basic ${btoa(binary)}`
}

async function cancelBody(response: Response): Promise<void> {
  try {
    await response.body?.cancel()
  } catch {
    // A consumed or failed response body does not change the authentication result.
  }
}
