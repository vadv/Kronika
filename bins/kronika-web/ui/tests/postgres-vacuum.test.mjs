import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const vacuum = await importModule('export * from "../src/postgres-vacuum.ts"')

const HOUR = 1_000_000_000_000
const S = 1_000_000

let ordinal = 0
function row(atSeconds, values, typeId = "1012001") {
  ordinal += 1
  return {
    logicalName: "pg_stat_progress_vacuum", ordinal: String(ordinal), segmentId: "a",
    timestamp: HOUR + atSeconds * S, typeId,
    values: { datid: 5, pid: 9, relid: 70, ...values },
  }
}

test("rows group into episodes by identity, and a lower counter starts a new episode", () => {
  const rows = [
    row(0, { heap_blks_scanned: 10, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap" }),
    row(30, { heap_blks_scanned: 50, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap" }),
    // The same key with a smaller scanned counter is a different run.
    row(60, { heap_blks_scanned: 5, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap" }),
    // A different relation is its own stream regardless of timing.
    row(60, { heap_blks_scanned: 100, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap", relid: 71 }),
  ]
  const episodes = vacuum.buildVacuumEpisodes(rows, 30)
  assert.equal(episodes.length, 3)
})

test("rows at 10:00:00 and 10:05:00 with no recorded sample between them are two episodes", () => {
  const rows = [
    row(0, { heap_blks_scanned: 10, phase: "scanning heap" }),
    row(300, { heap_blks_scanned: 20, phase: "scanning heap" }),
  ]
  // With a 30-second recorded interval the two rows are not adjacent.
  assert.equal(vacuum.buildVacuumEpisodes(rows, 30).length, 2)
  // Without a recorded interval only identity and counters decide.
  assert.equal(vacuum.buildVacuumEpisodes(rows, null).length, 1)
})

test("the in-phase span restarts when the index cycle increments under an unchanged phase name", () => {
  const rows = [
    row(0, { index_vacuum_count: 1, indexes_processed: 1, phase: "vacuuming indexes" }, "1012002"),
    row(30, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, "1012002"),
    row(60, { index_vacuum_count: 2, indexes_processed: 0, phase: "vacuuming indexes" }, "1012002"),
  ]
  const [episode] = vacuum.buildVacuumEpisodes(rows, 30)
  assert.equal(episode.rows.length, 3)
  assert.equal(episode.phaseRows.length, 1)
  assert.equal(vacuum.phaseSpanUs(episode), 0)
})

test("no movement needs three still samples of the phase counter and never fires on old index layouts", () => {
  const still = (typeId) => vacuum.buildVacuumEpisodes([
    row(0, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, typeId),
    row(30, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, typeId),
    row(60, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, typeId),
  ], 30)[0].noMovement
  assert.deepEqual(still("1012002"), { samples: 3, spanUs: 60 * S })
  // PG10-16 records no index progress: the phase shows its span and nothing more.
  assert.equal(still("1012001"), null)
  // Movement clears it.
  const moving = vacuum.buildVacuumEpisodes([
    row(0, { heap_blks_scanned: 1, phase: "scanning heap" }),
    row(30, { heap_blks_scanned: 1, phase: "scanning heap" }),
    row(60, { heap_blks_scanned: 2, phase: "scanning heap" }),
  ], 30)[0].noMovement
  assert.equal(moving, null)
  // Truncation is judged by the phase alone.
  const truncating = vacuum.buildVacuumEpisodes([
    row(0, { phase: "truncating heap" }),
    row(30, { phase: "truncating heap" }),
    row(60, { phase: "truncating heap" }),
  ], 30)[0].noMovement
  assert.deepEqual(truncating, { samples: 3, spanUs: 60 * S })
})

test("risk is fixed by phase name and the sort fronts the cursor's pass, riskiest first", () => {
  assert.equal(vacuum.phaseRisk("truncating heap"), "dangerous")
  assert.equal(vacuum.phaseRisk("vacuuming indexes"), "heavy")
  assert.equal(vacuum.phaseRisk("vacuuming heap"), "heavy")
  assert.equal(vacuum.phaseRisk("cleaning up indexes"), "heavy")
  assert.equal(vacuum.phaseRisk("scanning heap"), "ordinary")
  assert.equal(vacuum.phaseRisk("unheard-of phase"), "ordinary")
  const rows = [
    row(0, { phase: "scanning heap", relid: 70 }),
    row(60, { phase: "scanning heap", relid: 70 }),
    row(60, { phase: "truncating heap", relid: 71 }),
    row(30, { phase: "vacuuming heap", relid: 72 }),
  ]
  const episodes = vacuum.buildVacuumEpisodes(rows, null)
  const atTs = vacuum.vacuumAtTimestamp(rows, HOUR + 90 * S)
  assert.equal(atTs, HOUR + 60 * S)
  const sorted = vacuum.sortVacuumEpisodes(episodes, atTs)
  // Both at-sample episodes lead, the dangerous one first; the one last seen
  // at second 30 follows.
  assert.equal(sorted[0].last.values.relid, 71)
  assert.equal(sorted[1].last.values.relid, 70)
  assert.equal(sorted[2].last.values.relid, 72)
})

test("layout gates and the delay delta read the recorded layouts, not zeroes", () => {
  const old = [row(0, { phase: "scanning heap" })]
  const pg18 = [
    row(0, { delay_time: 100, phase: "scanning heap" }, "1012003"),
    row(30, { delay_time: 150, phase: "scanning heap" }, "1012003"),
  ]
  assert.equal(vacuum.vacuumLayoutHas(old, "indexes_total"), false)
  assert.equal(vacuum.vacuumLayoutHas(old, "delay_time"), false)
  assert.equal(vacuum.vacuumLayoutHas(pg18, "indexes_total"), true)
  assert.equal(vacuum.vacuumLayoutHas(pg18, "delay_time"), true)
  const [episode] = vacuum.buildVacuumEpisodes(pg18, 30)
  assert.equal(vacuum.delayDelta(episode), 50)
  const [single] = vacuum.buildVacuumEpisodes([pg18[0]], 30)
  assert.equal(vacuum.delayDelta(single), null)
})
