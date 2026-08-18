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
- **The collector's peak RSS stays under 25 MiB on an ordinary host**, and each
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

There is recorded metric data. Kronika adds no collection-quality model.

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
- Any artifact whose purpose is to assess collection completeness or
  continuity rather than store metrics.

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

The recorded environment gates cgroup collection. On a machine, including a
VM, the collector emits no `os_cgroup_context`, `os_cgroup_cpu`,
`os_cgroup_memory`, `os_cgroup_io`, or `os_cgroup_pids` rows. The presence of a
cgroup hierarchy is never used as a virtualization signal. Inside a container,
each cgroup tick records context and controller rows only for cgroups that
directly contain a live process visible in that namespace. It never walks
inactive nodes or ancestor trees. The pass is limited to 512 controller/path
candidates, 512 KiB of candidate paths, and 1,024 I/O cgroup/device rows per
tick. An exceeded candidate bound omits the workload sections atomically; an
exceeded I/O-row bound omits I/O only.

Inside a container, `os_cgroup_context` records cgroup version, the collector's
exact CPU, memory, and I/O paths from `/proc/self/cgroup`, and the effective
cpuset CPU count when the exact matching kernel file is usable. It also records
the tightest CPU quota/period ratio and memory limit that apply to that
membership. The hierarchy starts at the configured cgroup root and ends at the
exact membership path. Every cgroup v2 control file that exists on this path
must be valid. For a non-root membership only, a missing control file at the
mount root means that true root is unbounded; every descendant is required. A
different root read error, or a missing root file for root membership, leaves
capacity unknown. Cgroup v1 CPU reads are bound to one unambiguous controller
root. V1 memory uses the exact leaf's validated `hierarchical_memory_limit` only
when it agrees with `memory.limit_in_bytes` from that same bound root.

A CPU quota of `-1` means the applicable hierarchy was read coherently and is
unlimited; `null` means that hierarchy was not established coherently. A memory
limit is `null` when it is unlimited or cannot be read coherently. Missing
controllers and unusable cpuset data also remain `null`. A controller path
stays `null` when the stored layout cannot represent every operand shown by
web;
a missing counter or composition field is never turned into zero. This one-row
context selects the collector's rows from the bounded cgroup tables; a host CPU
count or host `/proc` value never substitutes for cgroup capacity or use. Local
cgroup rows continue to record the leaf controller files and do not relabel
them as effective hierarchical limits.

Web reads `instance_metadata.environment`. It hides Cgroups and requests no
cgroup snapshots for a machine. In a container it exposes CPU, Memory, I/O and
Tasks and loads the complete already-bounded direct-live rows recorded in that
namespace; the context row decorates only the collector's matching controller
row. The controller paths remain independent for cgroup v1. A recursive cgroup
tree is never materialized in `HourData`.

Where it runs decides which pressure rows describe it: host-scoped
`/proc/pressure` for a machine, and pressure from its own cgroup for a
container. Every `os_psi` row records that scope. The current procfs collector
produces host-scoped rows; when it runs in a container, their timestamps remain
visible but health is null. Node pressure must not stand in for container
pressure. Future rows from the container's own cgroup can be used without
changing the formula.

## Health

Health is up to three ordinary nullable point series from 0 to 100: OS,
PostgreSQL, and their combined value. A component penalty is `100 - health`.
Disabled PostgreSQL contributes no penalty. Enabled PostgreSQL with no usable
snapshot is `null`, and makes combined health `null` rather than silently
healthy.

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

Health is null when it cannot be computed: the first recorded snapshot has
nothing to subtract from; a counter that went backwards yields no stall time
to put over the interval; the pressure scope does not describe the recorded
environment; or either adjacent snapshot lacks a usable counter for CPU,
memory, or IO. The first snapshot in a later segment uses the immediately
preceding recorded snapshot when the environment and boot identity match;
storage boundaries do not reset the baseline. When a missing resource
reappears, that complete snapshot starts a new baseline.

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
`null`. The PostgreSQL enabled flag, capacity, and effective collection
interval are recorded once in the current `instance_metadata` row
of every segment. When the PostgreSQL interval is configured as zero, the
recorded effective interval is the collector timer tick.

```
overall_health = clamp(100 - os_penalty - postgres_penalty, 0, 100)
```

Each combined point has an OS-health timestamp. It uses the latest PostgreSQL
snapshot not later than that timestamp, including one from the preceding
segment, only while its age is at most the recorded effective PostgreSQL
interval. There is no interpolation: before the first PostgreSQL snapshot and
after a stale one the combined value is `null`. The index always exposes OS and
combined health; it exposes PostgreSQL health only when that source is
configured. These blocks contain only small points, never large source rows.

