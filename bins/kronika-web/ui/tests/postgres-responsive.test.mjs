import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

const directory = dirname(fileURLToPath(import.meta.url))
const stylesheet = await readFile(join(directory, "../src/styles.css"), "utf8")
const app = await readFile(join(directory, "../src/app.tsx"), "utf8")
const entityTable = await readFile(join(directory, "../src/entity-table.tsx"), "utf8")

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

test("PostgreSQL keeps its dock beside the table at 1024 pixels", async () => {
  const postgres = await readFile(join(directory, "../src/postgres-view.tsx"), "utf8")
  assert.match(postgres, /grid-cols-\[minmax\(0,1fr\)_clamp\(460px,32vw,600px\)\]/)
  assert.match(postgres, /className="pg-entity-main min-w-0"/)

  const processOverlay = mediaBlock("max-width: 1179px")
  assert.doesNotMatch(processOverlay, /\.pg-(?:entity-layout|detail)/)
  // The process layout collapses on the markup now, at the same breakpoint.
  assert.match(app, /max-\[1179px\]:grid-cols-\[minmax\(0,1fr\)\]/)

  const postgresOverlay = mediaBlock("max-width: 1000px")
  // The layout collapses on the markup; the dock still becomes a fixed overlay.
  assert.match(postgres, /max-\[1000px\]:grid-cols-\[minmax\(0,1fr\)\]/)
  assert.match(postgresOverlay, /\.pg-detail \{[^}]*position: fixed;/)
})

test("the operator bar wraps before its controls can widen a 1024 pixel page", () => {
  const compactShell = mediaBlock("max-width: 1179px")
  assert.match(compactShell, /\.topbar \{[^}]*flex-wrap: wrap;/)
})

test("every PostgreSQL table view owns the viewport flex chain", () => {
  assert.match(app, /visibleSource === "postgresql" && pgSection !== "overview"/)
  assert.match(app, /pg-table-shell/)
  assert.match(app, /pg-table-workspace/)
  assert.match(stylesheet, /\.pg-table-shell \{[^}]*height: 100dvh;[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(stylesheet, /\.pg-table-shell > \.topbar \{ flex: 0 0 auto; \}/)
  assert.match(stylesheet, /\.pg-table-workspace \{[^}]*flex: 1 1 0;[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(stylesheet, /\.pg-table-shell \.pg-entity-layout \{[^}]*flex: 1 1 0;[^}]*grid-template-rows: minmax\(0, 1fr\);[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(stylesheet, /\.pg-table-shell \.pg-entity-main, \.pg-table-shell \.pg-entity-layout \.entity-table \{[^}]*flex: 1 1 0;[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(entityTable, /\[\.pg-table-shell_\.pg-entity-layout_&\]:flex-1/)
  assert.match(stylesheet, /\.charts-hidden\.pg-table-shell \.pg-entity-layout, \.charts-hidden\.pg-table-shell \.pg-entity-layout \.entity-table, \.charts-hidden\.pg-table-shell \.pg-entity-layout \.entity-scroll \{[^}]*flex: 1 1 auto;/)
  assert.match(stylesheet, /\.pg-table-shell \.pg-detail \{[^}]*max-height: none;[^}]*min-height: 0;/)
  assert.match(stylesheet, /\.pg-table-shell \.pg-detail \.uplot-figure:not\(\.uplot-expanded\) \{[^}]*flex: 0 0 200px;[^}]*height: 200px;[^}]*max-height: 200px;/)
})

test("short PostgreSQL workspaces keep a visible chart path without crushing the table", () => {
  assert.match(stylesheet, /\.pg-table-workspace > \.timeline-shell \{ flex: 0 0 auto; \}/)

  const compact = mediaBlock("max-height: 620px")
  assert.match(compact, /\.timeline-chart:not\(\.uplot-expanded\)[^{]*\{[^}]*flex-basis: 132px;[^}]*height: 132px;[^}]*min-height: 132px;/)
  assert.match(compact, /\.timeline-chart:not\(\.uplot-expanded\) \.uplot-host \{ min-height: 102px; \}/)

  const launch = mediaBlock("max-height: 480px")
  assert.match(launch, /\.timeline-chart:not\(\.uplot-expanded\)[^{]*\{[^}]*flex-basis: 32px;[^}]*height: 32px;[^}]*min-height: 32px;/)
  assert.match(launch, /\.timeline-chart:not\(\.uplot-expanded\) \.uplot-host,[^}]*\.chart-marker-track \{ display: none; \}/)
  assert.match(launch, /\.timeline-chart:not\(\.uplot-expanded\) \.chart-expand::before \{ content: attr\(aria-label\); \}/)
})
