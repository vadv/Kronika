# Kronika design

[Русская версия](DESIGN.ru.md)

Agents read this before `AGENTS.md`, which covers how to work in the
repository rather than what is being built.

## What Kronika is

Kronika records the history of a machine and the databases on it, the way
`atop` records system history, and replays it later.

The collector takes periodic snapshots of system and database metrics, parses
logs, and turns notable log events into metrics. The web part reads and displays
the collected data. Everything below serves those two sentences.

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

Optional columns must not be used to keep an id stable. That is the mistake
this rule exists to prevent.

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
- Any artifact whose purpose is to assess the data rather than store it.

A missing metric produces one warning in the collector log and a `null` in
web. That is the whole treatment.

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
container, and records the answer in the `instance_metadata` section that every
segment carries. It does not guess, and web does not re-derive it.

Where it runs decides which pressure files describe it: a machine by
`/proc/pressure`, a container by its own cgroup. The collector reads the ones
that describe itself and records the scope on every `os_psi` row.

## Health

Health is one number per snapshot, from 0 to 100: the share of the interval in
which nothing was waiting for the most contended resource.

```
health = 100 - max(cpu, memory, io)
```

Each term is the delta of the `some` counter in `os_psi` over the delta of the
timestamp. That counter is already scaled by how much of the machine the
waiting covers, so one task waiting among sixteen busy CPUs reports about a
sixteenth of the interval rather than all of it. Each CPU is weighted by its
non-idle time, so idle CPUs neither dilute nor inflate it. A reader scales
nothing further, by CPU count or by cgroup quota.

`some` rather than `full`, because the question is whether anyone lost time,
not whether everything stopped. `full` is undefined for CPU and always reads
zero, so the three resources would not be measured the same way.

The counters rather than `avg10`: a kernel average is sampled at read time and
lags. On a sixteen-core host, five seconds of contention that the counters put
at 62% read back from `avg10` as 19%.

No thresholds and no weights. The worst resource decides, because an average
hides a saturated disk behind an idle CPU.

Health is null when it cannot be computed: the first snapshot of a segment has
nothing to subtract from, and a counter that went backwards yields no stall
time to put over the interval.

A container on cgroup v1 has no `*.pressure` files and so no health. The host's
pressure belongs to the node, and standing in with it would report someone
else's numbers.

A kernel built with `CONFIG_PSI_DEFAULT_DISABLED` and booted without `psi=1`
has no health either: `/proc/pressure` is absent, or the files are there and
reading one returns `EOPNOTSUPP`. The collector handles both and says `psi=1`
in the log line, because that is the whole fix.

An OOM kill, a filesystem at zero free and cgroup throttling are not a share of
time. They stay out of the formula and are shown alongside the line. Folding
them in would need weights, and weights need tuning.

## Reading

One crate reads segments: `kronika-reader`. It takes a data directory and a
time range and returns rows, and everything that reads goes through it. A
second reading path would be a second set of bugs, and the one the tests
exercise would not be the one that ships.

A read includes finished `.zms` segments and the current logical segment from
the valid prefix of `active.wal`. Finished segments are immutable and
browser-cacheable; web refreshes only the append-only active tail.

## Index files

Web builds `.idx` files next to the segments for fast dashboard access. An
`.idx` holds what a dashboard needs without reopening every segment.

Today that is health, and nothing else. A host with no database configured is
the case that has to work first, and health is the whole of what such a host
shows. Values pulled out of PostgreSQL and PgBouncer logs come once those
sources are wired to the API.

An `.idx` records the sources that were enabled when it was built. A different
set means a different file, and web rebuilds it under the same rule as a
version it does not know.

`.idx` files are derived data. Deleting one is safe; web rebuilds it from the
`.zms`. When web finds an `.idx` written by an incompatible version, it
rebuilds it instead of failing.

## Web

Rust for the API, static JavaScript for the interface. The API comes first and
is tested on its own; the interface is written against an API that already
works.

Which sources are enabled is declared by whoever starts web, not deduced from
what the segments happen to contain. A source that is enabled but has no data
is drawn empty. A misconfigured DSN is a line in the collector's log, not a
change in the interface.

Requests carry HTTP basic authentication. Other schemes come later, and the
check sits in one place so that adding one does not touch the handlers.

### Browser caching

Web retains no in-memory segment or index cache between requests. A finished
segment is immutable, and web serves one deterministic HTTP representation per
finished segment with `Cache-Control: private, max-age=31536000, immutable`. The browser stores
that representation as immutable in its private cache.

Each finished per-segment index has one stable URL. Web serves its response with
`Cache-Control: private, no-cache`; the browser stores it and uses ordinary `ETag`
revalidation because the index is derived from the segment and may be rebuilt.
An unchanged index returns `304 Not Modified` with no body; rebuilt content has
a different `ETag`. Because requests use the HTTP `Basic` authentication
scheme, public and shared caches must not store either response.

The active WAL is append-only. Browser-held active rows and index points
refresh against the current active cursor; web does not persist an `active.idx`
or rewrite one for each snapshot. When the same `SegmentId` moves from active to
finished, its finished data and index are canonical, and the browser does not
duplicate rows.

Browser caching is only an optimization. If the browser evicts a response or
ignores cache headers, web performs a normal reread; correctness does not
change.

### Standby

Web is not a resident service. Between requests it holds nothing: buffers,
decoded sections and open segments are all released, and the next request pays
to open what it needs.

This is why the index files exist. A dashboard opening a day reads `.idx` and
not every segment of that day, so starting from nothing stays cheap.

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

## Fail fast

A component that cannot return to a known durable state fails immediately and
visibly: the log carries the full error, and the daemon exits. The next start
finishes the interrupted operation.

Startup recovery is the only recovery path. A crash and a deliberate stop run
the same code, and the tests exercise it. The journal's poisoned state is this
rule applied: a reset that cannot complete or roll back refuses further use
until reopen.

A segment is published before the journal is reset, so a failed reset stops
without risking collected data.

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
