# Views and controls

[Русская версия](features.ru.md) · [Operator guide](operator-guide.md) · [README](../README.md)

This reference describes the interface in this source revision. A view or field appears when its
source and PostgreSQL layout were recorded. An absent measurement is `—`; a
recorded zero is `0`. Kronika reads recordings: opening a chart, finding a row or
asking an MCP tool does not run a query on the monitored database.

**Jump to:** [Time and controls](#time-navigation-and-the-inspector) ·
[Search](#find-a-row-change-a-lens-read-history) · [Heatmaps](#activity-heatmaps-who-contributed-over-the-hour) ·
[Processes](#processes-which-linux-task-was-doing-the-work) · [Host](#host-which-resource-and-which-scope) ·
[PostgreSQL](#postgresql-from-instance-to-query-and-relation) · [Events](#events-read-the-logs-beside-the-metrics) ·
[MCP](#mcp-and-the-ai-connection-panel) · [Export](#export-and-offline-utilities)

## Time, navigation and the Inspector

| Control | What it does |
| --- | --- |
| Day/hour picker and previous/next hour | Select one recorded calendar hour. The full hour remains the horizontal range even when only part of it contains samples. |
| Browser time / UTC | Changes displayed civil times and the hour picker. Stored instants do not change. Use UTC when reproducing a link or comparing a log from another machine. |
| Workspace clock | The selected cursor time. A metric reads its last stored value at or before that cursor; sources can have different sampling intervals. |
| Compact timeline | Choose a lane, preview a recorded instant with the pointer, then release to commit the cursor. Leaving without committing restores the selected time. |
| Previous/next recorded instant, ← / → | Move among recorded times available to the current view, independent of which lane is drawn. These are sample steps, not one-second increments. Form controls keep their own arrow keys. |
| Refresh | Request newer recordings and refresh the selected view. The visible current hour also refreshes every 15 seconds; hidden pages and historical hours do not poll. An old hour stays selected until another hour is chosen. |
| Chart | Open the timeline metric in the Inspector. Select a row to inspect that entity; its own Chart shows its recorded history. |
| Inspector Detail / Chart / related tabs | Switch between the selected row's facts, history and available related objects. The visible tabs depend on the row. |
| Expand chart / restore | Give the Inspector chart more space. A metric selector chooses one history measure; multi-series charts have labelled series and an All choice. |
| Inspector divider | Resize on desktop. With the divider focused, arrows resize it and Home/End select its width limits. |
| `?`, field help and Esc | `?` opens the help panel; a field's help mark opens its definition. Esc closes an open panel or selection. |
| Sign out | End the browser session on a live web instance. The next visit requires sign-in again. |
| Language and theme | Switch EN/RU and light/dark immediately; preferences are saved in this browser. |
| Copy exact value | Copy the underlying value rather than its rounded display. If automatic copying is unavailable, the exact text is selected for manual copying. |

A chart's legend identifies the series; hover displays a reading without
replacing the committed selection. Last, Min, Max, P50, P90 and P99 describe the
chart's recorded values. They are not request-latency percentiles unless the
recorded metric itself has that meaning. A duration keeps its denominator:
`ms/call` is different from `ms/s`. No point is invented between measurements.
A blank segment of a chart is not a zero line.

URLs retain the cursor, view, lens, sort, search and supported entity selection.
Browser Back/Forward restores navigation. Ordinary navigation to another search
surface clears its filter; a related-object link explicitly sets the target's
search. Not every display preference is in the URL: language, theme, activity
open state and Inspector width are browser preferences.

On phones, Chart and Search buttons open the same controls in panels; the lane
selector becomes a native select. The Inspector is a bottom sheet on phone and
an overlay on tablet, with its own scrolling. Wide tables remain scrollable.
This is a smaller workspace for the same recordings, not a separate mobile
metric set.

### Health, load and marks

The Health lane shows Overall, OS and PostgreSQL separately when available.
OS Health subtracts the largest observed CPU/memory/I/O stall-time share from
100; it uses adjacent PSI `some_total` counters in the recorded scope, not
CPU utilization or the kernel's rolling `avg10`. An OOM event or throttled-time
counter remains a separate reading. Without usable pressure inputs, Health is
unavailable; a cgroup v1 recording cannot borrow host PSI for its container.

PostgreSQL Health compares active backends, including those waiting, with
`2 × KRONIKA_POSTGRES_EFFECTIVE_CPUS`. It is 100 at or below that capacity;
above it, the penalty is `round(100 × (active − capacity) / active)`. The
configured capacity belongs to the monitored database, which can be remote;
`max_connections` and the collector host's CPU count are not substitutes.
Overall subtracts both component penalties and clamps to 0–100. Disabled
PostgreSQL contributes no penalty; enabled PostgreSQL without a usable
component leaves Overall unavailable. Health 100 describes these inputs,
not every possible failure on the machine.

Timeline **circles** mark log events, **diamonds** fixed threshold crossings,
and **triangles** sharp rises. The selected metric's help names its exact
boundary; these marks are separate from the Health calculation and from the
colours of table values. [The complete boundary list](../DESIGN.md#known-bad-boundaries)
includes CPU/memory/filesystem shares, load per online CPU, blocked sessions,
long log statements, transaction ages and increases in explicit error counters.

## Find a row, change a lens, read history

A **lens** chooses columns for a question. It does not recollect metrics or
change the stored row. Click a sortable column header to change order. Tree and
lock-chain order preserve their hierarchy; not every column is sortable. Large
paged tables expose a load-more control and a retry after a failed request.
Loading or a failed search can retain the previous rows with an explicit
status; that status matters before treating those rows as the new answer.

Search supports plain text, `field:value`, `field>quantity`, `field<quantity`,
uppercase `AND`/`OR` and parentheses. Text fields accept `*` and `?` where their
help says so. Use quotes around values with spaces. Apply or Enter commits the
whole expression; removable chips show the applied terms. An invalid draft
leaves the last valid result and URL intact.

Examples on their respective surfaces:

```text
# Processes
command:postgres* AND rss>100MiB
pid:1234

# Activity
state:active AND wait_type:Lock

# Statements / Plans
query_id:123456789

# Tables / Indexes
schema:public AND size>100MB
```

The numbers above are syntax examples, not IDs from the public recording. Open
Search help for the exact fields and copyable examples for the current table.
Comparisons are strict `>` or `<`, applied before sorting and paging; `MB` is
decimal and `MiB` binary, rates require `/s`, and units are case-sensitive.
Unavailable quantities do not become zero. `NOT`, `>=` and `<=` are not
supported. In grouped Tables/Indexes, names select members and quantities test
the resulting group: connect these stages with `AND`; one `OR` cannot mix them.

Selecting a row opens its Inspector. Detail preserves identifiers, recorded
text and units; Chart selects among temporal fields. Reference facts such as
scheduler priority or topology do not acquire a history merely because they
are numeric. Related buttons follow the recorded identifier, keeping the hour
and cursor. They select a matching set, not an arbitrary first matching row.

## Activity heatmaps: who contributed over the hour?

Open the collapsed Activity heading above a supported table. The selected
metric ranks the entire hour, including entities no longer present at the
cursor. Opening the ledger starts its data request; it is not preloaded while
collapsed. The compact view shows up to eight ranked rows, folding the rest
into Other. Full screen offers Top 10/25/50/100, with 25 as the initial limit.

| Reading/control | Meaning |
| --- | --- |
| Time cells | Counter rate over the cell's recorded intervals; gauges use interval readings. Blank is unavailable; the lightest fill is a real zero. |
| Hour summary | Accumulated counter change; a gauge maximum except for process RSS, whose column is **Average**. |
| At cursor | The cell value at the current cursor. It can differ from a table's point-in-time rate. |
| Total | Includes all entities, including those outside the displayed ranking. Its muted band always uses its own maximum, independently of Global/Per row. |
| Other | Total minus the displayed ranked entities. It is not an extra process or statement. |
| Global | One intensity scale across ranked and Other rows. This allows magnitude comparison. |
| Per row | Each row uses its own maximum. This reveals timing within a quiet row; equal colour across rows does not mean equal work. |
| Click a cell | Move the cursor to the final microsecond of that cell. The table then resolves its stored samples at or before that time. |
| Click a row | Filter the owning table to that entity or group. If it has no reading at the cursor, also move to its busiest recorded interval. |

Process rows group all recorded PIDs with the same command, including exited
processes. CPU time adds user and system time: one CPU-second per wall-clock
second means one used core. The hour total is CPU time, not an average percent.
RSS adds each group's resident memory at each recorded process snapshot and
averages across those snapshots. A process absent at a recorded timestamp
contributes nothing. This is not its peak or its mean while alive. Shared pages
can be counted in several processes' RSS; the total is not unique physical RAM.

Most ledgers show 60 cells per hour; Tables and Indexes show 12 because their
collection cadence is five minutes. Cell count does not change whole-hour
ranking. PostgreSQL byte measures use the recorded block size; process CPU time
uses the recorded clock tick rate. Where an Activity cut lacks that metadata,
its label and unit remain raw counts rather than invented bytes or seconds.

| Activity ledger | Available measures |
| --- | --- |
| Processes | CPU time, RSS, Disk read bytes, Disk write bytes, Major page faults, Run delay |
| Statements | Execution time, Calls, Rows, Shared buffer read bytes, Shared buffer dirtied bytes, Temp buffer written bytes, WAL bytes |
| Plans | Execution time, Calls, Rows, Shared buffer read bytes, Temp buffer written bytes |
| Databases | Commits, Rollbacks, Read bytes, Temp bytes, Deadlocks |
| Tables | Rows changed, Rows read by seq scans, Heap buffer read bytes, Dead tuples, Autovacuum time |
| Indexes | Index scans, Index tuples read, Index buffer read bytes |
| Cgroup CPU | CPU time, Throttled time |
| Cgroup I/O | Read bytes, Write bytes, Read operations, Write operations |

Tables/Indexes Activity follows the current object/schema/database/tablespace
grouping. **The public 5 September recording has a known Plans Activity error
from duplicate recorded plan identities.** Its Plans table and selected-plan
Inspector still work. The error is a limitation of this example, not a normal
empty Activity view or a reason to change its filters.

## Processes: which Linux task was doing the work?

| Lens | Readings and use |
| --- | --- |
| Tree | Parent/child structure, user, PID, CPU and memory percentages, VSZ, RSS, terminal, state, start time, accumulated CPU time and full command line. Search retains ancestors for context. |
| CPU | User/system core equivalents, scheduler run delay, block-I/O delay, voluntary/involuntary context switches, current CPU, nice/priority/realtime priority and scheduling policy. |
| Memory | RSS, virtual memory, swap, minor and major page faults. |
| Disk | Storage read/write bytes, read/write syscall counts, logical read/write bytes, cancelled writes and block-I/O delay. Page-cache reads can appear in logical reads while storage reads remain zero. |
| General | Parent PID, real/effective user, group IDs, thread count, terminal, exit signal and state. |

The compact totals beside the lenses describe the complete process snapshot,
not just visible or searched rows. Tree/General show processes, threads,
runnable processes and PostgreSQL backends; the other lenses show their
resource totals. CPU rates require the same recorded PID and process start
identity across samples. No predecessor or unavailable per-process I/O yields
`—`.

A selected process can expose PostgreSQL Activity by exact PID. The related
panel identifies its actual Activity sample and shows database, role,
application, client, state, wait, query and timing. That sample is the nearest
recorded Activity row for this PID; it need not equal the process cursor time.
Names come from the collector's recorded local user mapping. An unresolved user
remains a numeric UID.

## Host: which resource and which scope?

The **USE** ledger organizes utilization, saturation and errors. A populated
cell is a button: it opens that resource and selects that metric. Several
resource rows can stay open. Its heading summarizes the rows: largest
comparable utilization share at the cursor, resources with nonzero pressure
during the hour, and recorded error/event counts. It does not add unrelated
pressure units or turn load into a prescribed action.

### CPU and memory

CPU expands to user+nice, system, interrupts, I/O wait, stolen and idle shares;
used core equivalents exclude idle and I/O wait. Capacity is recorded online
logical CPUs. It also exposes load averages, runnable/blocked tasks, context
switches and CPU PSI. PSI describes time that work waited for a resource;
utilization describes work being done, so a busy CPU and a stalled workload
are different readings.

CPU Topology lists logical CPU, socket/core and NUMA IDs, model and maximum
frequency, alongside recorded CPUFreq policies.
Actual frequency is policy-scoped; scaling frequency is a separate reported or
requested operating point. The rollup uses online-CPU weighting only with
compatible recorded actual-frequency sources. Static maximum frequency is not
a measurement of the current clock.

Memory separates anonymous pages, file cache plus buffers, reclaimable slab,
unreclaimable slab, free and residual memory. Available is the kernel's
separate estimate and overlaps reclaimable memory: do not add it to those
components. Total, swap, swap activity, memory PSI and OOM kills complete the
resource view. The composition chart and each selected measure retain their
own units and history.

### Storage and network

| Storage mode | What to inspect |
| --- | --- |
| I/O | Device read/write throughput and operation rates, busy time, average queue, read/write latency and in-flight work. Average latency is the operation-time delta divided by completed-operation delta; no completed operations gives `—`. Host I/O PSI is host-wide, not device latency. |
| Filesystems | Exact mount point/root, source, type and device identity; total/available bytes and total/available inodes. Available space is not synonymous with free space usable by a privileged process. |
| Topology | Recorded partition-to-device and layered-device-to-slave edges. Expand the recorded structure; Kronika does not infer bind-mount ancestry or links absent from sysfs. |

Devices retain `major:minor`. Selecting one opens its exact row and metric
history. Network exposes per-interface RX/TX bytes, packets, errors and drops,
with histories, plus recorded link speed and duplex. Traffic belongs to the collector's network namespace; a
loopback byte can appear in both RX and TX.

### When the collector is in a container

Recorded environment controls the scope; the existence of a systemd cgroup on
a machine does not turn it into a container. Machine recordings have no
Cgroups view. Container recordings show **Container**, **Network namespace** and
**Host** separately.

Container CPU/Memory/I/O/Threads rows describe the collector's own cgroup.
CPU uses its effective quota/cpuset capacity when recorded; otherwise it shows
used cores without a fabricated percentage. Memory uses the effective limit
when known, or plain bytes. CPU throttling, controller PSI and OOM kills stay
in their own scope. Threads is `pids.current`, a count of TIDs including main
threads; local `pids.max` is a separate setting, not a guaranteed effective
hierarchical limit.

CPU and I/O disclose Activity ledgers; each container resource opens the
recorded direct-live cgroup table. CPU, Memory, I/O and Threads controls change
its mode. A selected row opens its Inspector. Cgroup memory separates anonymous,
file, slab, other kernel and residual charged memory; the limit is not host RAM.

The I/O inventory folds only layers connected by exact recorded block-topology
edges within the same cgroup. Its Inspector shows each charged layer's
counters as the same I/O, never their sum. Mount point, source, device name,
physical device and `major:minor` keep the row identifiable. The same device in
another cgroup stays another row. Container diskstats do not inventory every
unrelated disk on the underlying node.

## PostgreSQL: from instance to query and relation

PostgreSQL navigation follows the selected recording. An OS-only installation
has no empty database dashboard; an older hour containing PostgreSQL remains
available even if collection is now disabled. Extension- and version-specific
fields appear only where recorded. None of the controls below changes a
PostgreSQL setting, cancels a backend or runs VACUUM/EXPLAIN.

### Overview and Databases

Overview is the instance-wide ledger. Each row combines an hour value,
sparkline, cursor reading and expandable chart; recorded limits are dashed
rules. It covers:

- Connections/concurrency: client backends, active/waiting states, idle in
  transaction, oldest transaction/xmin and prepared transactions.
- Transaction work: commits/rollbacks, tuples read/fetched/written and buffer
  hit share.
- Errors/temporary data: block I/O time, temp bytes, deadlocks, checksum
  failures and abnormal session ends.
- WAL/checkpoints: generated WAL, scheduled/requested checkpoints,
  checkpoint/backend buffer writes, archived segments/failures and full WAL
  buffers. `pg_wal` directory size is a separate current value with history;
  it is not WAL generation rate.
- Vacuum limits: XID/multixact age and workers against recorded limits.
- Buffer pool/I/O: evictions, ring-buffer reuses, relation extends, fsyncs and
  vacuum-context reads when the PostgreSQL version recorded them.

The header shows recorded version, `max_connections`, `shared_buffers`,
`max_wal_size`, `checkpoint_timeout`, `autovacuum` and `track_io_timing`, plus
lifecycle records and setting changes. Open the changes list and select one
to move the cursor. This is a record of settings, not a settings editor.
`track_io_timing` off can leave PostgreSQL I/O-time counters at a real zero.

Databases separates backends, commits/rollbacks, sessions, tuple operations,
buffer reads/hits, I/O times, temp files/bytes, conflicts, deadlocks and frozen
XID age by database. Its Activity ledger answers which database contributed
over the hour; selecting a row provides field histories.

### Activity and Locks

Activity shows PID, database/role, SQL, query and transaction duration,
application, client, state and wait type/event. **System** includes system
backends; **Idle** includes idle sessions. Both are off initially. The Inspector adds backend type,
leader PID, Query ID, backend and state age, and transaction xmin/xid ages.
Idle state duration is not active query execution time. State transitions and
missing measurements do not turn into an uninterrupted running-query line.

Related Statements uses a nonzero Query ID. Process opens the Linux backend
context by PID. An Activity statement is the text at that sample; a Statements
row is cumulative statistics for a normalized query, not the same object.

Locks preserves blocker-chain order, roots, waiting descendants, extra
blockers and prepared-transaction blockers. It shows query/application,
lock target/relation/type/mode, state, wait and wait start. Select a row for its
complete blocker list and context. Searching a chain retains its structure;
a missing holder statement in a log is not filled in from a guess. Snapshot
chains here and historical lock-wait log groups in Events answer different
questions.

### Vacuum

Vacuum lists recorded episodes with database/relation, PID, Autovacuum or
Manual/other, phase, seen/last-seen time, time and samples in phase, heap
progress/size/vacuumed, index cycles and applicable layout-specific fields.
An episode remains visible after its process ends. **At sample** means it was
recorded in the cursor's collection pass; **Last seen** names its last observation.

The progress chart is scanned heap divided by total heap, not percentage of
elapsed runtime. Index passes can repeat. PostgreSQL 17+ adds processed-index
and dead-item memory readings; PostgreSQL 18+ adds cost-delay time. A field
absent from that layout is unavailable. A recorded zero delay can mean timing
was disabled.

Phase emphasis is fixed by phase name: `truncating heap` precedes an
AccessExclusiveLock attempt; heap/index vacuuming and index cleanup are heavy
phases. It is separate from observed resource load. **No movement** describes
three or more consecutive unchanged phase-specific samples, not a claim that
the backend is stuck. The Process panel reports the matched PID's CPU, storage
bytes, block-I/O wait and major-fault deltas over the observed episode span,
using its latest samples at or before the episode endpoints, with comparison
to PostgreSQL's scanned heap. A manual VACUUM backend can have
other work in that span.

### Statements and Plans

![Statements Activity and query Inspector from the recorded hour](images/statements.png)

| Statements lens | Question and fields |
| --- | --- |
| Execution | Which normalized SQL accumulated work? Calls/s, execution time/s, mean time/call and rows/s with database, role and Query ID. |
| Per call | How much work per execution? Interval mean time, rows and buffer bytes/call, with call rate. |
| I/O | Shared-buffer reads/hits/hit share, dirties/writes, local reads and temporary reads/writes. A PostgreSQL buffer miss may be served by the OS page cache. |
| Resources | WAL bytes and bytes/call, temporary writes, planning time/share and execution/call rates. |
| Stability | Recorded mean/min/max/standard deviation and coefficient of variation from the statistics window, alongside call rate. These are not recomputed latency percentiles for the selected hour. |

Kronika queries are hidden by default from Statements rows, Activity ranking
and summary under the same workload scope. The **Kronika queries** checkbox
shows the exact excluded-row count. An explicit search, related context or
opened collector row includes all statements and locks that control so a
requested row cannot disappear. Normalized identities can have several
recorded database/role/top-level rows.

![Recorded text plan in the Plans Inspector](images/plans.png)

| Plans lens | Fields |
| --- | --- |
| Execution | Plan's first text line, database/role, Query ID or Related Query ID, Plan ID, calls/s, execution time/s, mean time/call and rows/s. |
| Timing | Recorded mean/min/max/standard deviation, call rate and first/last call when the extension supplies them. |
| I/O | Shared reads/hits/hit share, buffer bytes/call, shared dirties, local reads and temporary reads. |
| Identifiers | Plan/query IDs, command type, accumulated calls and call rate. |

The plan Inspector displays the stored, readable text, preserving indentation;
it does not rerun EXPLAIN. Query/Plan ID links open matching Statements or
Plans at the same hour/cursor using visible search. The vadv layout presents
its nonzero related statement ID separately; an internal zero is not a usable
Query ID. The Query panel retrieves related recorded SQL. Copy actions retain
the stored text; a recorded truncation stays visible. Different extension
layouts do not create fields that were never recorded.

### Tables and Indexes

Both views have **object, schema, database and tablespace** groupings. Group
rows have metric histories and member counts; drill into their members, then
open an object. Table→Indexes and Index→Table links keep the relation context.
Group percentages are recomputed from their operands, not averaged across
objects. A group size filter applies to the reduced size of the group.

| Tables lens | What is available |
| --- | --- |
| Access | Tuple throughput, sequential scan share, sequential/index scans, tuples per scan and last scan times. |
| Changes | Insert/update/delete activity and shares, HOT/new-page updates, estimated dead share, changes since analyze and inserts since vacuum. |
| Maintenance | Manual/automatic vacuum and analyze counts, mean durations where recorded, last maintenance and TOAST autovacuum times. |
| Size and buffers | Main-fork plus TOAST size, component shares, row/dead-TOAST estimates, heap/index/TOAST buffer-hit shares. This table size is not the sum of every fork and index. |
| Freeze | Frozen XID/multixact ages, inserts since vacuum and last vacuum times. |

| Indexes lens | What is available |
| --- | --- |
| Usage | Scans, tuples read/fetched, entries and fetches per scan, access method, tablespace and last scan time. |
| Low activity | Size, recorded scans and last scan, with no-scan counts for groups. A quiet recorded interval alone is not an instruction to drop an index. |
| Size and buffers | Main-fork size and buffer-hit share; Inspector/history adds the corresponding recorded buffer fields. |
| State | Valid/ready, primary/unique/exclusion properties; group counts summarize these properties. |

An index Inspector includes its recorded definition. Table row estimates keep
their approximate meaning; `Never` is different from an unavailable timestamp.
These views use their five-minute relation snapshots, not a per-query access
trace. A SQL-text reference to a table is a lead to inspect, not an asserted
per-statement accounting link.

## Events: read the logs beside the metrics

Events combines an hour digest, metric marks and a grouped log console.
Critical/notable/routine tiers are fixed by event type and determine order and
initial expansion. Expand a group to read its structure; **Open representative
occurrence** fetches one complete recorded row into the Inspector. Its time
button moves the cursor. A representative is not every member of the group.

| Source | Grouping and useful readings |
| --- | --- |
| PostgreSQL errors | Severity, category and normalized message pattern; count, first/last occurrence and stored message detail. |
| Slow queries | Normalized statement pattern; count, longest and accumulated duration. The representative is the slowest occurrence. The recorded logging threshold explains which statements entered this source. |
| Autovacuum/autoanalyze | Relation; runs, elapsed work and recorded tuple/buffer details. |
| Checkpoints | One group with scheduled/requested counts, buffer writes and sync time. Checkpoint-warning groups retain their recorded interval. |
| Lock waits | Recorded holder PIDs; waiting sessions and longest wait. Matching acquired records join the same PID/target; unmatched completed waits stay separate. |
| Lifecycle | Individual crash, shutdown and ready records. |
| PgBouncer | Level and exact message, with shared database/user/client/file context. Literal `(nodb)` and `(nouser)` remain visible with explanations. |

Minute occurrence strips move the cursor. A timeline cluster selects the exact
interval and represented log sources; clearing it returns to the full hour.
Log rows, threshold crossings and sharp rises are separate counts. A log
occurrence with a threshold mark is still one log row. A limited result keeps
its notice even after local search narrows the returned groups.

Marker shape distinguishes log events, fixed threshold crossings and sharp
rises; field help explains the metric and boundary. Highlighting is a place to
look, not an explanation of why another metric changed. A routine increase in work is not automatically
an error. PgBouncer groups have no shared-timeline marks. Temporary-file log
records are available through Events occurrences/MCP and row detail, but are
not groups or timeline marks in this console.

## MCP and the AI connection panel

**Connect an AI agent** opens the same web server's `/mcp` endpoint and
client-specific setup prompts for Claude Code, Codex and Cursor. Copy uses the
current interface language. A prompt can contain the web sign-in credentials;
handle it like the password. This panel connects an external client; it is not
an embedded chat that sends the hour away automatically. See
[MCP client configuration](mcp-clients.md).

| Tools | Recorded question |
| --- | --- |
| `kronika_get_instance` | Newest recorded host/PostgreSQL facts. |
| `kronika_list_recorded_sections` | Recorded time bounds, available sections, fields, units and sources. |
| `kronika_rank_metrics` | Rank one or more metrics over `[from,to)`. Each field is an independent result; gauge ranking uses maxima, including RSS here. |
| `kronika_find_processes` | Linux processes at one recorded point. |
| `kronika_find_postgresql_activity`, `kronika_find_postgresql_locks`, `kronika_find_postgresql_vacuum` | Backends, locks and vacuum at a recorded point. |
| `kronika_find_postgresql_databases`, `kronika_find_postgresql_statements`, `kronika_find_postgresql_plans` | Database/query/plan metrics at a recorded point. |
| `kronika_find_postgresql_tables`, `kronika_find_postgresql_indexes` | Relations and supported groupings at a recorded point. |
| `kronika_find_events` | Groups or occurrences in `[from,to)`. |
| `kronika_get_row_detail` | Complete stored row from an opaque `detail_ref` returned by another tool. |

Use recorded bounds when asking about an old hour: `now` means request time,
not the last recorded sample. Copy `detail_ref` unchanged. Rankings and finders
return compact results; row detail supplies stored SQL, plans or log text.
The tools return readings, not live host commands, database operations or
instructions for changing a production system.

## Export and offline utilities

The top-bar **Export** produces an interactive HTML file from your own
recordings. This hour and ±5/15/30-minute presets use the current context;
start/end dates and times, hour shifts and duration remain visible before
export. Browser/UTC applies to input; repeated local times expose first/second
occurrence choices. The file contains all recorded sections in the range,
not just the visible lens or filter.

Preparation shows elapsed time; download shows received bytes. The active
preparation cannot be cancelled or restarted from the dialog. After saving,
the result shows filename, size and elapsed time. The file carries the UI,
WASM query engine and data and opens without a server. The WASM runtime runs on
the browser's main thread. Its frozen range cannot refresh itself or provide
a live MCP endpoint. Stored query text, command lines and logs travel with the
file; share the resulting recording with its intended recipients.

For command-line inspection, `kronika-dump` lists and reads segment contents and
can make a bounded ZMS slice; `kronika-report` wraps a recording as standalone
HTML. Packaging and command examples are in [the release guide](releases.md).
`kronika-demo` is an optional workload/example tool, not the collector required
to record your own host.

The metric catalogs provide field-level storage references:
[Linux](type-registry/os.md), [PostgreSQL](type-registry/postgresql-metrics.md)
and [PgBouncer](type-registry/pgbouncer.md). The
[design](../DESIGN.md) describes recording and calculation contracts.
