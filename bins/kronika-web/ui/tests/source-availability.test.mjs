import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { hasPostgresTelemetry } = await importModule('export { hasPostgresTelemetry } from "../src/source-availability.ts"')
const data = (postgresqlConfigured, postgresqlPresent = false) => ({ postgresqlConfigured, postgresqlPresent })

test("PostgreSQL availability follows configuration or selected-hour telemetry", () => {
  assert.equal(hasPostgresTelemetry(data(false)), false)
  assert.equal(hasPostgresTelemetry(data(true)), true)
  assert.equal(hasPostgresTelemetry(data(false, true)), true)
})

test("the unavailable PostgreSQL route renders Host synchronously", async () => {
  const source = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(source, /const visibleSource: Source = source === "postgresql" && !pgPresent \? "host" : source/)
  assert.match(source, /if \(source === "postgresql" && !pgPresent\) setSource\("host"\)/)
  assert.match(source, /visibleSource === "postgresql" && <PostgresView/)
  assert.match(source, /disabled=\{!pgPresent\}/)
})
