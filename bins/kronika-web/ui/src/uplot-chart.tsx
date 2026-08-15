import uPlot, { type AlignedData } from "uplot"
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { DisplayTimeFormatter } from "./display-time"
import { useDisplayTime } from "./display-time-context"
import { LabelHelp, type Translate } from "./help"
import type { Locale } from "./model"

export type ChartScale = "percent" | "nonnegative" | "signed"

export interface RecordedPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export interface RecordedSeries {
  readonly color: "cyan" | "amber" | "violet" | "green" | "red" | "gray" | "blue" | "rose"
  readonly id: string
  readonly helpKey: string
  readonly label: string
  readonly labelKey: string
  readonly points: readonly RecordedPoint[]
  readonly scale: ChartScale
  readonly tick?: ((number: number, locale: Locale) => string) | undefined
  readonly unit: string
  readonly value: (number: number, locale: Locale) => string
}

export interface ChartFrame {
  readonly data: AlignedData
  readonly isolated: ReadonlyMap<number, readonly number[]>
  readonly timestamps: readonly number[]
}

export interface ScalePartition {
  readonly key: string
  readonly label: string
  readonly scale: ChartScale
  readonly seriesIds: readonly string[]
  readonly unit: string
}

export interface ChartDecoration {
  readonly from: number
  readonly to: number
  readonly tone: "future" | "unavailable"
}

export interface ChartThreshold {
  readonly below: number
  readonly seriesId: string
}

const NO_DECORATIONS: readonly ChartDecoration[] = []

