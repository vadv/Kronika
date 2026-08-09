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
  a host already in trouble; an ordinary OS snapshot reads all of them and is
  allowed to die trying rather than report a fraction. Log files and potentially
  large PostgreSQL query results are streamed through bounded buffers. A
  PostgreSQL batch limits retained memory, not the number of source rows; each
  batch reaches the WAL before the collector fetches the next one. If a query
  fails after earlier batches reached the WAL, those batches remain and the
  collector logs the query error. It keeps no separate completeness or
  continuity state.

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

A section whose snapshot holds more than one row declares the columns that
identify one object across snapshots. Without that declaration a section is a
list of rows and nothing can ask what one disk, one interface or one table did
over time. Where rows of one section can come from more than one source, the
identity names the source too, or two sources write over each other's objects.

The registry also assigns each physical section layout a stable textual logical
name. Several physical layouts may share that name; generated compatibility
metadata decides whether their rows compose into one stream. The collector
discovers supported PostgreSQL layouts dynamically, and the catalog reports
the layouts recorded in each segment. Incompatible identities remain separate
streams with their exact physical layout and `type_id` as provenance. This
includes `pg_stat_statements` layouts before and after `toplevel` became part
of the identity, and the different `pg_store_plans` implementations. A column
absent from a physical layout is unavailable or `null`; it is never zero.

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

Where it runs decides which pressure rows describe it: host-scoped
`/proc/pressure` for a machine, and pressure from its own cgroup for a
container. Every `os_psi` row records that scope. The current procfs collector
produces host-scoped rows; when it runs in a container, their timestamps remain
visible but health is null. Node pressure must not stand in for container
pressure. Future rows from the container's own cgroup can be used without
changing the formula.

## Health

Health is up to four ordinary nullable point series from 0 to 100: OS,
PostgreSQL, PgBouncer, and their combined value. A component penalty is
`100 - health`.
Disabled optional sources contribute no penalty. An enabled source with no
usable snapshot is `null`, and makes combined health `null` rather than
silently healthy.

OS health is the share of the interval in which nothing was waiting for the
most contended resource:

```
health = 100 * (1 - max(cpu, memory, io))
```

Each term is `delta(some_total) / elapsed wall time` for that resource.
`some_total` accumulates wall-clock microseconds during which at least one task
in the recorded scope was stalled. One stalled task and several stalled tasks
count the same instant once. The counter is not weighted by task or CPU count,
and a reader does not divide it by CPU count or by a cgroup quota.

`some` rather than `full`, because the question is whether anyone lost time,
not whether everything stopped. `full` is undefined for CPU and always reads
zero, so the three resources would not be measured the same way.

The counters rather than `avg10`: a kernel rolling average is sampled at read
time and lags, so it does not describe exactly the interval between two
snapshots.

No thresholds and no weights inside the OS component. The worst resource
decides, because an average hides a saturated disk behind an idle CPU.

Health is null when it cannot be computed: the first snapshot of a segment has
nothing to subtract from; a counter that went backwards yields no stall time
to put over the interval; the pressure scope does not describe the recorded
environment; or either adjacent snapshot lacks a usable counter for CPU,
memory, or IO. When a missing resource reappears, that complete snapshot starts
a new baseline.

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

PostgreSQL pressure uses the number of rows whose `pg_stat_activity.state` is
exactly `active` in one snapshot. It does not split running and waiting
backends: both are active work the server must service. It does not use
`max_connections`, because that limit says nothing about the CPU capacity
available to the workload.

The operator records the monitored server's positive effective CPU capacity in
`KRONIKA_POSTGRES_EFFECTIVE_CPUS`. A metric DSN can be remote, so Kronika never
substitutes the collector host's CPU count. One effective CPU supplies two
service slots: one backend may execute while another sends results to its
client.

```
service_slots = 2 * effective_postgres_cpu
postgres_penalty = 0                                      when active <= service_slots
postgres_penalty = round(100 * (active-service_slots)/active) otherwise
postgres_health = 100 - postgres_penalty
```

For a two-CPU server, the first penalty is at five active backends. Enabled
PostgreSQL without a configured capacity or a usable activity snapshot is
`null`. The enabled flags, PostgreSQL capacity, and effective PostgreSQL
collection interval are recorded once in the current `instance_metadata` row
of every segment. When the PostgreSQL interval is configured as zero, the
recorded effective interval is the collector timer tick.

PgBouncer pressure must come from an actual pool/queue snapshot. Log events and
`SHOW CONFIG` do not say how many clients are queued. The current collector has
no such snapshot, so configured PgBouncer health and combined health remain
`null`; disabled PgBouncer contributes no penalty. A future collector must add
the pool signal before assigning a numeric penalty.

