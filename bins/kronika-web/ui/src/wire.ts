export async function readNdjson(
  response: Response,
  path: string,
  signal: AbortSignal,
): Promise<readonly Record<string, unknown>[]> {
  signal.throwIfAborted()
  if (response.body === null) {
    const records = parseText(await response.text(), path)
    signal.throwIfAborted()
    return records
  }

  const records: Record<string, unknown>[] = []
  const decoder = new TextDecoder()
  const reader = response.body.getReader()
  let pending = ""
  const abort = () => { void reader.cancel(signal.reason).catch(() => undefined) }
  signal.addEventListener("abort", abort, { once: true })

  try {
    while (true) {
      const { done, value } = await reader.read()
      signal.throwIfAborted()
      if (done) break
      pending = consume(`${pending}${decoder.decode(value, { stream: true })}`, records, path)
    }
    pending += decoder.decode()
    if (pending !== "") parseLine(pending, records, path)
  } catch (error) {
    await reader.cancel(error).catch(() => undefined)
    throw error
  } finally {
    signal.removeEventListener("abort", abort)
    reader.releaseLock()
  }

  return records
}

function parseText(body: string, path: string): readonly Record<string, unknown>[] {
  const records: Record<string, unknown>[] = []
  const pending = consume(body, records, path)
  if (pending !== "") parseLine(pending, records, path)
  return records
}

function consume(body: string, records: Record<string, unknown>[], path: string): string {
  let start = 0
  for (let end = body.indexOf("\n"); end !== -1; end = body.indexOf("\n", start)) {
    parseLine(body.slice(start, end), records, path)
    start = end + 1
  }
  return body.slice(start)
}

function parseLine(line: string, records: Record<string, unknown>[], path: string): void {
  const normalized = line.endsWith("\r") ? line.slice(0, -1) : line
  if (normalized === "") return
  const record = JSON.parse(normalized) as Record<string, unknown>
  if (record.record === "error") throw streamError(record, path)
  records.push(record)
}

function streamError(record: Record<string, unknown>, path: string): Error {
  const code = text(record.error ?? record.code ?? "stream_error")
  const parameter = record.parameter === undefined ? "" : ` parameter=${text(record.parameter)}`
  return new Error(`${code}${parameter} for ${path}`)
}

function text(value: unknown): string {
  return typeof value === "string" ? value : String(value)
}
