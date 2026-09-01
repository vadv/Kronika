# kronika-collector

[Русская версия](README.ru.md)

`kronika-collector` reads operating-system and PostgreSQL metrics, follows
PostgreSQL and PgBouncer logs, and writes Kronika segments. It has no public
command-line interface; environment variables provide its configuration.

## Configuration

Every variable below is read and parsed once, before the first collection. A
value that does not parse stops the daemon with a message naming the variable
and the invalid value. The daemon does not substitute a default.

There is no per-source row cap. Ordinary snapshots retain all rows for one
source. Large PostgreSQL results are streamed in bounded batches without
dropping rows. Each `segment_write_finish` log record includes `rss_kib`, the
process's peak resident set size.

`KRONIKA_OUT_DIR` is the only required variable.

### Storage

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_OUT_DIR` | — | Data root: the journal, the finished segments, and the writer lock. |
| `KRONIKA_SEGMENT_MAX_BYTES` | 64 MiB | Write the open segment once the journal holds this many raw bytes. |
| `KRONIKA_SEGMENT_MAX_AGE_S` | 900 | Write an open segment at this age even if the byte cap was not reached. |
| `KRONIKA_JOURNAL_MAX_BYTES` | 1 GiB | Hard cap of `active.wal`. Reaching it writes the open segment early rather than failing the append. |
| `KRONIKA_RETENTION` | 2 GiB | Local rotation target: a byte budget, `auto` (= `auto:80`), or `auto:P` for a used-space target on the backing filesystem. |

`KRONIKA_RETENTION` accepts an unsigned decimal byte count or `auto[:P]`. A
fixed budget must be at least twice `KRONIKA_SEGMENT_MAX_BYTES`; for example,
`10737418240` sets a 10 GiB budget. Fixed mode counts `active.wal`, finished
`.zms` files and their `.idx` sidecars, and recognized temporary files.
`auto:P` instead compares all used bytes on the backing filesystem with `P`
percent of its capacity, where `P` is from 1 through 99. The percentage applies
to the entire filesystem, not just Kronika data.

Rotation checks the target after a collection cycle that published one or more
finished ZMS files and on a one-minute timer; a running collection cycle can
delay the timer check. Fixed mode also recounts the files once per hour so
`.idx` files created by web enter the total. Rotation deletes stale writer ZMS
temporaries, orphan `.idx` files, then finished `.zms` files and their sibling
`.idx` files from oldest to newest. It never deletes `active.wal`, the newest
finished segment, or unrelated files. If those remaining files still exceed
the target, collection continues and logs `rotation_degraded`.

On the current M1 workload with 452 tables and 3,502 indexes, 43 finished ZMS
files totaled 82,687,221 bytes from 00:10 to 10:40 UTC on 2026-09-01. At the
current 15-minute segment cadence this is about 1.92 MB per segment, four
segments per hour, and 184 MB per day (about 0.18 GB). The default 2 GiB budget
corresponds to about 11.6 days of ZMS at this rate before accounting for
`active.wal` and `.idx`. This is one workload measurement, not a fixed storage
rate. Use the measured local rate when choosing a budget.

### Collection intervals

The scheduler tracks each source interval independently. The timer sleeps for
at most `KRONIKA_INTERVAL_S` and wakes earlier when a source becomes due. A
source interval of `0` reads on every timer cycle.

| Variable | Default, s | Sections |
| --- | ---: | --- |
| `KRONIKA_INTERVAL_S` | 5 | Maximum timer sleep; `0` disables the timer and leaves collection to signals. |
| `KRONIKA_OS_CORE_INTERVAL_S` | 10 | `1_102`–`1_111`, `1_114`–`1_120`, `1_122`. |
| `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 | `1_112`, `1_113`, `1_121`, `1_123`. |
| `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 | `1_100`. |
| `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 | `1_101`. |
| `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 | `1_201`–`1_204`; direct live process memberships, aligned with cgroup mapping. |
| `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | `1_200`. |
| `KRONIKA_LOG_INTERVAL_S` | 10 | `2_001`–`2_007`, `2_100`. |
| `KRONIKA_PG_INTERVAL_S` | 30 | `1_001`–`1_012`, `1_015`–`1_017`, `1_019`. |
| `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 | `1_013`, `1_014`; database and extension discovery. |

### Which server to ask for metrics

The first entry of `KRONIKA_PG_DSNS` is also where the `PostgreSQL` metric
sections come from. A metric row carries no column naming the server that
produced it, so a second DSN is followed for its log only, and starting with
more than one is one line in the log saying which was chosen.

Per-table and per-index statistics exist only inside the database that produced
them. The collector keeps one connection to each connectable database and
reuses it while it remains healthy for at most one hour. The five-minute
discovery pass adds a connection when a database appears and removes it when
the database disappears. A healthy connection is closed and reopened when it
reaches one hour, bounding collector connection resources and PostgreSQL
backend memory and session resources. A connection, protocol, or client query
deadline failure also makes the next collection reconnect.

Extensions are database-local even when their statistics are instance-wide.
The same discovery pass runs one compact inventory query in every database and
caches the database, schema, and usable interfaces of each installation. The
collector selects one usable installation of each extension, so shared rows are
not duplicated. Creating, dropping, or moving an extension changes the selected
installation on the next pass.

`pg_stat_statements` is collected from extension version 1.5 onward, with one
layout per column set. `pg_store_plans` is identified by its callable interface
and result columns rather than `extversion`: the collector keeps separate OSSC
and Datasentinel layouts for their zero-argument readers, and recognizes the
vadv boolean reader only with its four-key plan getter and executable native
text converter. The collector composes those functions in the discovered
schema so every stored `plan` is bounded human-readable text. It also discovers
the exact readable `pg_stat_statements_info` and `pg_store_plans_info` views. Each info
view is selected independently of its main statistics reader. The complete
layout map is in
[PostgreSQL metric types](../../docs/type-registry/postgresql-metrics.md).
When several databases expose different layouts, the collector chooses the
newest `pg_stat_statements` layout. `pg_store_plans` implementations are not
ranked: the current database wins when usable, otherwise database name is the
deterministic tie-break.

`pg_settings` is read on each PostgreSQL collection. The collector writes a
full snapshot after the first successful read, when a setting changes, and in
every new segment. It reuses the latest successful snapshot when OS metrics or
log rows open a segment between PostgreSQL collections.
The `primary_conninfo` and `ssl_passphrase_command` rows are omitted because
their values may contain secrets. Other command settings, including
`archive_command` and `restore_command`, and custom settings remain available.

### PostgreSQL query execution

Small administrative reads use PostgreSQL's Simple Query Protocol. Typed metric
reads use one-shot unnamed Extended Protocol queries. The collector sends one
query at a time on each connection: it creates no named prepared statements and
does not pipeline requests.
All persistent metric sessions from one collector process share one unique
`application_name`; activity and lock reads omit that exact name.

Before exposing each new PostgreSQL frontend session, the collector sends
exactly one Simple Protocol `SET statement_timeout = '30s'` and waits for its
completion. This server deadline is the primary query limit. The client-side
Rust fetch deadline is 35 seconds and remains a backstop for network or protocol
stalls; after it elapses, the collector makes one bounded CancelRequest attempt
and closes the connection.

PostgreSQL metric DSNs support direct PostgreSQL and PgBouncer session pooling.
PgBouncer transaction and statement pooling are unsupported for metric
collection because they do not preserve `SET`/`RESET` session state between
queries. The collector does not inject startup `options` and does not add a
per-query transaction to emulate session state. `KRONIKA_PGBOUNCER_DSNS` uses
the separate PgBouncer admin console path and does not receive the PostgreSQL
session `SET`.

Potentially large results are consumed in batches of at most 256 rows, targeting
approximately 512 KiB of decoded application data. The byte target may be
exceeded by the final SQL-bounded row. Each batch is encoded and appended to the
WAL before the next row is fetched. There is no top-N plan selection or shared
plan-text budget. Each statement or plan text is limited in SQL to 65,536
characters before it crosses the connection.

If a stream fails after earlier batches reached the WAL, those batches remain.
The collector logs the error, skips the rest of that read, and continues with
independent sources. PostgreSQL SQLSTATE `57014` from `statement_timeout` is
counted as a query timeout; after PostgreSQL returns `ReadyForQuery`, that
session remains reusable. A client-side backstop timeout closes the connection,
and the bounded CancelRequest attempt has a one-second deadline.

Every monitoring SQL query produces a `pg_query_finish` event at debug level
with its timings and counters. Fetches longer than 500 ms also produce a
`pg_query_slow` warning. About every five minutes, and once at shutdown,
`pg_query_summary` reports the query count and rate, rows, estimated logical
application bytes read and written, errors, timeouts, slow queries, fetch,
encoding and WAL append time, encoded and appended bytes, and `peak_rss_kib`.
Connection labels use `user@host:port`; raw DSNs are never logged.

### Which logs to follow

A source is named one of two ways. A DSN, and the server itself says which
file it writes, in which format, and who it is. A path or glob, and the file is
read for what it holds while nothing is known about the writer. Both may be
given at once; a file reached both ways is followed once, with what the server
said attached to it.

Every variable holds a `;`-separated list, so a host running several clusters
or several poolers names them all in one place. Nothing is followed unless one
of the four is set.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_PG_DSNS` | unset | PostgreSQL metric and log-discovery connections. Every entry supplies log location and format; only the first supplies metric rows. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | unset | Positive effective CPU capacity of the first PostgreSQL metric target. Required for numeric PostgreSQL health; never inferred from the collector host. |
| `KRONIKA_PG_LOGS` | unset | `PostgreSQL` logs named outright. An entry with `*` or `?` in its last component is a pattern matched against that directory. |
| `KRONIKA_PGBOUNCER_DSNS` | unset | Where to ask `PgBouncer` for `SHOW CONFIG`, which carries `logfile`. The account needs to be in `stats_users`; no administrative right beyond that. |
| `KRONIKA_PGBOUNCER_LOGS` | unset | `PgBouncer` logs named outright, paths or patterns. |

