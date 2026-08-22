# Demo Investigation Reel Design

## Product promise

The demo must answer an operator's question: "What was happening to the
database when the application stopped working?" It demonstrates recorded
recorded data, not automatic diagnosis. A visitor should be able to decide whether
PostgreSQL was involved, identify the mechanism, and name the process, query,
plan, lock holder, or maintenance task visible at that moment.

## Five-minute story

The database represents one small commerce application. Connections use
recognizable application names (`checkout-api`, `catalog-api`,
`payments-worker`, `reporting-worker`, `deploy-migration`, `vacuum-worker`)
and tables use recognizable names under the `shop` schema.

The generator continuously produces a quiet, credible baseline. Three bounded
episodes recur with quiet gaps:

1. A checkout lock convoy. One transaction is idle while holding an order row;
   checkout waiters form a real PostgreSQL lock chain and the tail reaches a
   finite statement timeout. The episode lasts seconds, then all transactions
   are committed or rolled back.
2. A query-plan regression. The same normalized checkout query first uses a
   composite orders index, then uses a sequential scan while a bounded
   deployment window removes that index, and finally returns to the index plan
   after the index is recreated. Multiple workers make the slow plan visible
   in statement, plan, process, table, and host readings without exhausting the
   container.
3. A throttled Vacuum over a named high-churn table, with a finite statement
   timeout and a quiet interval after completion.

Slow-query, syntax-error, and connection-error events remain supporting detail
but occur less often than the hero episodes.

## Safety and determinism

Every statement and idle transaction has a finite server-side timeout. Every
episode owns its connections, awaits their completion, restores any schema
change, and waits through a quiet interval. Setup is idempotent. The default
dataset is large enough to make index and sequential plans materially
different, but remains inside the demo's 1 GiB memory and 512 MiB PostgreSQL
tmpfs limits.

Random baseline traffic uses fixed seeds. Episode order, application names,
SQL text, table names, durations, and recovery are deterministic. Restarting
the ephemeral PostgreSQL instance starts from the same state.

## User-visible acceptance

Within a fresh five-minute window the normal UI must contain:

- a completed lock episode and a later quiet lock snapshot;
- checkout waiters and a named root holder in Activity and Locks;
- one normalized checkout query with at least two physical plan IDs;
- plan text containing an index-based plan before/after and a sequential scan
  during the regression;
- a corresponding bump in statement execution/read activity;
- named commerce tables and application processes instead of hundreds of
  anonymous tenant tables;
- at least one recorded Vacuum episode;
- no demo-owned transaction or query whose age grows without bound.

No new diagnostic, incident, correlation, confidence, or cause model is added
to Kronika. The demo uses the existing shared cursor and related-row navigation
to let the visitor inspect recorded facts.
