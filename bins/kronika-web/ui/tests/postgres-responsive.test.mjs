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
  // The process layout collapses at 1179px, the PostgreSQL one at 1000px, and
  // only there does the dock become a fixed overlay.
  assert.match(app, /max-\[1179px\]:grid-cols-\[minmax\(0,1fr\)\]/)
  assert.match(postgres, /max-\[1000px\]:grid-cols-\[minmax\(0,1fr\)\]/)
  assert.match(stylesheet, /@utility pg-detail \{[\s\S]*?@media \(max-width: 1000px\) \{[^}]*position: fixed;/)
  assert.doesNotMatch(app, /max-\[1179px\]:fixed/)
})

test("the operator bar wraps before its controls can widen a 1024 pixel page", () => {
  assert.match(app, /className="topbar[^"]*max-\[1179px\]:flex-wrap/)
})

test("every PostgreSQL table view owns the viewport flex chain", () => {
  assert.match(app, /visibleSource === "postgresql" && pgSection !== "overview"/)
  assert.match(app, /flex h-dvh min-h-0 flex-col overflow-hidden/)
  assert.match(app, /pg-table-shell/)
  assert.match(app, /pg-table-workspace flex min-h-0 flex-1 flex-col overflow-hidden/)
  assert.match(app, /\[\.pg-table-shell>&\]:flex-none/)
  // One utility carries the chain: layout, main column, table and scroll port
  // all hand their height down instead of growing the page.
  assert.match(stylesheet, /@utility pg-stretch \{[\s\S]*?:is\(\.pg-table-shell\) & \{[^}]*flex: 1 1 0;[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(stylesheet, /@utility pg-stretch \{[\s\S]*?:is\(\.charts-hidden\.pg-table-shell\) & \{[^}]*flex: 1 1 auto;/)
  assert.match(postgres, /pg-entity-layout[^"]*\[\.pg-table-shell_&\]:grid-rows-\[minmax\(0,1fr\)\]/)
  assert.match(entityTable, /className=\{`entity-table[^`]*pg-stretch/)
  assert.match(stylesheet, /@utility pg-detail \{[\s\S]*?:is\(\.pg-table-shell\) & \{[^}]*max-height: none;[^}]*min-height: 0;/)
  assert.match(chart, /\[\.pg-table-shell_\.pg-detail_&:not\(\.uplot-expanded\)\]:h-\[200px\]/)
})

test("short PostgreSQL workspaces keep a visible chart path without crushing the table", () => {
  assert.match(app, /\[&>\.timeline-shell\]:flex-none/)
  assert.match(stylesheet, /@custom-variant short \(@media \(max-height: 620px\)\)/)
  assert.match(stylesheet, /@custom-variant tiny-height \(@media \(max-height: 480px\)\)/)
  // A short viewport shrinks the timeline; a very short one hides the plot and
  // leaves a labelled way back into it.
  assert.match(stylesheet, /@utility launch-timeline \{[\s\S]*?max-height: 620px\)[\s\S]*?height: 132px/)
  assert.match(stylesheet, /@utility launch-timeline \{[\s\S]*?\.uplot-host \{ min-height: 102px/)
  assert.match(stylesheet, /@utility launch-timeline \{[\s\S]*?max-height: 480px\)[\s\S]*?height: 32px/)
  assert.match(stylesheet, /@utility launch-timeline \{[\s\S]*?chart-marker-track"\] \{ display: none/)
  assert.match(stylesheet, /@utility launch-timeline \{[\s\S]*?content: attr\(aria-label\)/)
})
