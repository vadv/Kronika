export function orderedRecordedTimes(timestamps: readonly number[]): readonly number[] {
  return [...new Set(timestamps.filter(Number.isSafeInteger))].sort((left, right) => left - right)
}

export function moveCursor(cursor: number, timestamps: readonly number[], key: string): number {
  const ordered = orderedRecordedTimes(timestamps)
  if (ordered.length === 0 || (key !== "ArrowLeft" && key !== "ArrowRight")) return cursor
  if (key === "ArrowLeft") {
    for (let index = ordered.length - 1; index >= 0; index -= 1) {
      const timestamp = ordered[index]
      if (timestamp !== undefined && timestamp < cursor) return timestamp
    }
    return cursor
  }
  for (const timestamp of ordered) if (timestamp > cursor) return timestamp
  return cursor
}

export function nearestRecordedTime(timestamps: readonly number[], target: number): number | null {
  const ordered = orderedRecordedTimes(timestamps)
  let closest = ordered[0]
  if (closest === undefined) return null
  for (const timestamp of ordered.slice(1)) {
    if (Math.abs(timestamp - target) < Math.abs(closest - target)) closest = timestamp
  }
  return closest
}

export function ownsArrowKeys(tagName: string, contentEditable: boolean): boolean {
  return contentEditable || ["BUTTON", "INPUT", "SELECT", "TEXTAREA"].includes(tagName.toUpperCase())
}

export function keyboardTargetOwnsArrows(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && ownsArrowKeys(target.tagName, target.isContentEditable)
}
