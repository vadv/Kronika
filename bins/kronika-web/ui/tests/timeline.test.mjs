import assert from "node:assert/strict"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  external: ["kronika:registry"],
  format: "esm",
  platform: "node",
  stdin: {
    contents: 'export { findingShape, groupFindings } from "../src/timeline.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

function finding(kind, timestamp, ordinal) {
  return {
    category: null,
    fieldOrdinal: 0,
    kind,
    rowOrdinal: ordinal,
    segmentId: "segment-a",
    timestamp,
    typeId: "1100001",
  }
}

test("timeline markers aggregate only identical kind and timestamp", () => {
  const first = finding("event", 200, "7")
  const grouped = helpers.groupFindings([
    finding("spike", 100, "1"),
    first,
    finding("event", 200, "8"),
    finding("known_bad", 200, "9"),
  ])

  assert.deepEqual(grouped.map(({ count, kind, timestamp }) => ({ count, kind, timestamp })), [
    { count: 1, kind: "spike", timestamp: 100 },
    { count: 2, kind: "event", timestamp: 200 },
    { count: 1, kind: "known_bad", timestamp: 200 },
  ])
  assert.equal(grouped[1].finding, first)
})

test("finding kinds have non-color shape identities", () => {
  assert.equal(helpers.findingShape("event"), "circle")
  assert.equal(helpers.findingShape("known_bad"), "diamond")
  assert.equal(helpers.findingShape("spike"), "triangle")
})