export function UPlotChart({
  cursor,
  hour,
  locale,
  onCursor,
  reading,
  series,
  className,
  decorations = NO_DECORATIONS,
  markerLayer,
  onPlotWidth,
  referenceTimestamp,
  testId,
  threshold,
  t,
}: {
  readonly className?: string | undefined
  readonly cursor?: number | undefined
  readonly decorations?: readonly ChartDecoration[] | undefined
  readonly hour: number
  readonly locale: Locale
  readonly markerLayer?: ReactNode | undefined
  readonly onCursor?: ((timestamp: number) => void) | undefined
  readonly onPlotWidth?: ((width: number) => void) | undefined
  readonly reading?: string | undefined
  readonly referenceTimestamp?: number | undefined
  readonly series: readonly RecordedSeries[]
  readonly testId?: string | undefined
  readonly threshold?: ChartThreshold | undefined
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const titleId = useId()
  const summaryId = useId()
  const shell = useRef<HTMLElement>(null)
  const host = useRef<HTMLDivElement>(null)
  const plot = useRef<uPlot | null>(null)
  const onCursorRef = useRef(onCursor)
  const onPlotWidthRef = useRef(onPlotWidth)
  const selectedRef = useRef<number | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [hovered, setHovered] = useState<number | null>(null)
  const [keyboardIndex, setKeyboardIndex] = useState(0)
  const opener = useRef<HTMLButtonElement>(null)
  const pagePosition = useRef({ left: 0, top: 0 })
  const returnFocus = useRef(false)
  const end = hour + 3_600_000_000
  const visibleSeries = useMemo(() => series.map((line) => ({
    ...line,
    points: line.points.filter(({ timestamp }) => timestamp >= hour && timestamp < end),
  })), [end, hour, series])
  const frame = useMemo(() => alignRecordedSeries(visibleSeries), [visibleSeries])
  const [themeRevision, setThemeRevision] = useState(0)
  const exact = hovered === null ? null : exactReadings(frame, series, hovered, locale, time)
  const selected = cursor === undefined || cursor < hour || cursor >= end ? null : cursor
  const keyboardTimestamp = frame.timestamps[keyboardIndex] ?? null
  onCursorRef.current = onCursor
  onPlotWidthRef.current = onPlotWidth
  selectedRef.current = selected

  useEffect(() => {
    const element = host.current
    if (element === null || frame.timestamps.length === 0) return
    const initialBounds = element.getBoundingClientRect()
    const options = chartOptions(visibleSeries, frame, hour, end, locale, time, decorations, threshold, selectedRef, referenceTimestamp, Math.max(1, Math.round(initialBounds.width)), Math.max(1, Math.round(initialBounds.height)), (chart) => {
      const index = chart.cursor.idx
      const timestamp = index === null || index === undefined ? null : frame.timestamps[index] ?? null
      setHovered(timestamp)
    }, (chart) => {
      const root = shell.current
      if (root === null) return
      const left = chart.over.offsetLeft
      const top = chart.over.offsetTop
      const width = chart.over.offsetWidth
      const endReserve = Number.parseFloat(getComputedStyle(root).getPropertyValue("--chart-marker-end-reserve")) || 0
      root.style.setProperty("--chart-plot-left", `${element.offsetLeft + left}px`)
      root.style.setProperty("--chart-plot-top", `${element.offsetTop + top}px`)
      root.style.setProperty("--chart-plot-width", `${width}px`)
      onPlotWidthRef.current?.(Math.max(1, width - endReserve))
    })
    const chart = new uPlot(options, frame.data, element)
    plot.current = chart
    const canvas = chart.root.querySelector("canvas")
    canvas?.setAttribute("aria-hidden", "true")
    const select = (event: PointerEvent) => {
      const bounds = chart.over.getBoundingClientRect()
      const timestamp = nearestRecordedTimestamp(frame.timestamps, chart.posToVal(event.clientX - bounds.left, "x"))
      if (timestamp !== null && timestamp !== selectedRef.current) onCursorRef.current?.(timestamp)
    }
    chart.over.addEventListener("pointerup", select)
    let resizeFrame = 0
    const resize = () => {
      cancelAnimationFrame(resizeFrame)
      resizeFrame = requestAnimationFrame(() => {
        const bounds = element.getBoundingClientRect()
        chart.setSize({ width: Math.max(1, Math.round(bounds.width)), height: Math.max(1, Math.round(bounds.height)) })
      })
    }
    resize()
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(resize)
    observer?.observe(element)
    window.addEventListener("resize", resize)
    return () => {
      cancelAnimationFrame(resizeFrame)
      observer?.disconnect()
      window.removeEventListener("resize", resize)
      chart.over.removeEventListener("pointerup", select)
      chart.destroy()
      plot.current = null
    }
  }, [decorations, end, expanded, frame, hour, locale, referenceTimestamp, themeRevision, threshold, time, visibleSeries])

  useEffect(() => {
    const observer = new MutationObserver(() => setThemeRevision((revision) => revision + 1))
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "data-theme"] })
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    plot.current?.redraw()
  }, [expanded, frame, referenceTimestamp, selected])

  useEffect(() => {
    if (selected === null) return
    const nearest = nearestRecordedTimestamp(frame.timestamps, selected)
    const index = nearest === null ? -1 : frame.timestamps.indexOf(nearest)
    if (index >= 0) setKeyboardIndex((current) => current === index ? current : index)
  }, [frame.timestamps, selected])

  useEffect(() => {
    setKeyboardIndex((index) => Math.min(index, Math.max(0, frame.timestamps.length - 1)))
  }, [frame.timestamps.length])

  useEffect(() => {
    if (!expanded) return
    const rootOverflow = document.documentElement.style.overflow
    const bodyOverflow = document.body.style.overflow
    const pageScrollLeft = pagePosition.current.left
    const pageScrollTop = pagePosition.current.top
    const blockPageScroll = (event: Event) => event.preventDefault()
    document.documentElement.style.overflow = "hidden"
    document.body.style.overflow = "hidden"
    opener.current?.focus({ preventScroll: true })
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault()
        collapse()
        return
      }
      if (event.key !== "Tab") return
      const root = shell.current
      if (root === null) return
      const focusable = [...root.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])')]
      const first = focusable[0]
      const last = focusable.at(-1)
      if (first === undefined || last === undefined) return
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    window.addEventListener("keydown", keydown)
    window.addEventListener("touchmove", blockPageScroll, { passive: false })
    window.addEventListener("wheel", blockPageScroll, { passive: false })
    return () => {
      document.documentElement.style.overflow = rootOverflow
      document.body.style.overflow = bodyOverflow
      window.removeEventListener("keydown", keydown)
      window.removeEventListener("touchmove", blockPageScroll)
      window.removeEventListener("wheel", blockPageScroll)
      window.scrollTo(pageScrollLeft, pageScrollTop)
    }
  }, [expanded])

  useLayoutEffect(() => {
    if (expanded || !returnFocus.current) return
    returnFocus.current = false
    opener.current?.focus({ preventScroll: true })
    window.scrollTo(pagePosition.current.left, pagePosition.current.top)
  }, [expanded])

  function collapse() {
    const active = document.activeElement
    if (active instanceof HTMLElement && shell.current?.contains(active)) active.blur()
    returnFocus.current = true
    setExpanded(false)
  }

  function expand() {
    pagePosition.current = { left: window.scrollX, top: window.scrollY }
    setExpanded(true)
  }

  const summary = chartSummary(visibleSeries, frame, hour, end, locale, time)
  return <figure
    aria-labelledby={expanded ? titleId : undefined}
    aria-modal={expanded ? "true" : undefined}
    className={`uplot-figure${className === undefined ? "" : ` ${className}`}${expanded ? " uplot-expanded" : ""}`}
    data-testid={testId}
    ref={shell}
    role={expanded ? "dialog" : undefined}
  >
    <figcaption id={titleId}>
      <span className="chart-series-labels">{series.map((line) => <LabelHelp helpKey={line.helpKey} key={line.id} labelKey={line.labelKey} t={t} />)}</span>
      {reading !== undefined && <strong className="chart-current">{reading}</strong>}
      <button
        aria-label={expanded ? (locale === "ru" ? "Закрыть развёрнутый график" : "Close expanded chart") : (locale === "ru" ? "Развернуть график" : "Expand chart")}
        className="chart-expand"
        onClick={() => expanded ? collapse() : expand()}
        ref={opener}
        type="button"
      >{expanded ? "×" : "↗"}</button>
    </figcaption>
    <p className="chart-summary" id={summaryId}>{summary}</p>
    <div aria-describedby={summaryId} aria-label={series.map(({ label, unit }) => `${label}${unit === "" ? "" : `, ${unit}`}`).join("; ")} className="uplot-host" ref={host} role="img" />
    {markerLayer !== undefined && <div className="chart-marker-track">{markerLayer}</div>}
    {exact !== null && <div aria-hidden="true" className="chart-tooltip">
      <time><strong>{exact.time}</strong></time>
      {exact.values.map(({ label, output, unit }) => <span key={label}>{label}{unit === "" ? "" : ` (${unit})`}<strong>{output}</strong></span>)}
    </div>}
    <input
      aria-label={locale === "ru" ? "Точная запись графика" : "Exact chart sample"}
      aria-valuetext={keyboardTimestamp === null ? undefined : sampleText(series, frame, keyboardTimestamp, locale, time)}
      className="chart-navigator"
      data-recorded-timestamp={keyboardTimestamp ?? undefined}
      disabled={frame.timestamps.length === 0}
      max={Math.max(0, frame.timestamps.length - 1)}
      min="0"
      onChange={(event) => {
        const index = Number(event.currentTarget.value)
        const timestamp = frame.timestamps[index]
        setKeyboardIndex(index)
        if (timestamp !== undefined) {
          setHovered(timestamp)
          if (timestamp !== selectedRef.current) onCursorRef.current?.(timestamp)
        }
      }}
      type="range"
      value={keyboardIndex}
    />
  </figure>
}

