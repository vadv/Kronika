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
  stdin: {
    contents: 'export { placeTooltip } from "../src/help.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const help = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`)

test("tooltip placement stays in the viewport and flips above a low anchor", () => {
  const size = { height: 80, width: 200 }
  assert.deepEqual(
    help.placeTooltip({ bottom: 114, height: 14, left: 300, top: 100, width: 14 }, size, { height: 600, width: 800 }),
    { left: 207, placement: "below", top: 120 },
  )
  assert.deepEqual(
    help.placeTooltip({ bottom: 584, height: 14, left: 786, top: 570, width: 14 }, size, { height: 600, width: 800 }),
    { left: 592, placement: "above", top: 484 },
  )
  assert.deepEqual(
    help.placeTooltip({ bottom: 18, height: 10, left: 2, top: 8, width: 10 }, { height: 180, width: 240 }, { height: 120, width: 160 }),
    { left: 8, placement: "below", top: 8 },
  )
})

test("field help uses a fixed top-level portal above every workspace layer", async () => {
  const [source, styles] = await Promise.all([
    readFile(new URL("../src/help.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.match(source, /createPortal\(/)
  assert.match(source, /document\.body/)
  assert.match(source, /position.*placeTooltip|placeTooltip\(/s)
  assert.match(source, /document\.addEventListener\("pointerdown", outside, true\)/)
  assert.match(source, /window\.addEventListener\("scroll", update, true\)/)
  assert.match(source, /event\.key !== "Escape"/)
  assert.match(styles, /\.tooltip[^}]*position:\s*fixed/s)
  assert.match(styles, /\.tooltip[^}]*z-index:\s*1000/s)
  assert.match(styles, /\.tooltip[^}]*max-width:[^;}]*100vw/s)
  assert.match(styles, /\.entity-header-cell > \.label-help[^}]*opacity:\s*0/s)
  assert.match(styles, /\.entity-header-cell:focus-within > \.label-help[^}]*opacity:\s*1/s)
})