```
overall_health = clamp(100 - os_penalty - postgres_penalty - pgbouncer_penalty,
                       0, 100)
```

Each combined point has an OS-health timestamp. It uses the latest PostgreSQL
snapshot not later than that timestamp only while its age is at most the
recorded effective PostgreSQL interval. There is no interpolation: before the
first PostgreSQL snapshot and after a stale one the combined value is `null`.
It also stays `null` while any other enabled component is unknown. The index
always exposes OS and combined health; it exposes each optional component only
when that source is configured. These blocks contain only small points, never
large source rows.

## Reading

One crate reads segments: `kronika-reader`. It takes a data directory and a
time range and returns rows, and everything that reads goes through it. A
second reading path would be a second set of bugs, and the one the tests
exercise would not be the one that ships.

A read includes finished `.zms` segments and the current logical segment from
the valid prefix of `active.wal`. Finished segments are immutable and
browser-cacheable. Web revalidates the catalog and refreshes append-only active
resources when a user requests current data.

## Index files

Web builds `.idx` files next to the segments for fast dashboard access. An
`.idx` holds segment-grain summaries so a long-range dashboard does not reopen
every segment body.

The file has a header, a table of contents and blocks. Every physical section
included in the index has its own targeted block, so a request decodes only the
section it needs. The table records each block's kind, layout, offset and
length. Offsets, lengths and the checksum belong to the file; section identities
and summaries belong to the block. Health follows the same indexed-series rules
as other metrics and has no special global block.

The index grain is one segment. For every object in an indexed physical
section, its block keeps the exact identity and enough data to reproduce the
section's whole-segment result: first and last observed timestamps and values,
or an equivalent counter delta and observed duration, plus the last gauge
sample. Missing and invalid inputs remain distinguishable from a real zero.

An index does not copy every `Label` column. Query text, plans, command lines
and similar display values would duplicate the largest fields in the segment.
After a heatmap selects its identities, a projected raw response supplies only
the display labels for those identities.

An `.idx` carries a checksum of its contents in its header. That is what a
browser revalidates against, so the file has to hold it rather than have web
compute it per request. Web writes a complete temporary index and replaces the
derived file atomically.

`.idx` files are derived data. Deleting one is safe; web rebuilds it from the
`.zms`. When web finds an `.idx` written by an incompatible version, it
rebuilds it instead of failing.

The index format is accepted only after measuring both `.idx` size and web peak
RSS through the production path. The measurement includes at least 5,000
`pg_stat_statements` rows and 5,000 `pg_store_plans` rows with their large text
fields.

## Web

Rust for the API, static JavaScript for the interface. The API comes first and
is tested on its own; the interface is written against an API that already
works.

Web configuration selects which source families the interface shows. It never
selects a physical PostgreSQL layout; the catalog reports the layouts actually
present. A configured source with no data is drawn empty. A misconfigured DSN
is a line in the collector's log, not a change in the interface.

Requests carry HTTP basic authentication. Other schemes come later, and the
check sits in one place so that adding one does not touch the handlers.

### Segment resources

HTTP exposes cacheable resources for explicit segments. It has no generic
query language or global health, top, series or rows calls. The route names
below are sketches rather than a framework choice:

- `/api/catalog` lists finished `SegmentId` values, their time bounds and the
  physical section layouts actually present. It also lists the active
  `SegmentId`, its actual sections and the cursor at the committed valid WAL
  prefix.
- `/api/segments/{segment_id}/sections/{logical_name}/index` returns one
  finished segment's derived index representation for that section.
- `/api/segments/{segment_id}/sections/{logical_name}/history` returns selected
  fields for one or more series in that segment. Label filters are exact
  equality matches.
- `/api/segments/{segment_id}/sections/{logical_name}/rows` returns raw rows in
  pages with a stable order and a next-page cursor.
- Active paths expose the same section projections and an append-only tail for
  the active `SegmentId` up to a returned WAL cursor.

The catalog reports which layouts are present, not their schemas. Registry and
layout metadata are generated from the compiled Rust registry into the static
JavaScript data module shipped with the same web build. It includes fields,
kinds, units, identities and layout-compatibility rules. There is no separately
maintained runtime schema service and no API-version field. A representation
header may name its exact physical layout, columns, kinds and units.

The interface never hardcodes which native PostgreSQL, `pg_stat_statements` or
`pg_store_plans` layout is installed. It addresses a section by its stable
logical name and retains the physical layout and `type_id` from the catalog or
response header as provenance.

### JavaScript data client

