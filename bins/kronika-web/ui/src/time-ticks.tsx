import { formatUtc } from "./model"

const TEN_MINUTES_US = 600_000_000

export function TimeTicks({ className, hour }: { readonly className: string; readonly hour: number }) {
  return <div aria-hidden="true" className={`time-ticks ${className}`}>
    {Array.from({ length: 7 }, (_, tick) => <span
      data-time-tick="true"
      key={tick}
      style={{
        left: `${tick / 6 * 100}%`,
        transform: tick === 0 ? undefined : tick === 6 ? "translateX(-100%)" : "translateX(-50%)",
      }}
    >{formatUtc(hour + tick * TEN_MINUTES_US).slice(11, 16)}</span>)}
  </div>
}
