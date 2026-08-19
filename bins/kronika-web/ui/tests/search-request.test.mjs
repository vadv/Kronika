import assert from "node:assert/strict"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importFile } from "./import-module.mjs"

const request = await importFile("../src/search-request.tsx")
const copy = {
  en: {
    "filter.searching": "Searching…",
    "filter.searching_retained": "Searching… Rows retained.",
    "filter.search_failed": "Search failed.",
    "filter.search_failed_retained": "Search failed. Rows retained.",
  },
  ru: {
    "filter.searching": "Идёт поиск…",
    "filter.searching_retained": "Идёт поиск… Строки сохранены.",
    "filter.search_failed": "Ошибка поиска.",
    "filter.search_failed_retained": "Ошибка поиска. Строки сохранены.",
  },
}

test("pending search renders an unmistakable localized live status without a fake percentage", () => {
  for (const locale of ["en", "ru"]) {
    const state = request.beginSearchRequest("os_process", false)
    const markup = renderToStaticMarkup(createElement(request.SearchRequestMessage, {
      request: state,
      t: (key) => copy[locale][key] ?? key,
    }))
    assert.match(markup, /role="status"/)
    assert.match(markup, /aria-live="polite"/)
    assert.match(markup, /<progress/)
    assert.ok(markup.includes(copy[locale]["filter.searching"]))
    assert.doesNotMatch(markup, /%|Loaded|Загружено|No rows match|Под фильтр/)
  }
})

test("retained rows and failure are explicit", () => {
  const newest = request.beginSearchRequest("os_process", true)
  const failed = { ...newest, phase: "error" }
  assert.equal(failed.phase, "error")
  assert.equal(failed.retained, true)
  const markup = renderToStaticMarkup(createElement(request.SearchRequestMessage, {
    request: failed,
    t: (key) => copy.en[key] ?? key,
  }))
  assert.match(markup, /role="alert"/)
  assert.match(markup, /Rows retained/)
})

test("the fixed table frame gives pending and failure precedence over completed empty copy", async () => {
  const entity = await readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8")
  const filter = await readFile(new URL("../src/table-filter.tsx", import.meta.url), "utf8")
  const message = await readFile(new URL("../src/search-request.tsx", import.meta.url), "utf8")
  assert.match(entity, /searchPending[\s\S]*searchRequest\.phase === "error"[\s\S]*loading/)
  assert.match(message, /filter\.\$\{pending \? "searching" : "search_failed"\}/)
  assert.match(entity, /aria-busy=\{searchPending\}/)
  assert.match(entity, /min-h-\[26px\]/)
  assert.match(entity, /searchMessage \?\? status/)
  assert.match(filter, /if \(!draftResult\.ok\) return[\s\S]*onPattern\?\.\(draftResult\.query\.canonical\)/)
})
