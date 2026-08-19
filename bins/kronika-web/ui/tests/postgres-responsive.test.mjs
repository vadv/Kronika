import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

const directory = dirname(fileURLToPath(import.meta.url))
const stylesheet = await readFile(join(directory, "../src/styles.css"), "utf8")
const app = await readFile(join(directory, "../src/app.tsx"), "utf8")
const entityTable = await readFile(join(directory, "../src/entity-table.tsx"), "utf8")
const postgres = await readFile(join(directory, "../src/postgres-view.tsx"), "utf8")
const chart = await readFile(join(directory, "../src/uplot-chart.tsx"), "utf8")

test("PostgreSQL keeps its dock beside the table at 1024 pixels", () => {
  assert.match(postgres, /grid-cols-\[minmax\(0,1fr\)_clamp\(460px,32vw,600px\)\]/)
  assert.match(postgres, /className="pg-entity-main min-w-0 pg-stretch"/)
  // The shared Inspector becomes an overlay at 1000px; the older PostgreSQL
  // entity dock keeps the same transition while it is migrated into it.
  assert.match(stylesheet, /\.inspector \{[\s\S]*?flex: 0 0 var\(--inspector-width\)/)
  assert.match(postgres, /max-\[1000px\]:grid-cols-\[minmax\(0,1fr\)\]/)
  assert.match(stylesheet, /@utility pg-detail \{[\s\S]*?@media \(max-width: 1000px\) \{[^}]*position: fixed;/)
  assert.doesNotMatch(app, /max-\[1179px\]:fixed/)
})

test("the operator bar wraps before its controls can widen a 1024 pixel page", () => {
  assert.match(app, /className="topbar/)
  assert.match(stylesheet, /@utility topbar \{[\s\S]*?@media \(max-width: 1000px\) \{[\s\S]*?display: grid;/)
  assert.match(stylesheet, /overflow: hidden;/)
})

test("every PostgreSQL table view owns the viewport flex chain", () => {
  assert.match(app, /visibleSource === "postgresql" && pgSection !== "overview"/)
  assert.match(app, /flex h-dvh min-h-0 flex-col overflow-hidden/)
  assert.match(app, /pg-table-shell/)
  assert.match(app, /pg-table-workspace flex flex-col overflow-hidden/)
  assert.match(app, /\[\.pg-table-shell>&\]:flex-none/)
  // One utility carries the chain: layout, main column, table and scroll port
  // all hand their height down instead of growing the page.
  assert.match(stylesheet, /@utility pg-stretch \{[\s\S]*?:is\(\.pg-table-shell\) & \{[^}]*flex: 1 1 0;[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(postgres, /pg-entity-layout[^"]*\[\.pg-table-shell_&\]:grid-rows-\[minmax\(0,1fr\)\]/)
  assert.match(entityTable, /className=\{`entity-table[^`]*pg-stretch/)
  assert.match(stylesheet, /@utility pg-detail \{[\s\S]*?:is\(\.pg-table-shell\) & \{[^}]*max-height: none;[^}]*min-height: 0;/)
  assert.match(chart, /\[\.pg-table-shell_\.pg-detail_&\]:h-\[200px\]/)
})

test("short PostgreSQL workspaces keep the honest compact preview", () => {
  assert.match(app, /\[&>\.timeline-shell\]:flex-none/)
  assert.match(stylesheet, /\.timeline-preview \{[^}]*height: 104px;/s)
  assert.match(chart, /variant === "preview" \? "h-\[76px\]/)
  assert.doesNotMatch(stylesheet, /\.timeline-preview[\s\S]{0,240}\.uplot-host[^}]*display: none/)
})
