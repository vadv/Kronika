# Investigate a recorded hour

[Русская версия](operator-guide.ru.md) · [Views and controls](features.md) · [README](../README.md)

Kronika lets you return to the machine as it was: which resources were busy,
which processes were present, what PostgreSQL backends were doing, and what
the logs recorded alongside them. Begin with a time and a question, keep that
time while moving between views, then narrow to a process, query or relation.
The result is a set of recorded observations you can explain and share.

For your own machine, first follow the [installation guide](../INSTALL.md).
Run the collector with explicit storage and the access its OS sources need;
open `kronika-web` on localhost over that storage. OS collection is enough to
start. Add PostgreSQL DSNs when that machine's database history is needed.
A preview of someone else's recording cannot tell you what happened on yours.

**Read in order:** [Choose the hour](#establish-the-hour-before-interpreting-the-numbers) →
[Find the resource](#find-where-and-when-work-changed) → [Read hour and cursor](#use-the-hour-and-the-cursor-together) →
[Worked examples](#worked-examples-from-a-real-public-recording) →
[Continue the case](#continue-a-case-into-maintenance-storage-and-logs) → [Share](#preserve-and-communicate-what-you-found)

## Establish the hour before interpreting the numbers

1. Select the day and hour containing the incident. The horizontal scale is
   that whole hour, not an automatically zoomed range of available points.
2. Choose **UTC** when matching UTC logs or the examples below. Browser time
   follows the browser's zone. The hour establishes the date, so most readings
   show only time; a value outside that day includes its date.
3. Select a moment on the timeline. The clock at the top is the cursor, not
   a promise that every source was sampled at that exact microsecond.
4. Read a metric's help and unit. CPU share, CPU time, throughput, a counter
   since statistics reset and an interval mean answer different questions.
5. Keep the time fixed while moving to another view. Follow recorded IDs when
   possible and compare the source snapshot/interval shown in the Inspector.

A faster OS sample and a slower PostgreSQL sample can appear under the same
cursor. Values normally use the latest stored sample at or before it;
process→Activity context additionally identifies the nearest recorded sample
for the exact PID. That is why a shared summary and a selected backend are not
necessarily simultaneous observations. Move with ←/→ to compare actual
recorded instants rather than inventing intermediate values.

**Refresh is about newly recorded data.** A visible current hour refreshes
every 15 seconds after the previous load completes; a hidden page stops and
refreshes when visible again. A pinned cursor stays selected. A cursor that
follows the newest observation advances when new data arrives. The hour does
not automatically roll into the next one. Historical hours do not poll, and
a standalone HTML file is frozen. Opening history only reads more of the
selected recording; it does not refresh or rerun the workload.

## Find where and when work changed

Use the compact timeline to locate a period, then open **Host**. Its USE ledger
separates resource use, time spent waiting for resources and recorded errors.
Open the resource's cell, inspect its composition and history, and then its
individual device, interface or cgroup if one is relevant.

| First observation | Next useful views | Comparison to make |
| --- | --- | --- |
| CPU use or scheduler pressure rises | Host CPU → Processes CPU → selected process | Used cores, CPU PSI/run delay, command and per-process history. |
| Memory use grows or OOM is recorded | Host Memory → Processes Memory → RSS Activity | Available/charged memory, limits, swap and the commands contributing resident memory. |
| Storage queue or I/O wait rises | Host Storage I/O → device → Processes Disk | Device throughput/latency, exact mount and process storage bytes. |
| PostgreSQL concurrency or transaction age rises | Overview → Activity → Locks | Active versus waiting, transaction age versus query duration, holder and waiter identities. |
| A query dominates execution or temporary writes | Statements Activity → row → Plans → Tables/Indexes | Whole-hour contribution, interval work/call, recorded plan, relation activity and size. |
| Maintenance overlaps a busy interval | Vacuum → episode Process panel → Tables Maintenance → Events | Phase/progress, observed process deltas and logged maintenance. |
| Errors or log bursts appear | Events → group → representative occurrence | Exact text/time and resource/backend readings around the same period. |

A coloured value or a marker is an entry point. Field help explains its fixed
boundary; an increase in ordinary work need not be an error. Kronika does not
state that two coincident lines explain one another. Write down the time,
scope and observed change before deciding what further operational action to
take outside the viewer.

## Use the hour and the cursor together

The table tells you what was present at the cursor. Activity above it ranks
what contributed across the entire hour. Start with **Global** colouring to
compare magnitudes; switch to **Per row** to compare timing within each row.
Return to Global before judging which row did more work.

In Processes Activity, click a busy cell to place the cursor at its end, then
a row to filter the owning table. The table resolves recorded samples at or before that time. A command can combine many PIDs, including processes that have
already exited. Select one PID for its own Inspector; use Tree to understand
its parent context. Total includes unshown rows; Other is the remainder of
that total, not an extra workload.

For CPU, an hour summary is accumulated CPU time; cells are CPU time per
wall-clock second. For RSS, the summary is average combined resident memory
across recorded process snapshots; cells retain interval readings. An RSS
average and a CPU total are useful together precisely because they measure
different things. Keep each unit attached to its reading.

Search narrows the objects, a lens narrows the columns, and Chart opens history.
For example, on Processes apply `command:postgres*`, switch between CPU and
Disk, select one row and use its history metric selector. Clear search to
recover the whole table. Read a pending/failed-request status before treating
retained rows as the new result.

## Worked examples from a real public recording

The examples below use the same frozen **5 September 2026, 19:00–20:00 UTC**
recording of the demo's actual Linux/PostgreSQL workload. They are observations
of a generated workload, not measurements of a production incident. Screenshots
come from that recording; displayed numbers are rounded by the interface.
Open the links, choose UTC, and repeat the steps. The recording does not change
when the browser's date changes.

### 1. A busy container does not mean the whole host is equally busy

[Open Host at 19:00:33 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788634833931637&view=host).
The container CPU row reads **66.8% of its limit**, while Host CPU reads
**17.5% busy**. Container memory is **53.8% of its limit**; Host memory is
**12.9% in use**. These percentages have different denominators.

![Separate Container, Network namespace and Host scopes](images/host-scopes.png)

1. Read the scope headings: Container, Network namespace and Host.
2. Open Container CPU. At this cursor, its **Throttled 34.9%** is alongside
   **CPU PSI 4.3%**. The first describes quota throttling; the second describes
   resource wait. Neither is the host CPU utilization.
3. Open Host CPU and compare its own history/capacity. Do not replace a missing
   cgroup limit with the host's online CPU count.
4. Open Container I/O or the Network namespace row for their own measurements.
   The recorded namespace shows **284 KiB/s RX and 284 KiB/s TX** here. Those
   two directions do not establish twice that amount of external traffic.

This example separates three questions: how much of the container allocation
was used, how much time work waited, and how busy the underlying host was.

### 2. Find the command's hour contribution, then the individual process

[Open Processes CPU at 19:03:39 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788635019201666&lens=cpu).
Open **Processes Activity → CPU time**.

![Processes CPU table beneath its whole-hour command Activity](images/processes.png)

The `postgres` group contains **517 PIDs** over the hour and accumulates
**9.44 min** of CPU time. `kronika-demo` accumulates **4.75 min**; the Total is
**14.6 min**. At the selected cell the `postgres` group reads **953 ms/s** and
`kronika-demo` **79.8 ms/s**. Those are cell rates, not the hour totals divided
by the lifespan of one process.

1. Keep Global selected to compare group magnitudes.
2. Click `postgres` to filter to that command's current processes; inspect a
   PID, then its Activity context if present. The whole-hour group count does
   not mean 517 concurrent backends.
3. Clear the filter and select PID **64**, `/usr/local/bin/kronika-demo`. In
   the shown process snapshot its user CPU is **0.12 cores**, system CPU
   **0.006 cores**. These point-interval values need not equal its minute cell.
4. Change the Activity metric to RSS. Read the **Average** heading and compare
   the command's hourly resident-memory contribution, then select Memory for
   the exact PID's cursor RSS and history.
5. Use Disk read/write bytes to distinguish this workload's storage activity
   from logical I/O served by memory.

The command ranking finds work that would be missed by looking only at the
processes alive at the end of the hour; the table and Inspector retain the
individual process context.

### 3. A normalized query leads to its recorded text plan

[Open Statements at 19:00:33 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788634833931637&view=pg.statements).
Open Activity with **Execution time**, then select the customer-order lookup:

```sql
select id, status, total_cents from shop.orders
where customer_id = $1 order by placed_at desc limit $2
```

![Statements: hourly contribution, interval rates and selected SQL](images/statements.png)

Its Query ID is **`-665077864269413128`**. The Activity hour contribution is
**16.7 min** of execution time. The table's calculation interval is
**19:00:28 → 19:00:33**, with **120 calls/s**, **1.42 s/s** execution time and
**11.9 ms/call**. Accumulated execution time can exceed a wall-clock second
when several calls overlap; it is not CPU utilization.

1. Switch to Per call to distinguish frequent inexpensive calls from costly
   individual calls. Read I/O or Resources for shared-buffer, temp and WAL
   work without changing the cursor.
2. Use **Open plans** or the Query ID link. The visible target search selects
   the same query's recorded plans. Choose Plan ID **`1544266440`**.
3. Read the Inspector's stored text. This plan includes `Parallel Seq Scan on
   orders`, a `Sort` on `placed_at DESC`, `Gather Merge` and `Limit`; it records
   `Workers Planned: 1` and filter `(customer_id = 4244)`.
4. Follow Related statements back, or use browser Back. The parameterized SQL
   and the concrete filter in the stored plan belong to their recorded forms;
   the viewer does not rerun EXPLAIN.
5. Open Tables and find `schema:shop AND table_name:orders`. Inspect Access,
   Size and buffers, and the linked Indexes; compare a recorded later time
   when exploring changes. A visible sequential scan by itself does not
   establish the reason for the measured execution time.

![Plan Inspector with actual stored plan text and related query](images/plans.png)

**Known limitation of this hour:** Plans Activity returns an error because
`pg_store_plans` recorded duplicate identities. The table and selected-plan
Inspector used above work. Do not interpret that error as zero activity,
This walkthrough makes no claim about
an hourly plan ranking that could not be read.

### 4. A waiting query and an idle transaction can be in the same chain

[Open Locks at 19:00:33 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788634833931637&view=pg.locks).
The recorded chain has root PID **3765**, `idle in transaction`, and waiting
PIDs **4761** and **4762**, `active` with wait type **Lock**, event
**transactionid**, mode **ShareLock**, target **transaction 4700**. Both wait
starts read **19:00:19**. The recorded application is `checkout-api`.

![Root and waiting PIDs in the recorded lock chain](images/locks.png)

1. Select a waiting row and read its blocker list and full query context.
2. Inspect the root. `idle in transaction` differs from a closed transaction:
   the row is still part of the recorded blocker chain.
3. Use Activity at this period to compare query duration with transaction
   duration and state; use the process context for Linux resource use.
4. Open Events and find lock-wait groups around this period. Open a
   representative occurrence for the recorded log text and time. A log holder
   PID does not itself contain the holder's SQL.
5. Step through later recorded instants to see how the selected rows change.
   Compare the actual source samples, not only the shared timeline's summary;
   the sources were not all collected simultaneously.

The chain establishes the recorded holder/waiter relationship. Its SQL and
application context give the next concrete objects to investigate.

## Continue a case into maintenance, storage and logs

When a query touches a large or busy relation, Tables offers more than size.
Use Access for scan activity, Changes for modifications/dead-row estimates,
Maintenance for vacuum/analyze counts and times, and Freeze for transaction
ages. Group by database or schema to compare aggregate activity, then drill to
the table. Follow its Indexes link to read usage, size/buffers, state and the
recorded definition. A low scan count over one hour is an observation about
that recording, not a lifetime usage history.

Vacuum records episodes over the hour, including completed ones. Select an
episode, compare phase runs and scanned-heap progress, then open its Process
panel. Its OS deltas use the matched PID's samples at or before the episode
endpoints. A manual VACUUM backend may have done other work during that span.
A coloured phase is fixed by its PostgreSQL meaning, while a flat progress
counter states only that the recorded counter did not move. Events can supply
logged autovacuum/autoanalyze details alongside those measurements.

For storage, follow the exact device's `major:minor`, mounts and recorded
layered-device links. Cgroup and physical-device counters can describe the same
I/O at different layers: read the Inspector's chain before adding anything.
For WAL, separate bytes generated, archiver outcomes and the current `pg_wal`
directory size. For network, keep namespace and direction attached to the
number. These distinctions prevent a visually adjacent number from becoming
an unrelated denominator or duplicated total.

In Events, expanding a group reads its count/structure; opening its
representative fetches the complete stored occurrence. Search by source,
category or text, inspect first/last times and click the minute strip. A
selected timeline cluster narrows the console to that interval and its
sources; clear it to recover the hour. PgBouncer is available as grouped logs
with exact messages and connection context. It has no shared-timeline marks,
so its absence from that strip does not imply an empty PgBouncer console.

## Preserve and communicate what you found

A useful handoff contains the selected hour/time zone, view and object ID,
the measured value with its unit and interval, the relevant stored text and a
link. Say "these rose together at these recorded times" when that is what the
recording establishes. Keep a layout-absent field or failed read explicit.

Copy the browser URL for another operator with access to the same web server.
For a self-contained handoff, use **Export** on your own web instance, review
the selected range and download the interactive HTML. It contains all recorded
sections in that range, including stored SQL, logs and command lines. The
recipient can reopen views and selections without a collector or web server.
The HTML remains frozen and has no live MCP endpoint. For scripted work, use [dump](../bins/kronika-dump/README.md) to slice
a recording and [report](../bins/kronika-report/README.md) to create HTML.

To involve an external AI client, use **Connect an AI agent** and the
[MCP client guide](mcp-clients.md). Ask it to list recorded bounds/sections,
rank a concrete metric in an explicit interval, find a named entity at a
recorded time and retrieve the selected row's detail. The tools read the same
recordings; they do not query current production state or execute corrective
commands. The [feature reference](features.md) lists every shipped tool and
all available views, lenses and controls.
