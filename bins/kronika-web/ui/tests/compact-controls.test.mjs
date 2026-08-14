import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("metric selection and help use sibling controls", async () => {
  const [timeline, system] = await Promise.all([
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
  ])
  assert.match(timeline, /className="lane-select"[\s\S]*?<\/button>\s*<LabelHelp[^>]*iconOnly/)
  assert.match(system, /className="metric-choice"[\s\S]*?<\/button>\s*<LabelHelp[^>]*iconOnly/)
  assert.doesNotMatch(timeline, /<button(?:(?!<\/button>)[\s\S])*<LabelHelp/)
  assert.doesNotMatch(system, /<button(?:(?!<\/button>)[\s\S])*<LabelHelp/)
})

test("narrow controls stay bounded and coarse-pointer table help is immediately reachable", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
  assert.match(styles, /\.hour-popover[^}]*position:\s*fixed/)
  assert.match(styles, /@media \(max-width: 520px\)[\s\S]*?\.top-actions[^}]*flex-wrap:\s*wrap/)
  assert.match(styles, /@media \(hover: none\), \(pointer: coarse\)[\s\S]*?\.entity-header-cell > \.label-help[^}]*opacity:\s*1[^}]*pointer-events:\s*auto/)
  assert.match(styles, /@media \(hover: none\), \(pointer: coarse\)[\s\S]*?\.help-dot[^}]*height:\s*44px[^}]*width:\s*44px/)
})

test("history placeholders are compact and name loading, error, and empty states", async () => {
  const [chart, styles, en, ru] = await Promise.all([
    readFile(new URL("../src/series-chart.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  assert.match(chart, /status === "loading"[\s\S]*?history\.loading[\s\S]*?status === "error"[\s\S]*?history\.error[\s\S]*?history\.empty/)
  assert.match(styles, /\.series-status[^}]*min-height:\s*30px/)
  assert.doesNotMatch(styles, /\.series-(?:empty|status)[^}]*height:\s*200px/)
  for (const dictionary of [en, ru]) {
    assert.match(dictionary, /^history\.loading:/m)
    assert.match(dictionary, /^history\.error:/m)
    assert.match(dictionary, /^history\.empty:/m)
  }
})