## Highlighting

Kronika records discrete snapshots, not continuous history. For every stored
row in these exact PostgreSQL event-stream layouts, IDX records one `event`
locator before applying the existing per-section cap:

- `2_001_001` `pg_log_errors`;
- `2_002_001` `pg_log_checkpoints`;
- `2_003_001` `pg_log_autovacuum`;
- `2_004_001` `pg_log_slow_queries`;
- `2_005_001` `pg_log_lock_waits`;
- `2_006_001` `pg_log_lifecycle`.

This list is exhaustive; registry metadata does not expand it. Separately,
Kronika adds one independent best-effort visual mark. `known_bad` means an
exact stored value crossed one small explicit boundary. Value colour and marks
remain separate.

The implementation uses explicit field matches and ordinary comparisons. It
has no policy or expression framework, persistent baseline, cadence or
continuity check, future confirmation, alert, incident, confidence, score,
cause, diagnosis, or correlation.

Zero is data and participates in comparisons. `null` is not a value. Web does
not interpolate it or carry a previous value across it.

### Known-bad boundaries

The initial exact comparisons are:

- a recorded slow-query occurrence lasts at least 5 seconds;
- aggregate CPU busy is at least 80% between two stored snapshots;
- host `load1` divided by the exact same-snapshot online CPU count is at least
  2;
- available host memory is at most 10% of the stored host memory total;
- a local filesystem with exact stored capacity is at least 90% used;
- overall health is below 50;
- the host OOM-kill counter or a database deadlock counter increases;
- active PostgreSQL backends exceed twice the configured positive effective
  PostgreSQL CPU count.

Optional or missing inputs produce no mark. Kronika does not substitute a
cgroup quota for the host CPU denominator, approximate an absent capacity, or
use a grouped duration sum as one event duration.

### IDX locators

Web records event locators and known-bad marks while building an index
through the production reader. When prior values are needed, it reads
preceding finished ZMS directly, never another IDX. Temporary state is
discarded after the build. The collector does not compute findings, and there
is no `active.idx`.

Each block stores only compact locators for one physical section. The
containing IDX is bound to its exact finished source ZMS. A record contains the
locator kind, physical `type_id`, field ordinal, current timestamp, and row
ordinal; together these identify the source row. An `event` locator uses field
ordinal 0, the row's `ts` field. A slow-query row at or above 5 seconds also
keeps its independent `known_bad` locator for `max_duration_ms`.

Only a `pg_log_errors` event locator also carries the row's stored one-byte
category: `0` lock, `1` constraint/data-integrity, `2` serialization, `3`
timeout, `4` resource, `5` data corruption, `6` system, `7` connection, `8`
auth, `9` syntax, or `10` other. IDX reads this byte directly and does not
classify SQLSTATE. The other five log layouts omit category because their
physical `type_id` already identifies the event class. HTTP and dump expose
the numeric category only on an error event locator.

`pg_log_temp_files` remains a raw `event_stream` storage section, but it is not
an operator event: it has no finding locator and does not appear in Events or
on the shared timeline. Raw temporary-file rows remain available through
ordinary history and row reads.

Derived overall health uses its compact health-point ordinal. Blocks do not
copy severity, SQLSTATE, messages, statements, identities, values, labels,
query or plan text, command lines, rows, or histories.

One fixed per-block cap keeps the format bounded. Stored locators remain in
deterministic timestamp and locator order; `total_hits` and `truncated` make an
omission visible. This is not ranking.

An hour response filters each stored locator to the requested inclusive
`[from,to]` before emitting it or counting it. If a source block was already
truncated and its omitted tail may intersect the hour, the filtered count
covers only returned in-window locators and `truncated` remains true. The hour
never counts a locator known to be outside its bounds.

The IDX format is unreleased. `KRNIDX6` is its one current reader and writer and
changes in place. Web discards and rebuilds any other IDX; there is no
old-format reader, migration, compatibility branch, or dual write.

### One timeline, no diagnosis

Kronika places event locators and independent marks on one timeline. An
`event` locator says only that the source row was recorded; it is not a visual
mark, anomaly, alert, incident, severity, cause, diagnosis, or correlation.
Kronika does not group marks into incidents or infer a main symptom, severity,
cause, relationship, confidence, or diagnosis. Several unrelated problems may
coexist, and the person examining the recorded data is the sole judge.

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
`.idx` holds compact segment-grain summaries for a curated set of presentation
fields plus the sparse findings defined above. Fields outside the summary
allowlist are read from ZMS; IDX does not promise a scan of every metric.

