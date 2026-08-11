import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  format: "esm",
  platform: "node",
  plugins: [{
    name: "registry",
    setup(context) {
      context.onResolve({ filter: /^kronika:registry$/ }, () => ({ namespace: "registry", path: "registry" }))
      context.onLoad({ filter: /.*/, namespace: "registry" }, () => ({
        contents: 'export const registry=[{typeId:"1100001",logicalName:"os_process",columns:[{name:"ts"},{name:"pid"},{name:"read_bytes"}]}]',
      }))
    },
  }],
  stdin: {
    contents: 'export { locatorMatchesColumn } from "../src/entity-table.tsx"; export { processFieldMatches } from "../src/process-table.tsx"; export { rowMatchesLocator } from "../src/locator.ts"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const helpers = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

const row = { segmentId: "segment-a", logicalName: "os_process", typeId: "1100001", ordinal: "7", timestamp: 100, values: { pid: 9, read_bytes: 12 } }
const finding = { segmentId: "segment-a", logicalName: "os_process", typeId: "1100001", rowOrdinal: "7", timestamp: 100, fieldOrdinal: 2, kind: "spike", category: null }

test("physical locators match the exact loaded row and mapped cell", () => {
  assert.equal(helpers.rowMatchesLocator(row, finding), true)
  assert.equal(helpers.rowMatchesLocator({ ...row, timestamp: 101 }, finding), false)
  assert.equal(helpers.rowMatchesLocator({ ...row, ordinal: "8" }, finding), false)
  assert.equal(helpers.locatorMatchesColumn({ field: "read_rate", label: "Read", physicalField: { "1100001": "read_bytes" } }, row.typeId, "read_bytes"), true)
  assert.equal(helpers.locatorMatchesColumn({ field: "write_bytes", label: "Write" }, row.typeId, "read_bytes"), false)
  assert.equal(helpers.processFieldMatches({ id: "read_bytes", field: "read_bytes", label: "Read", help: "Read", kind: "bytes", size: 90 }, row.typeId, "read_bytes"), true)
})

test("locator classes, scrolling, and selection state are independent", async () => {
  const entity = await readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8")
  const process = await readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8")
  for (const source of [entity, process]) {
    assert.match(source, /aria-selected=/)
    assert.match(source, /locator-row/)
    assert.match(source, /locator-cell/)
    assert.match(source, /scrollToIndex\(locatedIndex/)
  }
})
