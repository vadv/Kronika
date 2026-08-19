import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("metric selection and help use sibling controls", async () => {
  const [timeline, system] = await Promise.all([
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
  ])
  assert.match(timeline, /className="lane-select[^"]*"[\s\S]*?<\/button>\s*<LabelHelp[^>]*iconOnly/)
  assert.match(system, /className="metric-choice[^"]*"[\s\S]*?<\/button>\s*<LabelHelp[^>]*iconOnly/)
  assert.doesNotMatch(timeline, /<button(?:(?!<\/button>)[\s\S])*<LabelHelp/)
  assert.doesNotMatch(system, /<button(?:(?!<\/button>)[\s\S])*<LabelHelp/)
})

test("narrow controls stay bounded and coarse-pointer table help is immediately reachable", async () => {
  const [styles, picker, entityTable, help] = await Promise.all([
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../src/hour-picker.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/help.tsx", import.meta.url), "utf8"),
  ])
  assert.match(picker.match(/<div[^>]*data-testid="hour-popover"[^>]*>/s)?.[0] ?? "", /\bfixed\b/)
  assert.match(styles, /\.lensbar \{ flex-wrap: wrap;/)
  assert.match(help, /coarse:\[\.entity-header-cell>&\]:opacity-100 coarse:\[\.entity-header-cell>&\]:pointer-events-auto/)
  // 36x36 around a 14px mark: the mark steps in by its own reach so the target
  // fits inside the cell instead of being clipped by the column edge.
  assert.match(help, /coarse:\[\.entity-header-cell>&\]:mr-\[11px\]/)
  assert.match(help, /coarse:\[\.entity-header-cell_&\]:after:-inset-\[11px\]/)
  assert.match(entityTable, /scroll-padding-inline-end:15px/)
  assert.doesNotMatch(help, /w-11/)
})

test("history placeholders are compact and name loading, error, and empty states", async () => {
  const [chart, styles, en, ru] = await Promise.all([
    readFile(new URL("../src/series-chart.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  assert.match(chart, /status === "loading"[\s\S]*?history\.loading[\s\S]*?status === "error"[\s\S]*?history\.error[\s\S]*?history\.empty/)
  assert.match(chart, /min-h-\[30px\][^"]*text-sm/)
  assert.doesNotMatch(chart, /min-h-\[200px\]/)
  for (const dictionary of [en, ru]) {
    assert.match(dictionary, /^history\.loading:/m)
    assert.match(dictionary, /^history\.error:/m)
    assert.match(dictionary, /^history\.empty:/m)
  }
})

test("Boolean connectors and parentheses share one compact muted syntax token", async () => {
  const source = await readFile(new URL("../src/table-filter.tsx", import.meta.url), "utf8")
  const chips = source.match(/function SearchChips[\s\S]*?\n}\n\ntype SearchSyntaxTokenValue/)?.[0] ?? ""
  const syntax = source.match(/type SearchSyntaxTokenValue[\s\S]*?\n}\n\nfunction SearchChip/)?.[0] ?? ""

  assert.match(syntax, /"AND" \| "OR" \| "\(" \| "\)"/)
  assert.match(syntax, /h-\[19px\]/)
  assert.match(syntax, /text-\[9px\]/)
  assert.match(syntax, /font-medium/)
  assert.match(syntax, /leading-none/)
  assert.match(syntax, /text-fg4/)
  assert.doesNotMatch(syntax, /\bborder\b|\bbg-/)
  assert.match(chips, /data-search-predicate=""/)
  assert.match(chips, /<SearchSyntaxToken key=\{`\$\{path}:open`\} token="\(" \/>/)
  assert.match(chips, /<SearchSyntaxToken key=\{`\$\{path}:operator`\} token=\{current\.kind === "and" \? "AND" : "OR"\} \/>/)
  assert.match(chips, /<SearchSyntaxToken key=\{`\$\{path}:close`\} token="\)" \/>/)
  assert.doesNotMatch(chips, /grouped \? \["\("\]|grouped \? \["\)"\]/)
})
