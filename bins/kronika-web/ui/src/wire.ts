export function parseNdjson(body: string, path: string): readonly Record<string, unknown>[] {
  const records = body.split("\n").filter(Boolean).map((line) => JSON.parse(line) as Record<string, unknown>)
  const streamedError = records.find((record) => record.record === "error")
  if (streamedError !== undefined) {
    const code = text(streamedError.error ?? streamedError.code ?? "stream_error")
    const parameter = streamedError.parameter === undefined ? "" : ` parameter=${text(streamedError.parameter)}`
    throw new Error(`${code}${parameter} for ${path}`)
  }
  return records
}

function text(value: unknown): string {
  return typeof value === "string" ? value : String(value)
}
