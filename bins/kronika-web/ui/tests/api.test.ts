import assert from "node:assert/strict"
import test from "node:test"

import { parseNdjson } from "../src/wire.ts"

test("a streamed error record rejects an otherwise successful response", () => {
  assert.throws(
    () => parseNdjson('{"record":"rows"}\n{"record":"error","error":"unreadable"}\n', "/api/example"),
    /unreadable.*\/api\/example/,
  )
})
