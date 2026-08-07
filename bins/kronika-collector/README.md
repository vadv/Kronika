# kronika-collector

A daemon that reads the operating system on a tick and writes the result as
Kronika segments. It has no command line: everything comes from the
environment.

## Configuration

Every variable below is read and parsed once, before the first collection. A
value that does not parse stops the daemon with a message naming the variable
and what it was given; nothing falls back to a default silently.

`KRONIKA_OUT_DIR` is the only required variable.

### Where the data goes

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_OUT_DIR` | — | Data root: the journal, the finished segments, and the writer lock. |
| `KRONIKA_SEGMENT_MAX_BYTES` | 64 MiB | Write the open segment once the journal holds this many raw bytes; `0` writes on every tick. |
| `KRONIKA_SEGMENT_MAX_AGE_S` | 900 | Write an open segment at this age even if the byte cap was not reached. |
| `KRONIKA_JOURNAL_MAX_BYTES` | 1 GiB | Hard cap of `active.wal`. Reaching it writes the open segment early rather than failing the append. |
| `KRONIKA_RETENTION` | unset | Rotation target for the whole tree: a byte budget, `auto` (= `auto:80`), or `auto:P` for a used-fraction target of the backing partition. Unset keeps every segment. |

### How often each source is read

The base tick is `KRONIKA_INTERVAL_S`; each source runs on its own multiple of
it. A source interval of `0` reads on every tick. An interval equal to the tick
reads on most ticks but not reliably every one, because a wake that lands a
fraction early leaves the interval unelapsed.

| Variable | Default, s | Sections |
| --- | ---: | --- |
| `KRONIKA_INTERVAL_S` | 5 | The scheduler tick itself; `0` disables the timer and leaves collection to signals. |
| `KRONIKA_INSTANCE_INTERVAL_S` | 60 | Instance metadata. |
| `KRONIKA_OS_CORE_INTERVAL_S` | 10 | `1_102`–`1_111`, `1_114`–`1_120`. |
| `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 | `1_112`, `1_113`. |
| `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 | `1_100`. |
| `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 | `1_101`. |
| `KRONIKA_OS_CGROUP_INTERVAL_S` | 10 | `1_201`–`1_204`. |
| `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | `1_200`. |

### How much each source may read

Every path that walks a directory has a ceiling, because the collector shares a
host with a production database. A source that hits its ceiling logs one
`collection_degraded` line naming the ceiling and how many rows it dropped.

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_OS_MAX_PROCS` | 4096 | Processes read per tick, ordered by pid. |
| `KRONIKA_OS_MAX_CGROUPS` | 1024 | cgroup nodes read per tick. |
| `KRONIKA_OS_MAX_CGROUP_IO_ROWS` | 4096 | `io.stat` rows across all cgroups. |
| `KRONIKA_OS_CGROUP_MAX_DEPTH` | 8 | Depth of the cgroup tree walk below the root. |
| `KRONIKA_OS_MAX_DISKS` | 256 | Devices kept from `/proc/diskstats`, lowest `(major, minor)` first. |
| `KRONIKA_OS_MAX_IRQ_ROWS` | 512 | Lines kept from `/proc/interrupts`. |

### Everything else

| Variable | Default | Meaning |
| --- | ---: | --- |
| `KRONIKA_LOG_LEVEL` | `info` | One of `error`, `warn`, `info`, `debug`, `trace`. |
| `KRONIKA_PROC_ROOT` | `/proc` | Where procfs is mounted. Setting it also narrows container detection to the cgroup file under that root. |
| `KRONIKA_SYS_ROOT` | `/sys` | Where sysfs is mounted. |
| `KRONIKA_STATVFS_FIXTURE` | unset | Test hook: read filesystem capacity from this file instead of calling `statvfs`. |

## Running it

```sh
KRONIKA_OUT_DIR=/var/lib/kronika kronika-collector
```

`SIGTERM` and `SIGINT` stop the loop and leave the journal in place, so a
restart loses no collected data. `SIGUSR2` collects one window immediately
without waiting for the tick.
