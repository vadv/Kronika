import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { parseDictionary, validateDictionaries } from "../scripts/i18n.mjs"

test("flat dictionaries reject duplicates and empty values", () => {
  assert.throws(() => parseDictionary('app.title: "A"\napp.title: "B"', "sample"), /duplicate key/)
  assert.throws(() => parseDictionary('app.title: "  "', "sample"), /nonempty/)
})

test("locale validation checks key and placeholder parity", () => {
  assert.throws(
    () => validateDictionaries({ "app.title": "A" }, { "app.name": "A" }),
    /key mismatch/,
  )
  assert.throws(
    () => validateDictionaries({ "app.title": "At {time}" }, { "app.title": "В {date}" }),
    /placeholder mismatch/,
  )
  assert.deepEqual(
    validateDictionaries({ "app.title": "At {time}" }, { "app.title": "В {time}" }),
    ["app.title"],
  )
})

test("hour empty states are provisional only while the selected hour is open", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)

  assert.equal(english["status.no_data_current"], "No data yet. Refresh may show new data.")
  assert.equal(english["status.no_data_completed"], "No data was recorded in this hour.")
  assert.equal(russian["status.no_data_current"], "Данных пока нет. После обновления они могут появиться.")
  assert.equal(russian["status.no_data_completed"], "За этот час данные не записаны.")
})

test("project dictionaries cover the active UI keys", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)

  const roots = new Set()
  const literalKey = /["']((?:app|nav|section|status|hour|locale|help|common|lens|process|system|lane|locator|table|col|detail|pg|events)\.[a-z0-9_.]+)["']/g
  const sourceFiles = [
    "app.tsx",
    "detail.tsx",
    "events-view.tsx",
    "help.tsx",
    "hour-picker.tsx",
    "postgres-view.tsx",
    "plan-view.tsx",
    "process-table.tsx",
    "system-view.tsx",
    "timeline.tsx",
  ]
  for (const file of sourceFiles) {
    const source = await readFile(new URL(`../src/${file}`, import.meta.url), "utf8")
    for (const match of source.matchAll(literalKey)) roots.add(match[1])
    const dynamicPrefix = file === "system-view.tsx" ? "system.field" : file === "postgres-view.tsx" ? "pg.field" : null
    if (dynamicPrefix !== null) {
      for (const match of source.matchAll(/(?:text|number|id|bytes|milliseconds|timestamp|boolean|rateNumber|rateBytes|rateMilliseconds)\("([a-z0-9_]+)"/g)) {
        roots.add(`${dynamicPrefix}.${match[1]}`)
      }
    }
  }

  const required = new Set([
    ...["host", "processes", "postgresql", "events"].map((name) => `nav.${name}`),
    ...["overview", "cpu", "memory", "storage", "network"].map((name) => `section.${name}`),
    ...["generic", "cpu", "memory", "disk"].map((name) => `lens.${name}`),
    ...["event", "known_bad", "spike"].map((name) => `locator.${name}`),
    ...["overview", "activity", "statements", "plans", "locks", "databases"].map((name) => `pg.section.${name}`),
  ])
  for (const root of roots) {
    if (Object.hasOwn(english, root)) {
      required.add(root)
      continue
    }
    required.add(`${root}.label`)
    if (Object.hasOwn(english, `${root}.help`)) required.add(`${root}.help`)
  }

  const missing = [...required].filter((key) => !Object.hasOwn(english, key)).sort()
  assert.deepEqual(missing, [])
  assert.equal(Object.keys(english).some((key) => key.startsWith("system_table.")), false)
  assert.equal(Object.hasOwn(english, "detail.os_layout.label"), false)
  assert.equal(Object.hasOwn(english, "detail.pg_layout.label"), false)
})

test("plan copy identifies unavailable values and vadv attribution", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")

  assert.equal(english["common.unavailable"], "—")
  assert.equal(russian["common.unavailable"], "—")
  assert.equal(english["pg.section.plans"], "Plans")
  assert.equal(russian["pg.section.plans"], "Plans")
  for (const key of ["nav.host", "nav.processes", "nav.postgresql", "nav.events"]) {
    assert.equal(russian[key], english[key])
  }
  assert.equal(russian["pg.activity.idle"], "Idle")
  assert.equal(russian["pg.field.query_duration_ms.label"], "Query time")
  assert.equal(russian["pg.field.transaction_duration_ms.label"], "Xact time")
  assert.equal(english["pg.wal_storage.label"], "Size of files in pg_wal")
  assert.equal(english["pg.wal_storage.history"], "Size of files in pg_wal over the hour")
  assert.equal(english["pg.wal_storage.help"], "Total size of regular files visible in pg_wal at the cursor.")
  assert.equal(russian["pg.wal_storage.label"], "Размер файлов в pg_wal")
  assert.equal(russian["pg.wal_storage.history"], "Размер файлов в pg_wal за час")
  assert.equal(russian["pg.wal_storage.help"], "Суммарный размер обычных файлов, видимых в pg_wal у курсора.")
  assert.match(english["pg.field.queryid_stat_statements.help"], /vadv-only.*last attributed.*not an exact join key/)
  assert.match(russian["pg.field.queryid_stat_statements.help"], /только в vadv.*последнего запроса.*связанного.*не точный ключ соединения/)
})

