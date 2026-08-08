# Kronika design

[Русская версия](DESIGN.ru.md)

## What Kronika is

Kronika records the history of a machine and the databases on it, the way
`atop` records system history, and replays it later.

The collector takes periodic snapshots of system and database metrics, parses
logs, and turns notable log events into metrics. The web part reads and displays
the collected data.

Three duty cycles:

- **collector** runs all the time on the monitored host.
- **web** runs occasionally, when a person opens it.
- **sync** moves old segments to S3-like storage in the background.

## Resource priority

Low memory and CPU use take priority over speed, implementation elegance, and
feature count.

Concrete consequences:

- When web has to scan an hour of data, it takes longer and uses less RAM and
  CPU. Trade time for footprint, not the other way around.
- Web has a standby mode. With no human traffic, it drops everything it holds
  and goes quiet. Serving Prometheus `/metrics` does not count as traffic and
  must not wake the heavy paths.
- The collector shares a host with a production database. An out-of-memory kill
  there costs more than a lost segment.
- **The collector's peak RSS stays under 20 MB on an ordinary host**, and each
  segment write logs it as `rss_kib`. A host with thirty thousand processes is
  a host already in trouble; the collector reads all of them and is allowed to
  die trying rather than report a fraction. Log files are the exception: their
  size is set by someone else's software and they are read through a fixed
  buffer.

## Storage format

Fresh data goes to a raw write-ahead log (`.wal`). On a size threshold or a
timer, the collector compresses it into a segment.

A segment is `.zms` (compressed metric segment), stored at `YYYY/MM/DD/ts.zms`.
It is independent and self-contained: opening one segment requires no other
file, no external schema, no registry lookup at runtime.

Segments are optimized for size above everything else:

- The segment stores no schema description. Kronika readers use the compiled
  registry to decode it.
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

Optional columns must not be used to keep an id stable.

Every metric declares its kind and its unit, the way Prometheus does:

- `gauge` for a value that goes up and down.
- `counter` for a value that only grows.
- `event` for a discrete occurrence. A PostgreSQL `statement_timeout` is an
  event. It is not a counter and not a gauge, and forcing it into either loses
  what happened and when.

Units are part of the declaration: seconds, bytes, and so on. The column
contract stores the unit as compile-time data. It never reaches the segment,
costs no disk space, and does not change a metric id.

## What Kronika does not build

Kronika does not evaluate whether its own data is trustworthy, complete, or
continuous. That work would consume resources needed to collect metrics.

Specifically, none of this belongs in the project:

- Reset detection and reset bookkeeping. A counter that goes backwards is a
  counter that went backwards. Record the value, move on.
- Completeness accounting: "collected N of M", coverage tallies, per-section
  scoreboards of what did or did not arrive.
- Any machinery built around missing intervals. Snapshots with nothing between
  them are the normal state of a monitoring system, not a defect to detect,
  classify, or report.
- Any artifact whose purpose is to assess the data rather than store it.

A missing metric produces one warning in the collector log and a `null` in
web.

### Banned words

The following words are banned in code, comments, logs, commit messages, and
docs:

| Banned | Use instead |
|--------|-------------|
| seal, sealing | write, close |
| evidence, proof | damaged, broken |
| gap | (nothing — do not name the concept) |

Write plainly. The collector writes a segment. A corrupt journal part is
damaged and gets set aside. Two snapshots an hour apart are two snapshots.

## Where the collector is running

The collector decides at collection time whether it is on a VM or inside a
container, and records the answer in the `instance_metadata` section that every
segment carries. It does not guess, and web does not re-derive it.

A pod has a CPU limit; a VM has a physical CPU count. Health uses the CPU limit
inside a container and the CPU count on a VM.

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

Logs are part of the product output and carry the same weight as metrics.

Collector:

- Every error is logged with enough detail to act on it. No swallowed errors,
  no bare "failed".
- Writing a segment logs elapsed time, segment and journal bytes, section and
  journal-part counts, timestamps, and peak RSS.
- A metric that could not be collected is logged as such by the collector.

Web:

- Logs what it opened and what index it built, with timings and the same cheap
  counters.
- Shows `null` for a metric the collector failed to collect. Web does not
  invent or interpolate a value it does not have.

## Demo

The repository demo runs the project against live PostgreSQL and OS containers.

The demo reports segment size, RSS, and CPU use. It also supplies data for
segment-size benchmarks.

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
6. A dumper: what a segment's size is made of.

Work the list in order. Moving a step needs the owner's agreement.