Set one PostgreSQL connection as a quoted keyword DSN:

```sh
KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres'
```

A `;` inside the same value separates additional DSNs. The first supplies
metric rows; later entries are used only to discover logs.

A log file's size is set by someone else's software, so it is read through a
4 MiB buffer and never held whole. One collection reads at most 256 MiB from
each file.

Every way a source can be missing gets the same treatment: the server is down,
`logging_collector` is off, `logfile` is unset, the file is not there yet, a
new instance appeared. One line in the log, and the whole set is worked out
again five minutes later. The collector keeps running either way, because its
first job is the operating system. Log discovery refreshes the path, format,
and `log_line_prefix` on each pass. It queries `system_identifier` until the
first successful result, then keeps that value for the lifetime of the process.

A DSN that reaches a server on another host reports a path that does not exist
here. That is one line naming the path, and the hint it carries is the answer:
mount the directory and name the file in `KRONIKA_PG_LOGS`.

What ends up in a row depends on how the source was named. From a DSN, a
`PostgreSQL` row carries the `system_identifier` that survives restarts and
renames. From a path, that column is null, the format is decided by reading the
first record, and a `stderr` prefix cannot be parsed at all, so a record keeps
its time and its text and nothing else. Every row carries the file it was read
from.

### Other settings

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_LOG_LEVEL` | `info` | One of `error`, `warn`, `info`, `debug`, `trace`. |
| `KRONIKA_PROC_ROOT` | `/proc` | Where procfs is mounted. Setting it also narrows container detection to the cgroup file under that root. |
| `KRONIKA_SYS_ROOT` | `/sys` | Where sysfs is mounted. |
| `KRONIKA_STATVFS_FIXTURE` | unset | Test hook: use `path=TOTAL:FREE:INODES:AVAILABLE_INODES;...` values instead of calling `statvfs`. |

Filesystem capacity is queried only for `ext2`, `ext3`, `ext4`, `xfs`,
`btrfs`, `f2fs`, `zfs`, `tmpfs`, and `overlay`. Network, FUSE/userspace,
`autofs`, and unknown filesystem types keep nullable capacity fields. One
helper process handles the allowlisted mounts under a single one-second
deadline so a blocked capacity query cannot stop later snapshots.
The mount snapshot stores the exact mount root and both byte and inode
total/available pairs. Sysfs topology stores only partitions with an exact
parent block-device identity; layered device ancestry remains unspecified.

## Run the collector

```sh
KRONIKA_OUT_DIR=/var/lib/kronika kronika-collector
```

`SIGTERM` and `SIGINT` stop the loop and leave the journal in place, so a
restart can recover the collected data. `SIGUSR2` collects one window
immediately without waiting for the next timer tick.
