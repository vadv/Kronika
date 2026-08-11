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

test("project dictionaries cover the active UI keys", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)

  const roots = new Set()
  const literalKey = /["']((?:app|nav|section|status|hour|locale|help|common|lens|system|lane|locator|table|col|detail|pg|events)\.[a-z0-9_.]+)["']/g
  const sourceFiles = [
    "app.tsx",
    "detail.tsx",
    "events-view.tsx",
    "help.tsx",
    "postgres-view.tsx",
    "process-table.tsx",
    "system-view.tsx",
    "timeline.tsx",
  ]
  for (const file of sourceFiles) {
    const source = await readFile(new URL(`../src/${file}`, import.meta.url), "utf8")
    for (const match of source.matchAll(literalKey)) roots.add(match[1])
  }

  const required = new Set([
    ...["host", "postgresql", "events"].map((name) => `nav.${name}`),
    ...["system", "processes"].map((name) => `section.${name}`),
    ...["generic", "cpu", "memory", "disk"].map((name) => `lens.${name}`),
    ...["event", "known_bad", "spike"].map((name) => `locator.${name}`),
    ...["overview", "activity", "statements", "locks", "databases"].map((name) => `pg.section.${name}`),
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
