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
    "process-table.tsx",
    "system-view.tsx",
    "timeline.tsx",
  ]
  for (const file of sourceFiles) {
    const source = await readFile(new URL(`../src/${file}`, import.meta.url), "utf8")
    for (const match of source.matchAll(literalKey)) roots.add(match[1])
    const dynamicPrefix = file === "system-view.tsx" ? "system.field" : file === "postgres-view.tsx" ? "pg.field" : null
    if (dynamicPrefix !== null) {
      for (const match of source.matchAll(/(?:text|number|id|bytes|milliseconds|timestamp|boolean)\("([a-z0-9_]+)"/g)) {
        roots.add(`${dynamicPrefix}.${match[1]}`)
      }
    }
  }

  const required = new Set([
    ...["host", "postgresql", "events"].map((name) => `nav.${name}`),
    ...["system", "processes"].map((name) => `section.${name}`),
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
    required.add(`${root}.help`)
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
  assert.equal(russian["pg.section.plans"], "Планы")
  assert.equal(russian["pg.activity.idle"], "Простой")
  assert.equal(russian["pg.field.query_duration_ms.label"], "Время запроса")
  assert.equal(russian["pg.field.transaction_duration_ms.label"], "Время транзакции")
  assert.equal(english["pg.wal_storage.label"], "Size of files in pg_wal")
  assert.equal(english["pg.wal_storage.history"], "Size of files in pg_wal over the hour")
  assert.equal(english["pg.wal_storage.help"], "Total size of regular files visible in pg_wal at the selected snapshot.")
  assert.equal(russian["pg.wal_storage.label"], "Размер файлов в pg_wal")
  assert.equal(russian["pg.wal_storage.history"], "Размер файлов в pg_wal за час")
  assert.equal(russian["pg.wal_storage.help"], "Суммарный размер обычных файлов, видимых в pg_wal на выбранном снимке.")
  assert.match(english["pg.field.queryid_stat_statements.help"], /vadv-only.*last attributed.*not an exact join key/)
  assert.match(russian["pg.field.queryid_stat_statements.help"], /только в vadv.*последнего связанного.*не точный ключ соединения/)
})

test("help copy is concise and directs the reader to adjacent data", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const dictionaries = [
    ["en", parseDictionary(englishSource, "en.yaml"), /(?:Compare|Inspect|Check|Open|Find|Use)/],
    ["ru", parseDictionary(russianSource, "ru.yaml"), /(?:Сопоставьте|Проверьте|Смотрите|Откройте|Найдите|Сравните|Используйте)/],
  ]

  for (const [locale, dictionary, action] of dictionaries) {
    const helpEntries = Object.entries(dictionary).filter(([key]) => key.endsWith(".help") && key !== "pg.wal_storage.help")
    assert.ok(helpEntries.length > 100)
    for (const [key, value] of helpEntries) {
      const actionAt = value.search(action)
      assert.ok(actionAt > 0, `${locale} ${key} must direct the reader to related data`)
      assert.ok(value.slice(0, actionAt).includes("."), `${locale} ${key} must define the value before the action`)
      assert.ok(value.length <= 200, `${locale} ${key} is too long`)
    }
  }
})

test("obsolete status and internal collection copy stay out of the UI", async () => {
  const [englishSource, russianSource, appSource, helpSource, processSource, detailSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/help.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  const removed = [
    "app.kicker", "app.offline", "help.intro", "col.scope.label", "col.scope.help", "col.starttime.label", "col.starttime.help",
    "pg.field.relid.label", "pg.field.indexrelid.label", "pg.relation.scope.database", "pg.relation.scope.schema", "pg.relation.scope.table", "pg.relation.scope.index",
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
  assert.equal(russian["lens.generic"], "Основное")
  assert.equal(english["process.summary.running"], "Runnable")
  assert.equal(russian["process.summary.running"], "Готовы к выполнению")
  assert.doesNotMatch(englishSource, /Local · offline|Hover over|No source row|collection scope/)
  assert.doesNotMatch(russianSource, /Наведите указатель|исходное значение|исходной строки|Область сбора/)
  assert.doesNotMatch(appSource, /\[\.\.\.HELP_SYSTEM, \.\.\.HELP_PROCESS\]/)
  assert.doesNotMatch(helpSource, /help\.intro|help-intro/)
  assert.doesNotMatch(processSource, /col\.scope|idField\("scope"/)
  assert.doesNotMatch(detailSource, /col\.scope|processField\("scope"/)
  assert.doesNotMatch(processSource, /col\.starttime/)
  assert.doesNotMatch(detailSource, /col\.starttime/)
})
