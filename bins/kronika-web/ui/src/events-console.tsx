import { useEffect, useMemo, useRef, useState } from "react"

import { acceptResponse, loadSnapshot, type DataRow } from "./api"
import { DetailList, DetailRow } from "./detail-list"
import { useDisplayTime } from "./display-time-context"
import { EVENT_TIERS, type EventEntry, type EventStat, type EventTier } from "./events-groups"
import { categoryLabel, eventValue } from "./events-format"
import type { Translate } from "./help"
import { compact, humanDuration, rawText, type Locale } from "./model"

const MINUTE_US = 60_000_000
const TIER_ROWS = 100
const RAW_PAGE = 20

const TIER_DOT: Readonly<Record<EventTier, string>> = {
  critical: "bg-bad",
  notable: "bg-warn",
  routine: "bg-fg3",
}

const SECTION_LABELS: Readonly<Record<string, string>> = {
  pg_log_errors: "events.type.errors",
  pg_log_checkpoints: "events.type.checkpoints",
  pg_log_autovacuum: "events.type.autovacuum",
  pg_log_slow_queries: "events.type.slow_queries",
  pg_log_lock_waits: "events.type.lock_waits",
  pg_log_lifecycle: "events.type.lifecycle",
  pgbouncer_events: "events.type.pgbouncer",
}

const GROUPED_NOTES: Readonly<Record<string, string>> = {
  "pg.errors": "events.grouped.errors",
  "pg.slow": "events.grouped.slow",
  "pg.autovacuum": "events.grouped.autovacuum",
  "pg.checkpoints": "events.grouped.checkpoints",
  "pg.checkpoint_warning": "events.grouped.checkpoints",
  "pg.locks": "events.grouped.locks",
  "pg.lifecycle": "events.grouped.lifecycle",
  "pgbouncer.events": "events.grouped.pgbouncer",
}

export function sectionLabel(section: string, t: Translate): string {
  const key = SECTION_LABELS[section]
  return key === undefined ? section : t(key)
}

export function entryTitle(entry: EventEntry, t: Translate, locale: Locale): string {
  const stat = entry.stat
  if (stat.kind === "pg.checkpoints") {
    return t("events.entry.checkpoints", { count: entry.count, timed: stat.timed, requested: stat.requested })
  }
  if (stat.kind === "pg.checkpoint_warning") {
    return t("events.entry.checkpoint_warning", { count: entry.count, seconds: stat.secondsApart ?? "?" })
  }
  if (stat.kind === "pg.locks") {
    const duration = stat.maxMs === null ? "?" : humanDuration(stat.maxMs, locale)
    if (stat.acquired) return t("events.entry.locks_acquired", { waiters: stat.waiters, duration })
    return stat.holders === null
      ? t("events.entry.locks_unknown", { waiters: stat.waiters, duration })
      : t("events.entry.locks", { holders: stat.holders, waiters: stat.waiters, duration })
  }
  if (stat.kind === "pg.lifecycle") {
    if (stat.lifecycle === 0) return t("events.entry.lifecycle.crash", { pid: stat.pid ?? "?", signal: stat.signal ?? "?" })
    if (stat.lifecycle === 1) return t("events.entry.lifecycle.shutdown", { mode: stat.mode ?? "?" })
    return t("events.entry.lifecycle.ready")
  }
  return entry.text ?? ""
}

function entrySubtitle(entry: EventEntry, t: Translate, locale: Locale): string | null {
  const stat = entry.stat
  if (stat.kind === "pg.slow") {
    return t("events.entry.slow.sub", {
      count: entry.count,
      max: humanDuration(stat.maxMs, locale),
      total: humanDuration(stat.totalMs, locale),
    })
  }
  if (stat.kind === "pg.autovacuum") {
    return t("events.entry.autovacuum.sub", {
      runs: stat.runs,
      time: stat.totalMs === null ? "—" : humanDuration(stat.totalMs, locale),
    })
  }
  if (stat.kind === "pg.checkpoints") {
    return t("events.entry.checkpoints.sub", {
      buffers: stat.buffers === null ? "—" : compact(stat.buffers, locale),
      sync: stat.maxSyncMs === null ? "—" : humanDuration(stat.maxSyncMs, locale),
    })
  }
  if (stat.kind === "pg.locks" && stat.targets.length > 0) return stat.targets.join(" · ")
  if (stat.kind === "pg.lifecycle") return entry.text
  return null
}

