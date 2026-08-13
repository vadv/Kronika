import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

const directory = dirname(fileURLToPath(import.meta.url))
const stylesheet = await readFile(join(directory, "../src/styles.css"), "utf8")
const app = await readFile(join(directory, "../src/app.tsx"), "utf8")

function mediaBlock(condition) {
  const start = stylesheet.indexOf(`@media (${condition})`)
  assert.notEqual(start, -1, `missing ${condition} media query`)
  const opening = stylesheet.indexOf("{", start)
  let depth = 0
  for (let index = opening; index < stylesheet.length; index += 1) {
    if (stylesheet[index] === "{") depth += 1
    if (stylesheet[index] === "}") depth -= 1
    if (depth === 0) return stylesheet.slice(opening + 1, index)
  }
  assert.fail(`unterminated ${condition} media query`)
}

test("PostgreSQL keeps its dock beside the table at 1024 pixels", () => {
  assert.match(stylesheet, /\.pg-entity-layout \{[^}]*grid-template-columns: minmax\(0, 1fr\) 390px;/)
  assert.match(stylesheet, /\.pg-entity-main \{ min-width: 0; \}/)

  const processOverlay = mediaBlock("max-width: 1179px")
  assert.match(processOverlay, /\.process-layout/)
  assert.doesNotMatch(processOverlay, /\.pg-(?:entity-layout|detail)/)

  const postgresOverlay = mediaBlock("max-width: 1000px")
  assert.match(postgresOverlay, /\.pg-entity-layout \{ grid-template-columns: minmax\(0, 1fr\); \}/)
  assert.match(postgresOverlay, /\.pg-detail \{[^}]*position: fixed;/)
})

test("the operator bar wraps before its controls can widen a 1024 pixel page", () => {
  const compactShell = mediaBlock("max-width: 1179px")
  assert.match(compactShell, /\.topbar \{[^}]*flex-wrap: wrap;/)
})

test("only long PostgreSQL entity views own the viewport flex chain", () => {
  assert.match(app, /relationSection \|\| pgSection === "statements" \|\| pgSection === "plans"/)
  assert.match(app, /pg-table-shell/)
  assert.match(app, /pg-table-workspace/)
  assert.match(stylesheet, /\.pg-table-shell \{[^}]*height: 100dvh;[^}]*min-height: 0;/)
  assert.match(stylesheet, /\.pg-table-shell \.pg-entity-layout \.entity-scroll \{[^}]*height: auto;/)
  assert.match(stylesheet, /\.pg-table-shell \.pg-detail \{[^}]*max-height: none;[^}]*min-height: 0;/)
})
