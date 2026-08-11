import assert from "node:assert/strict"
import { createRequire } from "node:module"
import { readFile } from "node:fs/promises"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  external: ["react", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  stdin: {
    contents: 'export { TimeTicks } from "../src/time-ticks.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  treeShaking: true,
  write: false,
})
const module = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(module, module.exports, createRequire(import.meta.url))
const { TimeTicks } = module.exports

test("time ticks render as ordinary HTML with exact UTC positions", () => {
  const hour = Date.UTC(2026, 7, 10, 15) * 1_000
  const markup = renderToStaticMarkup(createElement(TimeTicks, { className: "test-ticks", hour }))
  assert.equal((markup.match(/data-time-tick="true"/g) ?? []).length, 7)
  assert.deepEqual([...markup.matchAll(/>(\d\d:\d\d)<\/span>/g)].map((match) => match[1]), [
    "15:00", "15:10", "15:20", "15:30", "15:40", "15:50", "16:00",
  ])
  assert.doesNotMatch(markup, /<svg|<text/)
})

test("responsive plot SVGs contain lines only while HTML owns time glyphs", async () => {
  const [timeline, chart, styles] = await Promise.all([
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/series-chart.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  for (const source of [timeline, chart]) {
    assert.match(source, /preserveAspectRatio="none"/)
    assert.match(source, /<TimeTicks /)
    assert.doesNotMatch(source, /<text\b/)
  }
  assert.match(styles, /\.time-ticks[^}]*font-family:\s*"JetBrains Mono"/s)
  assert.match(styles, /\.time-ticks[^}]*font-stretch:\s*normal/s)
  assert.match(styles, /\.time-ticks[^}]*letter-spacing:\s*normal/s)
  assert.match(styles, /\.time-ticks[^}]*pointer-events:\s*none/s)
})