The file has a header, a table of contents and typed blocks. Each block belongs
to one physical section, so a request decodes only the section and block kind it
needs. The table records each block's kind, layout, offset and length. Offsets,
lengths and the checksum belong to the file; each block carries its own summary
or finding payload. Health follows the same indexed-series rules as other
metrics and has no special global block.

A summary block's grain is one segment. For every object included in that
block, it keeps the exact identity and enough data to reproduce the
whole-segment result: first and last observed timestamps and values, or an
equivalent counter delta and observed duration, plus the last gauge sample.
Missing and invalid inputs remain distinguishable from a real zero.

An index does not copy every `Label` column. Query text, plans, command lines
and similar display values would duplicate the largest fields in the segment.
After a heatmap selects its identities, a projected raw response supplies only
the display labels for those identities.

An `.idx` carries a checksum of its contents in its header. That is what a
browser revalidates against, so the file has to hold it rather than have web
compute it per request. Web writes a complete temporary index and replaces the
derived file atomically.

`.idx` files are derived data. Deleting one is safe; web rebuilds it from ZMS
files. When web finds an `.idx` written by an incompatible version, it
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

Direct API requests accept HTTP Basic authentication. The browser sends Basic
only to create a signed first-party HttpOnly session cookie, then uses that
cookie for protected API requests. The check stays in one place outside the
handlers.

### Shipped interface

The React and TypeScript interface is built ahead of Rust into one
self-contained HTML document, compressed reproducibly and committed as one
gzip artifact. `kronika-web` embeds those exact bytes, so an ordinary Cargo
build needs no Node installation. Fonts, icons, styles and scripts are local;
the production document makes no external asset requests.

The build fully validates and fingerprints that embedded artifact. Web keeps
only the gzip bytes in steady state. A shell request without `Accept-Encoding`,
or one that selects `identity`, receives readable HTML decompressed directly
into bounded response chunks; it never creates or retains the complete identity
document. HEAD and matching validators use build-time metadata without decoding,
while a gzip-capable client receives the original bytes without decoding. Both
representations have their own strong ETag and exact length, and responses vary
on authorization and content encoding.

English and Russian source dictionaries are flat YAML files. The interface
build rejects duplicate keys, empty values, unequal key sets and unequal
placeholders, then generates the compact typed dictionaries shipped in the
document. A saved locale wins over `navigator.languages`, with English as the
fallback; source values, identifiers, queries and command lines are never
translated.

Wording follows one rule with two halves. A label carries the term the trade
already uses, in English, because that is how the counter is named in `top`,
`atop` and `pg_stat_*`: `Major page faults`, `Seq scans`, `Tuples updated`,
`Autovacuum`, `WAL`, `PSI`, `OOM kills`. Translating those into Russian breaks
recognition, and mixing the two inside one label is worse than either. Only the
grammatical frame and the units stay Russian, including the genitive that
fractions require.

A help string is the opposite. It explains the counter in plain Russian and does
not repeat the English term standing next to it. It says what the number really
measures, when it grows, and whether a high value is worse or better, in a
sentence or two, without pointing at another screen. One concept keeps one name
everywhere; a counter named twice is a defect, and drift in the English
dictionary is fixed before the Russian one is translated from it.
The buffer/block byte families are one such contract: labels such as `Shared
buffer read bytes`, `Shared buffer hit bytes`, and their local, temporary,
heap, index, and TOAST counterparts remain the same natural English terms in
both dictionaries; Russian help explains their semantics in Russian.

The interface covers one selected calendar hour. Host contains dense System
metric groups and virtualized Processes lenses; PostgreSQL contains Overview,
Activity, Statements, Locks and Databases whenever their sections are present.
Events expands the same findings drawn on the shared healthline. The timeline
always spans the complete hour, does not connect missing periods and drives
every view with one cursor. Marker shape identifies log events and threshold
crossings.
The selected timeline lane controls only the lines, legend and readings that
are drawn. Shared cursor navigation instead uses one sorted exact-deduplicated
union of the timestamps already available to the current screen: every shared
lane and health point, Process observation moments on Processes (including a
loaded selected-process history), and the exact per-database Activity moments
on PostgreSQL Activity. Findings remain directly selectable at their own exact
timestamp but do not become Arrow stops. Pointer selection, global Left/Right
and the shared timeline's keyboard control use this same union; an independent
detail chart keeps its own recorded series as its navigation domain.

