import assert from "node:assert/strict"
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
