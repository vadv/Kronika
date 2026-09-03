import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("the export dialog renders inclusive whole-second defaults in the visible mode", async () => {
  const dialog = await readFile(new URL("../src/export-dialog.tsx", import.meta.url), "utf8")
  assert.match(dialog, /data-testid="export-dialog"/)
  assert.match(dialog, /data-testid="export-form"/)
  assert.equal(dialog.match(/step=\{1\}/g)?.length, 2)
  assert.equal(dialog.match(/type="datetime-local"/g)?.length, 2)
  assert.match(dialog, /useState\(\(\) => exportRangeDefaults\(hour, mode\)\)/)
  assert.match(dialog, /t\("export\.mode", \{ mode: t\(`timezone\.\$\{mode\}`\) \}\)/)
})

test("the native modal owns focus, Escape, submission and restoration", async () => {
  const dialog = await readFile(new URL("../src/export-dialog.tsx", import.meta.url), "utf8")
  assert.match(dialog, /<dialog[\s\S]*?aria-modal="true"[\s\S]*?role="dialog"/)
  assert.match(dialog, /dialog\.current\?\.showModal\(\)/)
  assert.match(dialog, /fromInput\.current\?\.focus\(\{ preventScroll: true \}\)/)
  assert.match(dialog, /onCancel=\{\(event\) => \{ event\.preventDefault\(\); dismiss\(\) \}\}/)
  assert.match(dialog, /onKeyDown=\{\(event\) => event\.stopPropagation\(\)\}/)
  assert.match(dialog, /if \(target\?\.isConnected\) target\.focus\(\{ preventScroll: true \}\)/)
  assert.match(dialog, /<form[\s\S]*?onSubmit=/)
  assert.match(dialog, /if \(submitting\.current\) return/)
  assert.match(dialog, /submitting\.current = true/)
  assert.match(dialog, /disabled=\{busy\}[\s\S]*?type="submit"/)
  assert.match(dialog, /<progress[^>]*>/)
  assert.match(dialog, /role="alert"/)
  assert.match(dialog, /className="flex h-12 items-center overflow-auto" data-testid="export-status"/)
  assert.doesNotMatch(dialog, /reason\.message/)
})

test("the live top bar opens one export surface without changing navigation state", async () => {
  const [app, dialog, build] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/export-dialog.tsx", import.meta.url), "utf8"),
    readFile(new URL("../scripts/build.mjs", import.meta.url), "utf8"),
  ])
  assert.match(app, /\{!KRONIKA_REPORT && <button[^>]*data-testid="export-trigger"/)
  assert.match(app, /setExportOpen\(true\); setMcpOpen\(false\); setHelpOpen\(false\)/)
  assert.match(app, /\{!KRONIKA_REPORT && exportOpen && hour !== null && <ExportDialog/)
  assert.doesNotMatch(dialog, /history\.|location\.|writeAddress|setCursor/)
  assert.match(dialog, /fetchExportArtifact\(apiFetch, range\.from, range\.to, controller\.signal\)/)
  assert.match(build, /"\/api\/export"/)
  assert.match(build, /"export-trigger"/)
})