The union never rounds, buckets or creates timestamps, and adding a navigation
timestamp never adds a point to a drawn series. At a faster cursor, a slower
metric continues to show its last stored sample at or before the cursor without
interpolation or graph forward-fill. The `at` address and browser history keep
the exact safe-integer microsecond timestamp.
That selected hour establishes the civil date for the whole workspace. A
cursor, snapshot, table interval, detail or chart readout inside the selected
day shows time only; a value outside that day, or either endpoint of a
cross-day comparison, shows the full date. One contextual formatter owns this
presentation, while stored instants, addresses, joins and copied exact values
remain unchanged.

System presents host CPU from `/proc/stat` as user plus nice, system,
interrupts, I/O wait, stolen, and idle shares. Used core equivalents exclude
idle and I/O wait; available host capacity is the recorded online logical CPU
count. CPU history plots these shares together with used and available core
equivalents on labelled scales. In a container, cgroups are separate tables: used,
user, and system core equivalents come from cgroup counter deltas, and capacity
is the smaller of the validated effective quota and the exact effective cpuset
when both are finite. A coherently unlimited quota leaves the cpuset as
capacity. Capacity is `null` when the quota hierarchy is unknown or neither
value supplies a finite bound.

CPU frequency is a temporal CPUFreq-policy gauge in integer hertz. The
collector prefers `cpuinfo_avg_freq`, otherwise selects `cpuinfo_cur_freq`, and
keeps that choice for the lifetime of the policy; a failed read is null and
does not switch source. `scaling_cur_freq` remains a separately named reported
or requested value. Web draws one series per policy and computes an
online-CPU-weighted rollup only when every policy at that timestamp uses the
same actual source. It never copies a policy value into independent logical-CPU
series or graphs static maximum frequency as current frequency. CPU topology
and policy membership are a compact cursor-time reference without history.

Host memory uses non-overlapping anonymous, file-cache-plus-buffer,
reclaimable-slab, unreclaimable-slab, free, and residual categories. The
kernel's available-memory estimate is shown separately because it overlaps
reclaimable memory. Memory history plots the non-overlapping categories, total,
and the separate available estimate together with exact units. Container cgroup
memory separately shows current use, anonymous, file, slab, other kernel, and
residual charged memory; the collector row also receives the finite effective
hierarchical limit. Slab is
subtracted from kernel memory before both are displayed. The leaf's local
memory setting remains available as source data but is not shown as the
effective limit.

System tables contain devices, mounts, interfaces and CPU topology, plus
direct-live cgroups only in a recorded container environment. Block devices are
identified by `major:minor`. Average read and write
latency is `delta(operation_time_ms) / delta(completed_operations)` and is
`null` without a usable predecessor or when the operation delta is zero. Host
I/O PSI stays explicitly host-wide and is not presented as device latency;
cgroup I/O throughput and operations remain separate from host diskstats.
The filesystem table records mount point and root plus exact total/available
byte and inode pairs. It does not derive used space from availability. Storage
topology adds only exact sysfs partition-to-parent edges and leaves dm/LVM/MD,
whole devices, unresolved links, and bind ancestry opaque. Selecting a metric
opens its one-hour history.

A persisted local preference can remove every large chart panel from the layout,
so tables immediately use the released viewport height. Process-summary loads
retain the last successful rows and distinguish loading, request failure, and a
successful empty result. PostgreSQL navigation and visuals are absent when the
selected current data has PostgreSQL disabled; a historical hour that contains
PostgreSQL telemetry remains available, and disabled PostgreSQL leaves overall
health equal to OS health. A selected Linux process links to the nearest
`pg_stat_activity` data by exact PID and shows the PostgreSQL PID, database,
role, application, client, state, wait, query and times. Locale changes are
immediate and persist locally.

Processes uses the whole viewport row left after the timeline, controls and
summary. Its virtual table and adjacent selected-process dock share that row
and scroll independently on wide screens; the narrow dock remains bounded.
CPU history offers temporal counters and gauges, including major page faults,
but keeps scheduler references such as nice, priority and realtime priority as
compact cursor-time facts rather than graph choices.

Every displayed duration uses one adaptive formatter across tables, details,
current readings, axes, hover and statistics. It chooses ns, µs, ms, s, min or
h from magnitude and preserves semantic denominators such as `/s` and `/call`;
stored values, transport, sorting and calculations keep their exact base unit.
PostgreSQL block and buffer counters are converted with the exact recorded
`block_size` setting and displayed as adaptive bytes/s or bytes/call. Without
that setting the converted reading is unavailable. Buffer hits remain buffer
activity and are not relabelled as physical disk I/O.

