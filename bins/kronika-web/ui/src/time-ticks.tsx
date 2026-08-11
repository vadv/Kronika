import { formatUtc } from "./model"

const MINUTE_US = 60_000_000

/** Seven labels need the width of the timeline; in the detail dock they run
 *  into each other, and four say the same thing. */
export function TimeTicks({ className, hour, ticks = 6 }: { readonly className: string; readonly hour: number; readonly ticks?: number }) {
  const step = 60 / ticks
  return <div aria-hidden="true" className={`time-ticks ${className}`}>
    {Array.from({ length: ticks + 1 }, (_, tick) => <span
      data-time-tick="true"
      key={tick}
      style={{
        left: `${tick / ticks * 100}%`,
        transform: tick === 0 ? undefined : tick === ticks ? "translateX(-100%)" : "translateX(-50%)",
      }}
    >{formatUtc(hour + tick * step * MINUTE_US).slice(11, 16)}</span>)}
  </div>
}
