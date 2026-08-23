import type { Cell, DataRow } from "./api"
import { semantic, semanticsOf, type ValueTone } from "./product-semantics"

export type { ValueTone } from "./product-semantics"

const RATE_ZERO_TONE = semantic("value_tone.rate_zero", "rate_zero_tone").policy.tone
const TEXT_TONES = new Map(semanticsOf("text_value_tone").map((definition) => [definition.policy.field, definition.policy]))
const NUMERIC_TONES = new Map(semanticsOf("numeric_value_tone").map((definition) => [definition.policy.field, definition]))

export function semanticValueTone(field: string, cell: Cell, rate = false, row?: DataRow): ValueTone | null {
  const text = typeof cell === "string" ? cell.trim() : null
  const textPolicy = TEXT_TONES.get(field)
  if (textPolicy !== undefined) {
    const reading = text ?? (textPolicy.ascii_values ? asciiChar(cell) : null)
    if (reading !== null) {
      const tone = textPolicy.values[reading]
      if (tone !== undefined) return tone
    }
    if (textPolicy.nonempty_tone !== null && text !== null && text !== "") return textPolicy.nonempty_tone
  }

  const number = numericCell(cell)
  if (number === null) return null
  if (rate && number === 0) return RATE_ZERO_TONE

  const definition = NUMERIC_TONES.get(field)
  if (definition === undefined) return null
  if (definition.policy.active_client_only && row !== undefined && !isActiveClient(row)) return null
  for (const threshold of definition.thresholds) {
    if (threshold.operator === "lt" ? number < threshold.value : number >= threshold.value) return threshold.tone
  }
  return null
}

function isActiveClient(row: DataRow): boolean {
  return textCell(row.values.backend_type) === "client backend" && textCell(row.values.state) === "active"
}

function textCell(cell: Cell | undefined): string | null {
  return typeof cell === "string" || typeof cell === "number" || typeof cell === "boolean"
    ? String(cell).trim()
    : null
}

function asciiChar(cell: Cell): string | null {
  return typeof cell === "number" && Number.isFinite(cell) ? String.fromCharCode(cell) : null
}

function numericCell(cell: Cell): number | null {
  if (typeof cell === "number") return Number.isFinite(cell) ? cell : null
  if (typeof cell !== "string" || cell.trim() === "") return null
  const number = Number(cell)
  return Number.isFinite(number) ? number : null
}
