const HOUR = 3_600_000_000
const MINUTE = 60_000_000

export function moveCursor(cursor: number, hour: number, key: string): number {
  const direction = key === "ArrowLeft" ? -1 : key === "ArrowRight" ? 1 : 0
  if (direction === 0) return cursor
  return Math.max(hour, Math.min(hour + HOUR - 1_000, cursor + direction * MINUTE))
}

export function ownsArrowKeys(tagName: string, contentEditable: boolean): boolean {
  return contentEditable || ["BUTTON", "INPUT", "SELECT", "TEXTAREA"].includes(tagName.toUpperCase())
}

export function keyboardTargetOwnsArrows(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && ownsArrowKeys(target.tagName, target.isContentEditable)
}
