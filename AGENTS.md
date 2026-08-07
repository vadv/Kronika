# Kronika agent instructions

This file is the contract for every coding agent working in this repository:
Codex, Claude, Kimi. `CLAUDE.md` is a symlink to this file. Read it before
touching code and follow it over your own defaults.

Ask the owner when something here is missing or contradicts the task. Do not
invent a rule and proceed.

## What Kronika is

Kronika records the history of a machine and the databases on it, the way
`atop` records system history, and replays it later.

The collector takes periodic snapshots of system and database metrics, parses
logs, and turns notable log events into metrics. The web part reads what the
collector wrote and shows it. Everything else in this document exists to serve
those two sentences.

Two processes, different duty cycles:

- **collector** runs all the time on the monitored host.
- **web** runs occasionally, when a person opens it.
- **sync** moves old segments to S3-like storage in the background.

## The value we protect

Minimum memory and CPU. This is the reason the project exists, and it outranks
speed, elegance, and feature count.

Concrete consequences:

- When web has to scan an hour of data, it takes longer and uses less RAM and
  CPU. Trade time for footprint, not the other way around.
- Web has a standby mode. With no human traffic, it drops everything it holds
  and goes quiet. Serving Prometheus `/metrics` does not count as traffic and
  must not wake the heavy paths.
- The collector shares a host with a production database. An out-of-memory kill
  there costs more than a lost segment.

Every new code path needs an answer to: what is the peak memory, and what
enforces the bound? A config limit, a format constant, or the size of an input
the caller already holds are bounds. "Usually small" is not.

## Storage format

Fresh data goes to a raw write-ahead log (`.wal`). On a size threshold or a
timer, the collector compresses it into a segment.

A segment is `.zms` (compressed metric segment), stored at `YYYY/MM/DD/ts.zms`.
It is independent and self-contained: opening one segment requires no other
file, no external schema, no registry lookup at runtime.

Segments are optimized for size above everything else:

- The segment carries no description of how to interpret or unpack itself. This
  project is the only consumer and it already knows.
- Strings are the main cost. Normalize repeated strings to a `sha256` and store
  references.
- Small strings are stored as-is. Compressing them costs more than it saves.

Segments live on local disk and on S3-like storage. Old segments move to S3 in
the background.

Any change to the segment framing, encoding, or dictionary must be measured for
size on demo data before it is proposed. Report the before and after numbers in
the PR. A framing change without a size benchmark is not reviewable.

## Metric registry discipline

A segment holds many metrics and the set is extensible. Each metric has an id.

**Any change to a metric's fields creates a new metric id.** There is no
backward compatibility inside a metric id, and none is wanted.

- `pg_stat_statements` v1.2 and v1.3 are separate metric ids.
- Adding one field to the existing v1.2 shape also creates a new metric id.

Do not add optional columns to keep an id stable. That is the mistake this rule
exists to prevent.

Every metric declares its kind and its unit, the way Prometheus does:

- `gauge` for a value that goes up and down.
- `counter` for a value that only grows.
- `event` for a discrete occurrence. A PostgreSQL `statement_timeout` is an
  event. It is not a counter and not a gauge, and forcing it into either loses
  what happened and when.

Units are part of the declaration: seconds, bytes, and so on.

## Where the collector is running

The collector decides at collection time whether it is on a VM or inside a
container, and writes the answer into the segment header. It does not guess,
and web does not re-derive it.

This matters because the numbers differ. A pod has a CPU limit; a VM has a
physical CPU count. Health is computed against the CPU limit inside a
container and against the CPU count on a VM.

## Health and index files

Web builds `.idx` files next to the segments for fast dashboard access. An
`.idx` holds what a dashboard needs without reopening every segment: critical
values extracted from logs, and health.

Health is a computed metric derived from other metrics, load average among
them.

`.idx` files are derived data. Deleting one is safe; web rebuilds it from the
`.zms`. When web finds an `.idx` written by an incompatible version, it
rebuilds it instead of failing.