export function alignRecordedSeries(series: readonly RecordedSeries[]): ChartFrame {
  const allTimestamps = series.flatMap((line) => line.points.map((point) => point.timestamp))
  const invalid = allTimestamps.find((timestamp) => !Number.isSafeInteger(timestamp))
  if (invalid !== undefined) throw new Error(`invalid chart timestamp ${invalid}`)
  const timestamps = [...new Set(allTimestamps)]
    .sort((left, right) => left - right)
  const positions = new Map(timestamps.map((timestamp, index) => [timestamp, index]))
  const isolated = new Map<number, readonly number[]>()
  const columns = series.map((line, seriesIndex) => {
    const column: (number | null | undefined)[] = Array(timestamps.length).fill(undefined)
    for (const point of line.points.slice().sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId))) {
      const index = positions.get(point.timestamp)
      if (index !== undefined) {
        if (column[index] !== undefined && !Object.is(column[index], point.value)) throw new Error(`conflicting chart sample ${line.id}@${point.timestamp}`)
        column[index] = point.value
      }
    }
    isolated.set(seriesIndex + 1, isolatedSampleIndices(column))
    return column
  })
  return { data: [timestamps, ...columns] as AlignedData, isolated, timestamps }
}

export function isolatedSampleIndices(values: readonly (number | null | undefined)[]): readonly number[] {
  const output: number[] = []
  for (let index = 0; index < values.length; index += 1) {
    if (typeof values[index] !== "number") continue
    let before = index - 1
    while (before >= 0 && values[before] === undefined) before -= 1
    let after = index + 1
    while (after < values.length && values[after] === undefined) after += 1
    if ((before < 0 || values[before] === null) && (after >= values.length || values[after] === null)) output.push(index)
  }
  return output
}