export function entryChips(entry: EventEntry, t: Translate): readonly string[] {
  const stat = entry.stat
  if (stat.kind === "pg.errors") {
    return [
      t(`events.severity.${["error", "fatal", "panic", "warning", "log"][stat.severity] ?? "log"}`),
      ...(stat.sqlstate === null ? [] : [stat.sqlstate]),
      ...(stat.category === null ? [] : [categoryLabel(stat.category, t)]),
      ...(stat.database === null ? [] : [stat.database]),
    ]
  }
  if (stat.kind === "pg.autovacuum") return [t(stat.analyze ? "events.autovacuum.analyze" : "events.autovacuum.vacuum")]
  if (stat.kind === "pgbouncer.events") {
    return [
      t(`events.pgbouncer.${["fatal", "error", "warning", "log", "debug", "noise"][stat.level] ?? "log"}`),
      ...(stat.database === null ? [] : [stat.database]),
    ]
  }
  return []
}

export function EventStrip({ entry, hour, onCursor, t }: {
  readonly entry: EventEntry
  readonly hour: number
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const peak = Math.max(...entry.minutes, 1)
  const columns = entry.minutes.length
  return <button
    aria-label={t("events.strip")}
    className="block h-[20px] w-[120px] flex-none cursor-pointer border-0 bg-transparent p-0"
    data-testid="event-strip"
    onClick={(event) => {
      event.stopPropagation()
      const bounds = event.currentTarget.getBoundingClientRect()
      const minute = Math.min(columns - 1, Math.max(0, Math.floor(((event.clientX - bounds.left) / bounds.width) * columns)))
      onCursor(hour + minute * MINUTE_US)
    }}
    tabIndex={-1}
    type="button"
  >
    <svg aria-hidden="true" className="block h-full w-full" preserveAspectRatio="none" viewBox={`0 0 ${columns * 2} 20`}>
      <line className="stroke-line2" strokeWidth="1" x1="0" x2={columns * 2} y1="19.5" y2="19.5" />
      {entry.minutes.map((count, minute) => count === 0 ? null : <rect
        className="fill-accent3"
        height={Math.max(2, Math.round((count / peak) * 20))}
        key={minute}
        width="2"
        x={minute * 2}
        y={20 - Math.max(2, Math.round((count / peak) * 20))}
      />)}
    </svg>
  </button>
}

export function EventEntryRow({ entry, expanded, hour, locale, onCursor, onToggle, t }: {
  readonly entry: EventEntry
  readonly expanded: boolean
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onToggle: () => void
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const title = entryTitle(entry, t, locale)
  const subtitle = entrySubtitle(entry, t, locale)
  const chips = entryChips(entry, t)
  const moments = entry.firstTs === entry.lastTs
    ? time.timestamp(entry.firstTs)
    : `${time.timestamp(entry.firstTs)}–${time.timestamp(entry.lastTs)}`
  return <div className="border-b border-line" data-entry-key={entry.key} data-testid="event-entry" data-tier={entry.tier}>
    <button
      aria-expanded={expanded}
      className="grid w-full cursor-pointer grid-cols-[10px_minmax(0,1fr)_auto_120px_auto] items-center gap-2.5 border-0 bg-s1 px-[9px] py-[7px] text-left text-fg2 hover:bg-s3 aria-expanded:bg-s2 max-[760px]:grid-cols-[10px_minmax(0,1fr)_auto]"
      onClick={onToggle}
      type="button"
    >
      <span aria-hidden="true" className={`h-[8px] w-[8px] rounded-full ${TIER_DOT[entry.tier]}`} />
      <span className="min-w-0">
        <span className="flex items-baseline gap-1.5">
          <small className="flex-none text-xs text-fg3">{sectionLabel(entry.section, t)}</small>
          {chips.map((chip) => <small className="flex-none bg-s3 px-1 text-xs text-fg3" key={chip}>{chip}</small>)}
        </span>
        <strong className="mt-[2px] block truncate font-mono text-xs font-medium text-fg" data-testid="event-entry-title">{title}</strong>
        {subtitle !== null && <small className="mt-[2px] block truncate text-xs text-fg3">{subtitle}</small>}
      </span>
      <span className="whitespace-nowrap font-mono text-xs tabular-nums text-fg2">×{compact(entry.count, locale)}</span>
      <span className="max-[760px]:hidden"><EventStrip entry={entry} hour={hour} onCursor={onCursor} t={t} /></span>
      <time className="whitespace-nowrap font-mono text-xs tabular-nums text-fg3 max-[760px]:hidden">{moments}</time>
    </button>
    {expanded && <EventEntryDetail entry={entry} locale={locale} onCursor={onCursor} t={t} />}
  </div>
}

function EventEntryDetail({ entry, locale, onCursor, t }: {
  readonly entry: EventEntry
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const [shown, setShown] = useState(RAW_PAGE)
  useEffect(() => setShown(RAW_PAGE), [entry.key])
  const note = GROUPED_NOTES[entry.stat.kind]
  const facts = entryFacts(entry, locale, t)
  return <div className="border-t border-line2 bg-s2 px-[9px] py-[7px]" data-testid="event-entry-detail">
    {entry.stat.kind === "pg.errors" && <ErrorSample row={entry.rows[0]} t={t} />}
    {entry.stat.kind === "pg.locks" && <LockWaiters entry={entry} locale={locale} t={t} />}
    {facts.length > 0 && <DetailList>{facts.map(([label, shownValue]) => <DetailRow key={label} term={label}>{shownValue}</DetailRow>)}</DetailList>}
    <div className="mt-1.5 grid gap-px">
      {entry.rows.slice(0, shown).map((row) => <RawRow key={`${row.segmentId}:${row.typeId}:${row.ordinal}`} locale={locale} onCursor={onCursor} row={row} stat={entry.stat} t={t} />)}
    </div>
    {entry.rows.length > shown && <button className="mt-1.5 cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4" onClick={() => setShown((current) => current + RAW_PAGE)} type="button">{t("events.raw.more", { count: entry.rows.length - shown })}</button>}
    {note !== undefined && <p className="mt-1.5 text-xs text-fg3">{t(note)}</p>}
  </div>
}

function entryFacts(entry: EventEntry, locale: Locale, t: Translate): readonly (readonly [string, string])[] {
  const stat = entry.stat
  const field = (name: string) => t(`events.field.${name}`)
  if (stat.kind === "pg.errors") {
    return [
      ...(stat.database === null ? [] : [[field("database"), stat.database] as const]),
      ...(stat.username === null ? [] : [[field("username"), stat.username] as const]),
    ]
  }
  if (stat.kind === "pg.slow") {
    return [
      [field("count"), compact(entry.count, locale)],
      [field("max_duration_ms"), humanDuration(stat.maxMs, locale)],
      [field("total_duration_ms"), humanDuration(stat.totalMs, locale)],
    ]
  }
  if (stat.kind === "pg.autovacuum") {
    return [
      ...(stat.totalMs === null ? [] : [[field("elapsed_ms"), humanDuration(stat.totalMs, locale)] as const]),
      ...(stat.tuplesRemoved === null ? [] : [[field("tuples_removed"), compact(stat.tuplesRemoved, locale)] as const]),
      ...(stat.tuplesDead === null ? [] : [[field("tuples_dead_not_removable"), compact(stat.tuplesDead, locale)] as const]),
    ]
  }
  if (stat.kind === "pg.checkpoints") {
    return [
      ...(stat.buffers === null ? [] : [[field("buffers_written"), compact(stat.buffers, locale)] as const]),
      ...(stat.maxSyncMs === null ? [] : [[field("sync_ms"), humanDuration(stat.maxSyncMs, locale)] as const]),
    ]
  }
  if (stat.kind === "pg.checkpoint_warning") {
    return stat.secondsApart === null ? [] : [[field("seconds_apart"), `${compact(stat.secondsApart, locale)} s`]]
  }
  if (stat.kind === "pg.locks") {
    return [
      ...(stat.holders === null ? [] : [[field("holding_pids"), stat.holders] as const]),
      ...(stat.targets.length === 0 ? [] : [[field("lock_target"), stat.targets.join(" · ")] as const]),
    ]
  }
  if (stat.kind === "pg.lifecycle") {
    return [
      ...(stat.pid === null ? [] : [[field("pid"), String(stat.pid)] as const]),
      ...(stat.signal === null ? [] : [[field("signal"), String(stat.signal)] as const]),
      ...(stat.mode === null ? [] : [[field("shutdown_mode"), stat.mode] as const]),
    ]
  }
  return stat.database === null ? [] : [[field("database"), stat.database]]
}

// The per-row hour fetch carries only the sample; the first occurrence's
// larger texts are read once, when the entry is opened.
const ERROR_TEXT_FIELDS = ["detail", "hint", "context", "statement"]

function ErrorSample({ row, t }: { readonly row: DataRow | undefined; readonly t: Translate }) {
  const [texts, setTexts] = useState<DataRow | null>(null)
  useEffect(() => {
    setTexts(null)
    if (row === undefined) return
    const controller = new AbortController()
    acceptResponse(
      loadSnapshot(
        row.segmentId,
        row.timestamp,
        [{ section: row.logicalName, fields: ERROR_TEXT_FIELDS, typeId: row.typeId }],
        controller.signal,
        undefined,
        { typeId: row.typeId, rowOrdinal: row.ordinal, fullText: true },
      ),
      controller.signal,
      (page) => setTexts(page.sections[row.logicalName]?.[0] ?? null),
    )
    return () => controller.abort()
  }, [row])
  if (row === undefined) return null
  const values = { ...row.values, ...(texts?.values ?? {}) }
  const parts = (["sample", "detail", "hint", "context", "statement"] as const)
    .flatMap((field) => {
      const content = rawText(values[field] ?? null)
      return content === null || content === "" ? [] : [[field, content] as const]
    })
  if (parts.length === 0) return null
  return <div className="grid gap-1">
    {parts.map(([field, content]) => <p className="text-xs leading-[1.45] text-fg2 [overflow-wrap:anywhere]" key={field}>
      <span className="text-fg3">{t(`events.field.${field}`)}: </span>
      <span className="font-mono">{content}</span>
    </p>)}
  </div>
}

function LockWaiters({ entry, locale, t }: {
  readonly entry: EventEntry
  readonly locale: Locale
  readonly t: Translate
}) {
  const waiters = useMemo(() => {
    const byPid = new Map<string, { readonly pid: string; maxMs: number | null; mode: string | null; target: string | null; statement: string | null }>()
    for (const row of entry.rows) {
      const pid = rawText(row.values.pid ?? null) ?? "?"
      const duration = row.values.duration_ms
      const ms = typeof duration === "number" ? duration : null
      const current = byPid.get(pid)
      if (current === undefined) {
        byPid.set(pid, {
          pid,
          maxMs: ms,
          mode: rawText(row.values.lock_mode ?? null),
          target: rawText(row.values.lock_target ?? null),
          statement: rawText(row.values.statement ?? null),
        })
      } else {
        if (ms !== null && (current.maxMs === null || ms > current.maxMs)) current.maxMs = ms
        if (current.statement === null) current.statement = rawText(row.values.statement ?? null)
      }
    }
    return [...byPid.values()].sort((left, right) => (right.maxMs ?? 0) - (left.maxMs ?? 0))
  }, [entry.rows])
  return <div className="grid gap-1" data-testid="lock-waiters">
    <p className="text-xs text-fg3">{t("events.holder.missing")}</p>
    {waiters.map((waiter) => <p className="text-xs leading-[1.45] text-fg2 [overflow-wrap:anywhere]" key={waiter.pid}>
      <span className="font-mono tabular-nums">pid {waiter.pid}</span>
      {waiter.mode !== null && <span> · {waiter.mode}</span>}
      {waiter.target !== null && <span> · {waiter.target}</span>}
      {waiter.maxMs !== null && <span> · {humanDuration(waiter.maxMs, locale)}</span>}
      {waiter.statement !== null && <span className="font-mono"> · {waiter.statement}</span>}
    </p>)}
  </div>
}

function RawRow({ locale, onCursor, row, stat, t }: {
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly row: DataRow
  readonly stat: EventStat
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const parts = rawParts(row, stat, locale, t)
  return <button
    className="grid w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)] items-baseline gap-2 border-0 bg-transparent px-1 py-[3px] text-left hover:bg-s3"
    data-testid="event-raw-row"
    onClick={() => onCursor(row.timestamp)}
    type="button"
  >
    <time className="whitespace-nowrap font-mono text-xs tabular-nums text-fg3">{time.timestamp(row.timestamp)}</time>
    <span className="truncate font-mono text-xs text-fg2">{parts.join(" · ")}</span>
  </button>
}

function rawParts(row: DataRow, stat: EventStat, locale: Locale, t: Translate): readonly string[] {
  const cell = (field: string) => row.values[field] ?? null
  const shownText = (field: string) => rawText(cell(field))
  const shownValue = (field: string) => eventValue({ logicalName: row.logicalName }, field, cell(field), locale, t)
  if (stat.kind === "pg.errors") {
    const count = shownText("count")
    return [
      ...(count === null || count === "1" ? [] : [`×${count}`]),
      ...(shownText("sample") === null ? [] : [shownText("sample") ?? ""]),
    ]
  }
  if (stat.kind === "pg.slow") {
    return [`×${shownText("count") ?? "1"}`, shownValue("max_duration_ms")]
  }
  if (stat.kind === "pg.autovacuum") {
    return [
      shownValue("elapsed_ms"),
      ...(shownText("tuples_removed") === null ? [] : [`${t("events.field.tuples_removed")}: ${shownValue("tuples_removed")}`]),
    ]
  }
  if (stat.kind === "pg.checkpoints" || stat.kind === "pg.checkpoint_warning") {
    return [
      shownValue("phase"),
      ...(shownText("reason") === null ? [] : [shownText("reason") ?? ""]),
      ...(shownText("buffers_written") === null ? [] : [`${t("events.field.buffers_written")}: ${shownValue("buffers_written")}`]),
      ...(shownText("sync_ms") === null ? [] : [`sync ${shownValue("sync_ms")}`]),
      ...(shownText("seconds_apart") === null ? [] : [`${shownText("seconds_apart")} s`]),
    ]
  }
  if (stat.kind === "pg.locks") {
    return [
      shownValue("kind"),
      `pid ${shownText("pid") ?? "?"}`,
      ...(shownText("duration_ms") === null ? [] : [shownValue("duration_ms")]),
      ...(shownText("statement") === null ? [] : [shownText("statement") ?? ""]),
    ]
  }
  if (stat.kind === "pg.lifecycle") {
    return [shownText("message") ?? "", ...(shownText("query_detail") === null ? [] : [shownText("query_detail") ?? ""])]
  }
  return [
    ...(shownText("username") === null ? [] : [shownText("username") ?? ""]),
    ...(shownText("host") === null ? [] : [shownText("host") ?? ""]),
  ]
}

export function EventTierSection({ entries, expandedKey, hour, locale, onCursor, onToggle, t, tier }: {
  readonly entries: readonly EventEntry[]
  readonly expandedKey: string | null
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onToggle: (key: string) => void
  readonly t: Translate
  readonly tier: EventTier
}) {
  const [open, setOpen] = useState(tier !== "routine")
  const [shown, setShown] = useState(TIER_ROWS)
  // The last expandedKey this section reacted to; a collapse by hand must not
  // be undone by a refetch, but every new selection must still reveal itself.
  const opened = useRef<string | null>(null)
  useEffect(() => setShown(TIER_ROWS), [hour])
  useEffect(() => {
    if (expandedKey === null || opened.current === expandedKey) return
    const index = entries.findIndex((entry) => entry.key === expandedKey)
    if (index < 0) return
    opened.current = expandedKey
    setOpen(true)
    setShown((current) => index < current ? current : index + 1)
  }, [entries, expandedKey])
  if (entries.length === 0) return null
  const total = entries.reduce((sum, entry) => sum + entry.count, 0)
  return <section data-testid={`event-tier-${tier}`}>
    <button
      aria-expanded={open}
      className="flex w-full cursor-pointer items-center gap-2 border-b border-line2 bg-s2 px-[9px] py-[5px] text-left"
      onClick={() => setOpen((current) => !current)}
      type="button"
    >
      <span aria-hidden="true" className={`h-[8px] w-[8px] rounded-full ${TIER_DOT[tier]}`} />
      <span className="text-xs font-medium text-fg2">{t(`events.tier.${tier}`)}</span>
      <span className="text-xs tabular-nums text-fg3">{t("events.console.count", { groups: entries.length, count: total })}</span>
    </button>
    {open && entries.slice(0, shown).map((entry) => <EventEntryRow
      entry={entry}
      expanded={expandedKey === entry.key}
      hour={hour}
      key={entry.key}
      locale={locale}
      onCursor={onCursor}
      onToggle={() => onToggle(entry.key)}
      t={t}
    />)}
    {open && entries.length > shown && <button
      className="m-1.5 cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4"
      onClick={() => setShown((current) => current + TIER_ROWS)}
      type="button"
    >{t("events.more_groups", { count: entries.length - shown })}</button>}
  </section>
}

export function tiersOf(entries: readonly EventEntry[]): readonly (readonly [EventTier, readonly EventEntry[]])[] {
  return EVENT_TIERS.map((tier) => [tier, entries.filter((entry) => entry.tier === tier)] as const)
}