## Logging

Logs are the product's second output. Treat them with the same care as metrics.

Collector:

- Every error is logged with enough detail to act on it. No swallowed errors,
  no bare "failed".
- Sealing a segment logs how long it took and what it cost. Cheap counters and
  timings, enough for an operator to see the shape of the work.
- A metric that could not be collected is logged as such by the collector.

Web:

- Logs what it opened and what index it built, with timings and the same cheap
  counters.
- Shows `null` for a metric the collector failed to collect. Web does not
  invent, interpolate, or hide the gap.

All log messages are English. See the language rules below.

## Testing

BDD tests come first. Feature files describe observable behavior, and the steps
assert it against a real artifact, not a mock.

BDD runs inside a cached Docker image so the environment is reproducible and CI
does not rebuild the world on every run. Parsing collector log messages is the
reference case: write the feature against the log output an operator would
read.

Pure functions get unit tests in the same change. Put tests in their own files
rather than growing the module they cover.

Do not write `@wip` feature files with no step definitions. A scenario that
asserts nothing is worse than no scenario.

## Demo

The repository ships a demo that runs the project against a live PostgreSQL and
OS container, and it shows the stages of the project as they land.

When the collector becomes runnable, the demo runs it and reports segment size,
RSS, and CPU consumed. Every later stage extends the same demo. The demo is
also the data source for the size benchmarks required above.

## Rust rules

- Write plain Rust a newcomer can read. Simple control flow, simple types, no
  clever generics or macro tricks where a function does the job.
- Do not multiply entities. No trait with one implementation, no factory for
  one product, no config for a value that never changes.
- Split large files into small ones in their own directories. A file that has
  grown past comprehension gets split, not a table of contents comment.
- Keep tests in separate files from the code they test.
- Handle errors explicitly. No `unwrap()` or `expect()` on a path that can fail
  in production.
- Lean on the tooling. Clippy runs strict, warnings are errors, and a lint you
  disagree with gets discussed before it gets an `#[allow]`.

Before proposing anything: `cargo fmt --all --check`, clippy with warnings
denied across the workspace and all targets, and the full test suite.

## Pull requests and review

Make PRs large enough to deliver a working piece of behavior, with as many
commits as the work needs. Do not split a change into fragments that nobody can
review on their own.

Before opening a PR or merging one, run a review panel and fix every **high**
finding:

1. **Rust performance** — hot paths, allocations, needless clones, per-row
   work, async overhead.
2. **PostgreSQL DBA** — query cost and locking behavior on the monitored
   instance, correctness across supported PostgreSQL versions, safe SQL.
3. **Rust architect** — module and crate boundaries, public API shape, whether
   the change fits the design or bends it.

When a reviewer proposes something that makes the program more complicated, do
not apply it silently. Ask the owner whether the complexity is worth it. A
review comment can be generated filler, and filler that lands as an abstraction
is expensive to remove later.

## Language

The product is bilingual from the start, `ru` and `en`, with more languages
later. Build user-facing strings and docs so a third language is a translation,
not a rewrite.

Fixed rules:

- **Log messages: English only.** No exceptions, including messages that are
  only ever read during development.
- **Code comments: simple English.** Write what the code cannot say: an
  invariant, a trade-off, a reason. A comment that restates the next line is a
  defect to delete.
- **Commit messages: English.** Say what the change does to the behavior of the
  system and why. Do not list the files you touched.

All three are checked at review. Padding, throat-clearing, and generated prose
that says nothing are blocking findings, in comments and in commit messages
alike.

## Roadmap

Collector:

1. System metrics.
2. Log parsing primitives.
3. PostgreSQL log handling.
4. PostgreSQL metrics.
5. Other databases: MySQL, ClickHouse, CockroachDB.

Web:

1. Day and hour selection.
2. OS metrics.
3. Log events.
4. PostgreSQL metrics.
5. ClickHouse, CockroachDB, MySQL.

Work the list in order. When a step needs to move, ask first.
