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

### Which logs to follow

No log is followed unless its path is given. A log file's size is set by
someone else's software, so it is read through a fixed buffer and never held
whole; a file that grows faster than the collector reads it is read at 4 MiB
per tick until it catches up.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_PG_LOG` | unset | The `PostgreSQL` log to follow. The file name decides the format: `.csv` is `csvlog`, `.json` is `jsonlog`, anything else is `stderr`. |
| `KRONIKA_PGBOUNCER_LOG` | unset | The `PgBouncer` log to follow. `PgBouncer` writes to a file only when `logfile` is set in `pgbouncer.ini`. |
| `KRONIKA_PG_DSN` | unset | How to reach `PostgreSQL` to read `log_line_prefix`. Only a `stderr` log needs it, and only for the database and user of a record. |

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