export function exactReadings(frame: ChartFrame, series: readonly RecordedSeries[], timestamp: number, locale: Locale, time: Pick<DisplayTimeFormatter, "axis" | "clock">) {
  const index = frame.timestamps.indexOf(timestamp)
  if (index < 0) return null
  return {
    time: compactChartTime(timestamp, time, chartSecondsUseful(frame.timestamps, time)),
    values: series.map((line, ordinal) => {
      const stored = frame.data[ordinal + 1]?.[index]
      return { label: line.label, output: typeof stored === "number" ? line.value(stored, locale) : "—", unit: line.unit }
    }),
  }
}

export function scaleRange(scale: ChartScale, values: readonly number[]): readonly [number, number] {
  if (scale === "percent") return [0, 100]
  const finite = values.filter(Number.isFinite)
  if (scale === "nonnegative") return [0, niceCeiling(Math.max(0, ...finite))]
  if (finite.length === 0) return [-1, 1]
  const low = Math.min(...finite)
  const high = Math.max(...finite)
  if (low === high) {
    if (low === 0) return [-1, 1]
    return low < 0 ? [-niceCeiling(-low), 0] : [0, niceCeiling(high)]
  }
  return [low < 0 ? -niceCeiling(-low) : 0, high > 0 ? niceCeiling(high) : 0]
}

export function nearestRecordedTimestamp(timestamps: readonly number[], target: number): number | null {
  const first = timestamps[0]
  if (first === undefined) return null
  const last = timestamps.at(-1)!
  if (target <= first) return first
  if (target >= last) return last
  let low = 1
  let high = timestamps.length - 1
  while (low < high) {
    const middle = Math.floor((low + high) / 2)
    if (timestamps[middle]! < target) low = middle + 1
    else high = middle
  }
  const right = timestamps[low]!
  const left = timestamps[low - 1]!
  return target - left <= right - target ? left : right
}

export function scalePartitions(series: readonly RecordedSeries[]): readonly ScalePartition[] {
  const stored = new Map<string, RecordedSeries[]>()
  for (const line of series) {
    const key = scaleKey(line)
    const grouped = stored.get(key) ?? []
    grouped.push(line)
    stored.set(key, grouped)
  }
  return [...stored].map(([key, grouped]) => ({
    key,
    label: grouped.map(({ label }) => label).join(" / "),
    scale: grouped[0]!.scale,
    seriesIds: grouped.map(({ id }) => id),
    unit: grouped[0]!.unit,
  }))
}

