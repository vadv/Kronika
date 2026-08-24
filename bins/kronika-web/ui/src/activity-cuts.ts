import type { ActivityCut } from "./heatmap-product"

export type { ActivityCut } from "./heatmap-product"

export interface ActivityScales {
  readonly blockSize: number | null
  readonly clockTicks: number | null
}

export function cutScale(cut: ActivityCut, scales: ActivityScales): { readonly scale: number; readonly kind: ActivityCut["kind"] } {
  if (cut.scaleBy === "block_size") {
    return scales.blockSize === null || scales.blockSize <= 0
      ? { scale: 1, kind: "count" }
      : { scale: scales.blockSize, kind: cut.kind }
  }
  if (cut.scaleBy === "clock_ticks") {
    return scales.clockTicks === null || scales.clockTicks <= 0
      ? { scale: 1, kind: "count" }
      : { scale: 1 / scales.clockTicks, kind: cut.kind }
  }
  if (cut.scaleBy === "kib") return { scale: 1_024, kind: cut.kind }
  return { scale: 1, kind: cut.kind }
}

export function activityPreview(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 240)
}