The small static data client composes segment resources into `listMetrics`,
`listSeries`, `history`, `heatmap` and `rows`. `listMetrics` combines the
configured source families, co-shipped registry metadata and layouts found in
the catalog. `listSeries` discovers identities and applies exact label filters.
The other calls request every intersecting segment and combine finished and
active representations by `SegmentId`. `heatmap` derives the ranked top view in
the client; HTTP has no top entity. Health is an ordinary indexed time series
available through `history` and section indexes.

History can select several fields and series. The client requests every segment
that intersects the window. Neither client nor server applies an implicit limit
to the selected fields, series or segments.

Raw rows use `page_size >= 1`. A page has a stable order and a next cursor, and
the reader stops work when the page is full. Zero is rejected rather than
coerced. The same rule applies to zero heatmap columns and `top=0`. In
`top=N`, `N` is the number of identities returned; it is not a scan or memory
limit.

### Heatmap values

Every heatmap column carries its exact interval boundaries. For a counter, a
cell is the last value minus the first value for that identity in the interval,
divided by the elapsed time between those two observations. Missing input,
fewer than two usable observations, a non-positive observed duration or a
negative delta produces `null`. A zero delta produces `0`. For a gauge, the
cell is the last sample in the interval, or `null` when no usable sample exists.

Ranking uses the whole requested window and does not change with the number of
columns. The first pass scans the whole window and selects the top K identities.
Only the second pass allocates the K-by-column result and fills its cells. The
requested K limits the returned identities, not the work needed to rank them.
Long ranges use segment-grain `.idx`; sub-segment resolution and partial
boundary segments use projected raw samples.

### Representations

Potentially large textual section responses are streamable, for example as
NDJSON. Every 64-bit integer and cursor component is decimal text so JavaScript
does not lose precision. A blob value carries the stored bytes and the recorded
`full_len`, `truncated` and `hash` metadata.

The server returns codes and data: unit and kind names, logical section names,
physical layouts, column names and unix times. It reads no `Accept-Language`,
translates nothing and formats nothing. The interface holds the words for every
language it ships and decides how to display numbers and times. A table name,
database name, statement or log line leaves the server as recorded.

Synchronous disk reads, decompression and Parquet work run outside async
handlers. A slow section read must not block unrelated requests.

### Browser caching

Web keeps no segment, index or decoded-data cache between requests. The catalog
uses `Cache-Control: private, no-cache`. Finished raw and projected section
representations use
`Cache-Control: private, max-age=31536000, immutable`; all selection,
projection and ordering parameters are part of their URL.

Each finished per-section derived index has a stable URL and uses
`Cache-Control: private, no-cache`. The browser revalidates it with the `ETag`
from the `.idx` checksum. An unchanged index returns `304 Not Modified` with no
body. Active resources use `Cache-Control: private, no-store`. Basic
authentication keeps all of these representations out of public and shared
caches.

An active cursor is `(segment_id, wal_position)`, where `wal_position` is the
committed end of the valid WAL prefix. It is never a metric timestamp, so rows
with equal timestamps remain distinguishable. Active index points are computed
for the response; web writes no `active.idx`.

The active and finished forms retain the same `SegmentId`. When the segment
finishes, its finished resource becomes canonical. The data client replaces
the active form for that `SegmentId` instead of appending another copy, so the
transition cannot duplicate rows. It then follows the new active `SegmentId`
from the catalog.

A request refreshes the tail; a timer must not. A front end that polls on an
interval keeps web awake for nobody, which is the one thing standby exists to
prevent.

Browser caching is only an optimization. If the browser evicts a response or
ignores cache headers, web performs a normal reread; correctness does not
change.

### Standby

Web is not a resident service. Between requests it holds nothing: buffers,
decoded sections and open segments are all released, and the next request pays
to open what it needs.

This is why the index files exist. A long-range dashboard reads segment-grain
indexes for complete segments and raw projections only where exact boundaries
require them, so starting from nothing stays cheap.

### Web BDD

Web BDD runs only in CI. Its scenarios cover:

- discovery of actual `pg_stat_statements` and `pg_store_plans` extension
  layouts and their textual fields without frontend layout constants;
- multi-field, multi-series history with exact label filters, and raw-row
  pagination including rejected zero limits;
- every cache policy and header above, `ETag` revalidation and
  `304 Not Modified`;
- equal-timestamp active rows, WAL-position cursors and the active-to-finished
  transition for one `SegmentId` without duplicates;
- exact heatmap intervals, counter and gauge values, `null` and real-zero
  behavior, whole-window ranking independent of column count, and top K;
- lossless JavaScript output for 64-bit values and blobs; and
- bounded peak memory for a large individual object.

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