function chartOptions(
  series: readonly RecordedSeries[],
  frame: ChartFrame,
  hour: number,
  end: number,
  locale: Locale,
  time: Pick<DisplayTimeFormatter, "axis">,
  decorations: readonly ChartDecoration[],
  threshold: ChartThreshold | undefined,
  selected: { readonly current: number | null },
  referenceTimestamp: number | undefined,
  width: number,
  height: number,
  onHover: (chart: uPlot) => void,
  onGeometry: (chart: uPlot) => void,
): uPlot.Options {
  const styles = getComputedStyle(document.documentElement)
  const color = (name: string) => styles.getPropertyValue(name).trim()
  const partitions = scalePartitions(series)
  const scales = Object.fromEntries(partitions.map(({ key, scale: semantic }) => {
    const grouped = series.flatMap((line, ordinal) => scaleKey(line) === key
      ? Array.from(frame.data[ordinal + 1] ?? []).filter((value): value is number => typeof value === "number")
      : [])
    const [min, max] = scaleRange(semantic, grouped)
    return [key, { auto: false, range: [min, max] }]
  }))
  const decorate = (chart: uPlot) => {
    const context = chart.ctx
    context.save()
    if (threshold !== undefined) {
      const seriesIndex = series.findIndex(({ id }) => id === threshold.seriesId)
      const scale = seriesIndex < 0 ? undefined : scaleKey(series[seriesIndex]!)
      if (scale !== undefined) {
        const boundary = chart.valToPos(threshold.below, scale, true)
        const zero = chart.valToPos(0, scale, true)
        context.fillStyle = color("--chart-threshold")
        context.fillRect(chart.bbox.left, Math.min(boundary, zero), chart.bbox.width, Math.abs(zero - boundary))
      }
    }
    for (const decoration of decorations) {
      const from = chart.valToPos(Math.max(hour, decoration.from), "x", true)
      const to = chart.valToPos(Math.min(end, decoration.to), "x", true)
      context.fillStyle = color(decoration.tone === "future" ? "--chart-future" : "--chart-unavailable")
      context.fillRect(Math.min(from, to), chart.bbox.top, Math.abs(to - from), chart.bbox.height)
    }
    context.restore()
  }
  const drawSelection = (chart: uPlot) => {
    const context = chart.ctx
    const selectedTimestamp = selected.current
    context.save()
    if (referenceTimestamp !== undefined && referenceTimestamp >= hour && referenceTimestamp < end) {
      const x = chart.valToPos(referenceTimestamp, "x", true)
      context.strokeStyle = color("--fg4")
      context.setLineDash([2 * uPlot.pxRatio, 3 * uPlot.pxRatio])
      context.beginPath()
      context.moveTo(x, chart.bbox.top)
      context.lineTo(x, chart.bbox.top + chart.bbox.height)
      context.stroke()
    }
    if (selectedTimestamp !== null) {
      const x = chart.valToPos(selectedTimestamp, "x", true)
      context.strokeStyle = color("--cursor")
      context.setLineDash([3 * uPlot.pxRatio, 3 * uPlot.pxRatio])
      context.beginPath()
      context.moveTo(x, chart.bbox.top)
      context.lineTo(x, chart.bbox.top + chart.bbox.height)
      context.stroke()
      const dataIndex = frame.timestamps.indexOf(selectedTimestamp)
      if (dataIndex >= 0) {
        context.setLineDash([])
        for (let ordinal = 0; ordinal < series.length; ordinal += 1) {
          const number = frame.data[ordinal + 1]?.[dataIndex]
          if (typeof number !== "number") continue
          const y = chart.valToPos(number, scaleKey(series[ordinal]!), true)
          context.fillStyle = color(chartColor(series[ordinal]!.color))
          context.beginPath()
          context.arc(x, y, 3.5 * uPlot.pxRatio, 0, Math.PI * 2)
          context.fill()
        }
      }
    }
    context.restore()
  }
  return {
    width,
    height,
    ms: 1,
    pxAlign: true,
    legend: { show: false },
    scales: { x: { auto: false, range: [hour, end], time: false }, ...scales },
    axes: [
      { scale: "x", side: 2, size: 28, space: (_chart, _axis, _scale, _increment, space) => Math.max(84, space), stroke: color("--fg3"), grid: { stroke: color("--line") }, values: (_chart, splits) => splits.map((timestamp) => axisTimeLabel(timestamp, time)) },
      ...partitions.map(({ key, unit }, axisIndex) => {
        const grouped = series.filter((line) => scaleKey(line) === key)
        const line = grouped[0]!
        return { ...(unit === "" ? {} : { label: unit }), scale: key, side: axisIndex % 2 === 0 ? 3 : 1, size: 62, stroke: color("--fg3"), grid: { stroke: axisIndex === 0 ? color("--line") : "transparent" }, values: (_chart: uPlot, splits: number[]) => splits.map((value) => (line.tick ?? line.value)(value, locale)) }
      }),
    ],
    cursor: {
      dataIdx: (_chart, _seriesIndex, closestIndex) => closestIndex,
      drag: { setScale: false, x: false, y: false },
      move: (chart, left, top) => {
        if (left < 0 || frame.timestamps.length === 0) return [left, top]
        const timestamp = nearestRecordedTimestamp(frame.timestamps, chart.posToVal(left, "x"))
        return timestamp === null ? [left, top] : [chart.valToPos(timestamp, "x"), top]
      },
      points: { size: 6 },
      y: false,
    },
    hooks: { draw: [drawSelection], drawClear: [decorate], ready: [onGeometry], setCursor: [onHover], setSize: [onGeometry] },
    series: [
      { label: "time" },
      ...series.map((line, index) => ({
        label: line.label,
        scale: scaleKey(line),
        stroke: color(chartColor(line.color)),
        width: 1.6,
        points: { filter: [...(frame.isolated.get(index + 1) ?? [])], show: true, size: 5 },
      })),
    ],
  }
}