test("dense-table help is factual, concise, and complete in both locales", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  const pgFields = new Set([
    "pid", "backend_age_ms", "query_duration_ms", "transaction_duration_ms", "state_duration_ms", "queryid", "planid", "toplevel", "datname", "usename", "query", "plan",
    "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_call", "blocks_per_call", "hit_pct", "wal_per_call",
    "plan_time_pct", "cv", "min_exec_time_ms", "max_exec_time_ms", "mean_exec_time_ms", "stddev_exec_time_ms", "first_call", "last_call",
    "rows_per_second", "planning_ms_per_second", "shared_blks_hit", "shared_blks_read", "shared_blks_written", "shared_blks_dirtied",
    "local_blks_read", "temp_blks_read", "temp_blks_written", "wal_bytes", "queryid_stat_statements", "cmd_type", "numbackends",
    "xact_commit", "xact_rollback", "blks_hit", "blks_read", "tup_returned", "tup_fetched", "tup_inserted", "tup_updated", "tup_deleted",
    "deadlocks", "conflicts", "temp_files", "temp_bytes", "blk_read_time", "blk_write_time", "sessions", "frozen_xid_age",
    "application_name", "state", "wait_event_type", "wait_event", "blocked_by", "lock_locktype", "lock_mode", "lock_target", "lock_relname", "waitstart",
  ])
  const exact = new Set([
    "pg.datname.help", "pg.usename.help", "pg.application_name.help", "pg.client_addr.help", "pg.backend_type.help", "pg.state.help",
    "pg.wait_event_type.help", "pg.wait_event.help", "pg.query.help", "pg.field.statement_database.help", "pg.field.plan_database.help",
    "pg.leader_pid.help", "pg.query_id.help", "pg.backend_xid_age.help", "pg.backend_xmin_age.help", "pg.backend_start.help",
    "pg.xact_start.help", "pg.query_start.help", "pg.state_change.help",
  ])
  const audited = (key) => key.endsWith(".help") && (
    key.startsWith("col.") || key.startsWith("system.field.") || key.startsWith("pg.help.relation.") || key.startsWith("pg.vacuum.")
    || exact.has(key) || key.startsWith("pg.field.") && pgFields.has(key.slice("pg.field.".length, -".help".length))
  )
  const dictionaries = [["en", english], ["ru", russian]]

  for (const [locale, dictionary] of dictionaries) {
    const helpEntries = Object.entries(dictionary).filter(([key]) => audited(key))
    assert.ok(helpEntries.length > 100)
    for (const [key, value] of helpEntries) {
      assert.doesNotMatch(value, locale === "en"
        ? /\b(?:compare|inspect|check|open|recommend|threshold|diagnos|server sort|nulls? last|raw backend)\b/i
        : /\b(?:сопоставьте|проверьте|смотрите|откройте|рекоменду|порог|диагноз|сортировк\S* на сервере|raw backend)\b/iu, `${locale} ${key}`)
      const sentences = value.match(/[.!?](?:\s|$)/g)?.length ?? 0
      assert.ok(sentences >= 1 && sentences <= 2, `${locale} ${key} must contain one or two sentences`)
      assert.ok(value.length <= 360, `${locale} ${key} is too long`)
    }
  }

  assert.match(english["pg.field.calls_per_second.help"], /times per second this statement ran.*call frequency or the weight/i)
  assert.match(english["pg.field.execution_ms_per_second.help"], /wall-clock second.*exceed 1000.*overlap/i)
  assert.match(english["pg.field.mean_exec_ms_per_call.help"], /one execution took on average.*unavailable without calls/i)
  assert.match(english["pg.field.rows_per_second.help"], /rows per second the statement returns or affects/i)
  assert.equal(english["pg.field.statement_database.help"], "Database associated with this physical statement entry.")
  assert.match(russian["pg.field.calls_per_second.help"], /раз в секунду выполнялся.*частота вызовов или тяжесть/i)
})

test("obsolete status and internal collection copy stay out of the UI", async () => {
  const [englishSource, russianSource, appSource, eventsSource, helpSource, processSource, detailSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/events-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/help.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  const removed = [
    "app.kicker", "app.offline", "help.intro", "col.scope.label", "col.scope.help", "col.starttime.label", "col.starttime.help",
    "pg.field.relid.label", "pg.field.indexrelid.label", "pg.relation.scope.database", "pg.relation.scope.schema", "pg.relation.scope.table", "pg.relation.scope.index", "locator.spike.help",
  ]
  for (const key of removed) {
    assert.equal(Object.hasOwn(english, key), false)
    assert.equal(Object.hasOwn(russian, key), false)
  }
  assert.equal(Object.keys(english).some((key) => key.startsWith("system.metric.process_")), false)
  assert.equal(Object.keys(russian).some((key) => key.startsWith("system.metric.process_")), false)

  assert.equal(english["common.raw"], "Copy exact value")
  assert.equal(russian["common.raw"], "Скопировать точное значение")
  assert.equal(english["lens.generic"], "General")
  assert.equal(russian["lens.generic"], "General")
  assert.equal(english["process.summary.running"], "Runnable")
  assert.equal(russian["process.summary.running"], "Runnable")
  assert.equal(english["locator.spike"], "Sharp rise")
  assert.equal(russian["locator.spike"], "Резкий рост")
  assert.doesNotMatch(englishSource, /Local · offline|Hover over|No source row|collection scope/)
  assert.doesNotMatch(russianSource, /Наведите указатель|исходное значение|исходной строки|Область сбора/)
  assert.doesNotMatch(appSource, /\[\.\.\.HELP_SYSTEM, \.\.\.HELP_PROCESS\]/)
  assert.match(eventsSource, /\["all", "event", "known_bad"\]/)
  assert.doesNotMatch(eventsSource, /\["all", "event", "known_bad", "spike"\]/)
  assert.doesNotMatch(helpSource, /help\.intro|help-intro/)
  assert.doesNotMatch(processSource, /col\.scope|idField\("scope"/)
  assert.doesNotMatch(detailSource, /col\.scope|processField\("scope"/)
  assert.doesNotMatch(processSource, /col\.starttime/)
  assert.doesNotMatch(detailSource, /col\.starttime/)
})
