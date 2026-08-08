# kronika-collector

[Русская версия](README.ru.md)

`kronika-collector` reads operating-system metrics at configured intervals and
writes Kronika segments. It has no public command-line interface; environment
variables provide its configuration.

## Configuration

Every variable below is read and parsed once, before the first collection. A
value that does not parse stops the daemon with a message naming the variable
and the invalid value. The daemon does not substitute a default.

There is no per-source row cap. A source returns every available row. Each
`segment_write_finish` log record includes `rss_kib`, the process's peak
resident set size.

`KRONIKA_OUT_DIR` is the only required variable.

### Storage

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_OUT_DIR` | — | Data root: the journal, the finished segments, and the writer lock. |
| `KRONIKA_SEGMENT_MAX_BYTES` | 64 MiB | Write the open segment once the journal holds this many raw bytes. |
| `KRONIKA_SEGMENT_MAX_AGE_S` | 900 | Write an open segment at this age even if the byte cap was not reached. |
| `KRONIKA_JOURNAL_MAX_BYTES` | 1 GiB | Hard cap of `active.wal`. Reaching it writes the open segment early rather than failing the append. |
| `KRONIKA_RETENTION` | 2 GiB | Rotation target for the whole tree: a byte budget, `auto` (= `auto:80`), or `auto:P` for a used-fraction target of the backing partition. |

### Collection intervals

The scheduler tracks each source interval independently. The timer sleeps for
at most `KRONIKA_INTERVAL_S` and wakes earlier when a source becomes due. A
source interval of `0` reads on every timer cycle.

| Variable | Default, s | Sections |
| --- | ---: | --- |
| `KRONIKA_INTERVAL_S` | 5 | Maximum timer sleep; `0` disables the timer and leaves collection to signals. |
| `KRONIKA_OS_CORE_INTERVAL_S` | 10 | `1_102`–`1_111`, `1_114`–`1_120`. |
| `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 | `1_112`, `1_113`. |
| `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 | `1_100`. |
| `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 | `1_101`. |
| `KRONIKA_OS_CGROUP_INTERVAL_S` | 10 | `1_201`–`1_204`. |
| `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | `1_200`. |
| `KRONIKA_LOG_INTERVAL_S` | 10 | `2_001`–`2_007`, `2_100`. |
| `KRONIKA_PG_INTERVAL_S` | 30 | `1_001`–`1_009`, `1_012`, `1_019`. |
| `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 | `1_013`, `1_014`. |

### Which server to ask for metrics

The first entry of `KRONIKA_PG_DSNS` is also where the `PostgreSQL` metric
sections come from. A metric row carries no column naming the server that
produced it, so a second DSN is followed for its log only, and starting with
more than one is one line in the log saying which was chosen.

Per-table and per-index statistics exist only inside the database that produced
them, so the collector opens one connection per database and replaces every
connection once it reaches an hour. The database list is read on each of those
cycles: a database created since the last one starts being collected, one that
was dropped stops.

`pg_stat_statements` and `pg_store_plans` are collected where they are
installed, in whichever release is installed; a server without them simply
produces nothing for those sections. A read takes the costliest 500 plans and
up to a mebibyte of plan text.

Every segment carries a full `pg_settings` snapshot, so a segment read on its
own says what the numbers in it were produced under.

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
| `KRONIKA_PG_DSNS` | unset | Where to ask `PostgreSQL` for `pg_current_logfile()`, `log_line_prefix` and its `system_identifier`. |
| `KRONIKA_PG_LOGS` | unset | `PostgreSQL` logs named outright. An entry with `*` or `?` in its last component is a pattern matched against that directory. |
| `KRONIKA_PGBOUNCER_DSNS` | unset | Where to ask `PgBouncer` for `SHOW CONFIG`, which carries `logfile`. The account needs to be in `stats_users`; no administrative right beyond that. |
| `KRONIKA_PGBOUNCER_LOGS` | unset | `PgBouncer` logs named outright, paths or patterns. |

A log file's size is set by someone else's software, so it is read through a
fixed buffer and never held whole; a file that grows faster than the collector
reads it is read at 4 MiB per tick until it catches up.

Every way a source can be missing gets the same treatment: the server is down,
`logging_collector` is off, `logfile` is unset, the file is not there yet, a
new instance appeared. One line in the log, and the whole set is worked out
again five minutes later. The collector keeps running either way, because its
first job is the operating system.

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
| `KRONIKA_STATVFS_FIXTURE` | unset | Test hook: use `path=TOTAL:FREE;...` capacity values instead of calling `statvfs`. |

Filesystem capacity is queried only for `ext2`, `ext3`, `ext4`, `xfs`,
`btrfs`, `f2fs`, `zfs`, `tmpfs`, and `overlay`. Network, FUSE/userspace,
`autofs`, and unknown filesystem types keep nullable capacity fields. One
helper process handles the allowlisted mounts under a single one-second
deadline so a blocked capacity query cannot stop later snapshots.

## Run the collector

```sh
KRONIKA_OUT_DIR=/var/lib/kronika kronika-collector
```

`SIGTERM` and `SIGINT` stop the loop and leave the journal in place, so a
restart can recover the collected data. `SIGUSR2` collects one window
immediately without waiting for the next timer tick.