function chartColor(tone: RecordedSeries["color"]): string {
  if (tone === "cyan") return "--accent"
  if (tone === "amber") return "--warn"
  if (tone === "violet") return "--event"
  if (tone === "green") return "--ok"
  if (tone === "red") return "--bad"
  if (tone === "blue") return "--accent2"
  if (tone === "rose") return "--bad-edge"
  return "--fg3"
}

export function axisTimeLabel(timestamp: number, time: Pick<DisplayTimeFormatter, "axis">): string {
  return compactTimePart(time.axis(timestamp))
}

export function compactChartTime(timestamp: number, time: Pick<DisplayTimeFormatter, "axis" | "clock">, seconds: boolean): string {
  return compactTimePart(seconds ? time.clock(timestamp) : time.axis(timestamp))
}

export function chartSecondsUseful(timestamps: readonly number[], time: Pick<DisplayTimeFormatter, "axis">): boolean {
  const minutes = new Set<string>()
  for (const timestamp of timestamps) {
    const minute = axisTimeLabel(timestamp, time)
    if (minutes.has(minute)) return true
    minutes.add(minute)
  }
  return false
}

function scaleKey(line: RecordedSeries): string {
  return `y:${line.unit || line.id}:${line.scale}`
}

export function chartSummary(series: readonly RecordedSeries[], frame: ChartFrame, hour: number, end: number, locale: Locale, time: Pick<DisplayTimeFormatter, "hourRange">): string {
  const labels = series.map((line, ordinal) => {
    const column = frame.data[ordinal + 1] ?? []
    const values = column.filter((value): value is number => typeof value === "number")
    const nulls = column.filter((value) => value === null).length
    const range = values.length === 0 ? "—" : `${line.value(Math.min(...values), locale)}…${line.value(Math.max(...values), locale)}`
    return locale === "ru"
      ? `${line.label} (${line.unit || "без единицы"}): ${values.length} записей, ${nulls} явных пустых значений, минимум…максимум ${range}`
      : `${line.label} (${line.unit || "unitless"}): ${values.length} samples, ${nulls} explicit nulls, minimum…maximum ${range}`
  }).join("; ")
  return `${time.hourRange(hour).primary}. ${labels}.`
}

export function sampleText(series: readonly RecordedSeries[], frame: ChartFrame, timestamp: number, locale: Locale, time: Pick<DisplayTimeFormatter, "axis" | "clock">): string {
  const exact = exactReadings(frame, series, timestamp, locale, time)
  if (exact === null) return ""
  return `${exact.time}; ${exact.values.map(({ label, output, unit }) => `${label}${unit === "" ? "" : ` (${unit})`}: ${output}`).join("; ")}`
}

function compactTimePart(output: string): string {
  return output.trim().split(/\s+/)[0] ?? output
}

function niceCeiling(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  return (normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10) * magnitude
}
