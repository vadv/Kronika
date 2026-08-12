import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

const directory = dirname(fileURLToPath(import.meta.url))
const stylesheet = await readFile(join(directory, "../src/styles.css"), "utf8")

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

  const processOverlay = mediaBlock("max-width: 1179px")
  assert.match(processOverlay, /\.process-layout/)
  assert.doesNotMatch(processOverlay, /\.pg-(?:entity-layout|detail)/)

  const postgresOverlay = mediaBlock("max-width: 1000px")
  assert.match(postgresOverlay, /\.pg-entity-layout \{ grid-template-columns: minmax\(0, 1fr\); \}/)
  assert.match(postgresOverlay, /\.pg-detail \{[^}]*position: fixed;/)
})