Shared charts reserve room at the end for the last time label after accounting
for every visible side axis. The compact 128 px timeline keeps its x-axis,
cursor and labels inside the figure before the following navigation. Charts
keep series names with the aligned statistics and place percentile columns without
colliding with the plot edge. Sparse tables are content-sized instead of
reserving large framed boards. A boundary is draggable only when it controls a
real split; otherwise the layout stays light. The exact current `pg_wal` file
size is a subordinate value with optional history, not a primary overview
chart.
The shared Processes, Host and PostgreSQL shells join their timeline and
primary controls directly with a light content-sized boundary. Empty spacer
tracks and strong nonfunctional splitter bands are forbidden; a visible resize
handle exists only for a real bounded accessible split.

PostgreSQL Tables and Indexes use lens-specific details at database, schema,
object and cluster-wide tablespace level. Tablespace identity is its effective
OID; the nullable name is display only. Tables group heap main-fork plus TOAST
storage and never user-index bytes, while Indexes use each index's own
placement. Storage-less partitioned table parents remain visible at the other
levels but do not enter Tablespace groups. The dock is one compact two-column
operator-fact list plus one explicitly selected metric history, not a loose
vertical dump of every physical field. Tablespace history is an exact
cross-database as-of reduction; it is never approximated from one database or
the current page.

Every entity table uses one bounded public search language and one URL-owned
applied expression. Ordinary input without a colon is free text. Structured
input is at most eight `field:value` clauses joined only by case-insensitive
`AND`; string values may be quoted with `\"` and `\\` escapes and may use `*`
and `?`, while decimal identifiers are exact and never globbed. Each surface
owns a compact field registry containing canonical names, aliases, type,
physical projection and help. Aliases canonicalize to the same chip; unknown
or unavailable fields and malformed selectors are errors, never text
fallbacks. `query_id` and `plan_id` are the only public plan/query identifier
spellings, independent of physical extension layout.

The token field keeps an editable draft separate from the last valid applied
expression. Submission is atomic: an invalid span is marked and announced
while the URL, request and last successful rows remain unchanged. A valid
expression becomes removable keyboard controls; manual entry, paste and
related-row links all produce this same state. Progressive RU/EN help lists
only the current surface's fields and rules. The server parses and validates
the same bounded expression, adds only its registered physical projection,
and filters the complete eligible set before semantic ordering and cursor
pagination. Search refusal, a successful empty set and transport failure are
distinct; refresh failure retains the last successful data.

PostgreSQL related-row navigation is confined to the PostgreSQL feature area
and stored as that same public expression in the URL. For PostgreSQL 14–18 an
Activity row, including Activity joined to a selected process, can open every
retained `pg_stat_statements` row at the unchanged cursor whose `dbid` and
nonzero signed `queryid` match the row's nullable `datid` and `query_id`; it
deliberately does not filter by role or top-level status. Statements open all
plans with the available database, role and Query ID, and plans open all
statements by the corresponding identity. A plan row uses the layout's
available shared database, role and query identifiers, while the vadv fork of
`pg_store_plans` uses only its nonzero last-attributed
`queryid_stat_statements`. These actions
show related cumulative rows, select none automatically and make no claim
about one exact execution. Missing or zero IDs are inert; Back restores the
prior view, cursor and expression.

Activity duration presentation is state-aware. `Query time` exists only for
`active`. `Time in state` emits an explicit null for pure `idle`, so a long
idle period cannot draw a line or transition spike; `idle in transaction` and
`idle in transaction (aborted)` retain their operationally relevant duration.
History projections include state and preserve null breaks through transitions.

Within the selected calendar hour, PID alone identifies OS process and
`pg_stat_activity` rows, histories, filters, joins and counter deltas. Process
`starttime` and PostgreSQL `backend_start` remain observed timestamps and do
not participate in that identity. For a process-to-Activity join, retained
rows are filtered by exact PID before the cursor-nearest timestamp is chosen.
Per-database collection timestamps may differ slightly, so a globally nearer
row for another PID must never hide or replace the selected backend.

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
Long ranges use segment-grain `.idx` for fields in the summary allowlist.
Other fields, sub-segment resolution and partial boundary segments use
projected raw samples.

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

Each finished per-section derived index has a stable URL and, because a
finished segment cannot change, uses
`Cache-Control: private, max-age=31536000, immutable` with the `ETag` from the
`.idx` checksum. A browser that does revalidate gets `304 Not Modified` with no
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
summaries for allowlisted fields in complete segments and raw projections for
other fields or where exact boundaries require them, so starting from nothing
stays cheap.

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
