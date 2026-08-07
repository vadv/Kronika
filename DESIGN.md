# Kronika design

What the project is and how its pieces fit. Agents read this before
`AGENTS.md`, which covers how to work in the repository rather than what is
being built.

## What Kronika is

Kronika records the history of a machine and the databases on it, the way
`atop` records system history, and replays it later.

The collector takes periodic snapshots of system and database metrics, parses
logs, and turns notable log events into metrics. The web part reads what the
collector wrote and shows it. Everything below serves those two sentences.

Three duty cycles:

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

## Metric registry

A segment holds many metrics and the set is extensible. Each metric has an id.

**Any change to a metric's fields creates a new metric id.** There is no
backward compatibility inside a metric id, and none is wanted.

- `pg_stat_statements` v1.2 and v1.3 are separate metric ids.
- Adding one field to the existing v1.2 shape also creates a new metric id.

Optional columns must not be used to keep an id stable. That is the mistake
this rule exists to prevent.

Every metric declares its kind and its unit, the way Prometheus does:

- `gauge` for a value that goes up and down.
- `counter` for a value that only grows.
- `event` for a discrete occurrence. A PostgreSQL `statement_timeout` is an
  event. It is not a counter and not a gauge, and forcing it into either loses
  what happened and when.

Units are part of the declaration: seconds, bytes, and so on. The unit lives in
the column contract, which is compile-time data and never reaches the segment,
so declaring it costs nothing on disk and never changes a metric id.

## What Kronika does not build

There is a metric and there is data. Nothing else.

Kronika does not ship a layer that reasons about whether its own data is
trustworthy, complete, or continuous. That layer catches nothing an operator
cares about, and the effort it absorbs comes straight out of collecting more
metrics.

Specifically, none of this belongs in the project:

- Reset detection and reset bookkeeping. A counter that goes backwards is a
  counter that went backwards. Record the value, move on.
- Completeness accounting: "collected N of M", coverage tallies, per-section
  scoreboards of what did or did not arrive.
- Any machinery built around missing intervals. Snapshots with nothing between
  them are the normal state of a monitoring system, not a defect to detect,
  classify, or report.
- Any artifact whose purpose is to prove something about the data rather than
  to be the data.

A missing metric is one warning line in the collector log and a `null` in web.
That is the whole treatment.

This section outranks a reviewer's suggestion. When a review proposes adding
one of these, the answer is no, and the reason is this paragraph.

### Banned words

Three words are banned in code, comments, logs, commit messages, and docs,
because each one drags the machinery above back in behind it:

| Banned | Use instead |
|--------|-------------|
| seal, sealing | write, close |
| evidence, proof | damaged, broken |
| gap | (nothing — do not name the concept) |

Write plainly. The collector writes a segment. A corrupt journal part is
damaged and gets set aside. Two snapshots an hour apart are two snapshots.

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

Logs are the product's second output, and carry the same weight as metrics.

Collector:

- Every error is logged with enough detail to act on it. No swallowed errors,
  no bare "failed".
- Writing a segment logs how long it took and what it cost. Cheap counters and
  timings, enough for an operator to see the shape of the work.
- A metric that could not be collected is logged as such by the collector.

Web:

- Logs what it opened and what index it built, with timings and the same cheap
  counters.
- Shows `null` for a metric the collector failed to collect. Web does not
  invent or interpolate a value it does not have.

## Demo

The repository ships a demo that runs the project against a live PostgreSQL and
OS container, and it shows the stages of the project as they land.

When the collector becomes runnable, the demo runs it and reports segment size,
RSS, and CPU consumed. Every later stage extends the same demo. The demo is
also the data source for the segment size benchmarks required by `AGENTS.md`.

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

Work the list in order. Moving a step needs the owner's agreement.
