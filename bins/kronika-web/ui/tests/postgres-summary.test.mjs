import assert from "node:assert/strict"
import test from "node:test"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

import { importFile } from "./import-module.mjs"

const summary = await importFile("../src/postgres-summary.tsx")

const row = (ordinal, values) => ({
  logicalName: "pg_stat_database", ordinal, segmentId: "1", timestamp: 10,
  typeId: "1005001", values,
})

test("database summary aggregates the filtered databases into four facts", () => {
  const values = summary.databaseSummary([
    row("0", { datid: 0, datname: null, numbackends: 99, xact_commit: 99, xact_rollback: 99, blks_hit: 99, blks_read: 1, frozen_xid_age: 999 }),
    row("1", { datid: 1, datname: "app", numbackends: 3, xact_commit: 10, xact_rollback: 2, blks_hit: 90, blks_read: 10, frozen_xid_age: 100 }),
    row("2", { datid: 2, datname: "app_jobs", numbackends: 2, xact_commit: 5, xact_rollback: 3, blks_hit: 80, blks_read: 20, frozen_xid_age: 500 }),
  ], "database:app*")

  assert.deepEqual(values, { backends: 5, transactions: 20, buffer_hit_pct: 85, xid_age: 500 })
  const markup = renderToStaticMarkup(createElement(summary.PostgresSummary, {
    locale: "en", section: "pg_stat_database", summary: values, t: (key) => key,
  }))
  assert.equal((markup.match(/data-summary-fact=/g) ?? []).length, 4)
  assert.doesNotMatch(markup, /<button|<table/)
})
