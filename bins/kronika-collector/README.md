# kronika-collector

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

Collector reads Linux/PostgreSQL metrics and local PostgreSQL/PgBouncer logs,
then writes `active.wal` and compressed `.zms` segments. Configuration is read
once at startup; invalid values terminate startup. `KRONIKA_STORAGE_DIR` is
required. Sources: [configuration](src/config.rs), [scheduler](src/scheduler.rs),
[main loop](src/main.rs).

## Configuration

<a id="storage"></a>
### Storage

| Variable | Default | Accepted value and meaning |
| --- | --- | --- |
| `KRONIKA_STORAGE_DIR` | Required | Real data-root directory for journal, segments and writer ownership lock. |
| `KRONIKA_SEGMENT_MAX_BYTES` | `67108864` (64 MiB) | Positive unsigned bytes; journal-size threshold for segment publication. |
| `KRONIKA_SEGMENT_MAX_AGE_S` | `900` | Unsigned seconds; open-segment age threshold. `0` makes publication immediately eligible. |
| `KRONIKA_JOURNAL_MAX_BYTES` | `1073741824` (1 GiB) | `36..1073741824` bytes; hard journal cap. Reaching it publishes the segment early. |
| `KRONIKA_RETENTION` | `2147483648` (2 GiB) | Unsigned byte budget, `auto` (= `auto:80`), or `auto:P`, `P=1..99`. |

For fixed retention budget `B` and segment threshold `S`, validation requires
`B >= 2 × S` (saturating `u64` multiplication). Fixed mode counts `active.wal`,
finished `.zms`, `.idx` sidecars and recognized temporary files. For `auto:P`, let `F = f_blocks × f_frsize` and
`U = F − f_bfree × f_frsize` from the backing filesystem's `statvfs`.
The byte threshold is `floor(F × P / 100)`. Rotation compares it with
`max(0, U − pending_reclaim)`. `pending_reclaim` accumulates bytes unlinked by
rotation but not yet reflected in filesystem free space. Each observed fall in
`U` reduces that pending value, down to zero. This measures the entire filesystem.

Rotation runs after a collection cycle publishes segments and on a one-minute
timer. A collection in progress can delay the timer. Fixed mode recounts files
hourly to include new web indexes. Deletion order: stale writer ZMS temporaries,
orphan indexes, then oldest finished segments with their sibling indexes.
`active.wal`, the newest finished segment and unrelated files are retained.
If the retained files exceed the target, collection continues and logs
`rotation_degraded`. Source: [rotation.rs](src/rotation.rs).

### Collection intervals

All interval values are unsigned whole seconds. Sources have independent due
times. A per-source `0` reads on every timer cycle.

| Variable | Default, s | Data |
| --- | ---: | --- |
| `KRONIKA_INTERVAL_S` | 5 | Maximum timer sleep; `0` disables timed collection. Positive source intervals can wake the timer earlier. |
| `KRONIKA_OS_CORE_INTERVAL_S` | 10 | CPU, memory, disks, network, PSI. |
| `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 | Mounts, filesystem capacity and device topology. |
| `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 | Process counters. |
| `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 | Process status details. |
| `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 | Container cgroup controller rows for direct live memberships. |
| `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | Process-to-cgroup mappings. |
| `KRONIKA_LOG_INTERVAL_S` | 10 | Configured PostgreSQL/PgBouncer logs. |
| `KRONIKA_PG_INTERVAL_S` | 30 | PostgreSQL metrics and settings. |
| `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 | Relations; database and extension discovery. |

### Connections and logs

| Variable | Default | Meaning |
| --- | --- | --- |
| `KRONIKA_PG_DSNS` | Unset | Semicolon-separated PostgreSQL keyword DSNs or URLs. First DSN enables server metrics; every DSN discovers local log paths/format. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Unset | Integer `1..4294967295`, CPU capacity available to the first PostgreSQL target, including a remote server or a separate cgroup. Requires `KRONIKA_PG_DSNS`; recorded as the operand for PostgreSQL health. |
| `KRONIKA_PG_LOGS` | Unset | Semicolon-separated local PostgreSQL paths or globs. Only the final component supports `*` and `?`. |
| `KRONIKA_PGBOUNCER_DSNS` | Unset | Semicolon-separated admin-console DSNs (`dbname=pgbouncer`) for `SHOW CONFIG`/`logfile`; account belongs to `stats_users`. |
| `KRONIKA_PGBOUNCER_LOGS` | Unset | Semicolon-separated local PgBouncer paths or final-component globs. |

Blank lists select no sources. Blank entries between semicolons are errors.
The first PostgreSQL DSN supplies metrics from its initial database and other
connectable non-template databases on that server. Further DSNs supply log
discovery only. PostgreSQL metric rows have no server identity column.

### Other settings

| Variable | Default | Meaning |
| --- | --- | --- |
| `KRONIKA_LOG_LEVEL` | `info` | Case-insensitive `error`, `warn`/`warning`, `info`, `debug`, `trace`; structured logs on stderr. |
| `KRONIKA_PROC_ROOT` | `/proc` | procfs root; setting it limits container detection to that root's cgroup file. |
| `KRONIKA_SYS_ROOT` | `/sys` | sysfs root. |
| `KRONIKA_STATVFS_FIXTURE` | Unset | Test hook: `path=TOTAL:FREE:INODES:AVAILABLE_INODES;...` substitutes `statvfs` values. |

## PostgreSQL collection

<a id="postgresql-role"></a>
### PostgreSQL role

[Role creation commands](../../INSTALL.md#5-postgresql) belong to installation.
Runtime privileges:

| Scope | Required privilege |
| --- | --- |
| Role | Inherited `pg_monitor` membership; collector does not issue `SET ROLE`. |
| Each collected database | `CONNECT`; normal catalog read/function access. |
| Selected extension schema | `USAGE`. |
| `pg_stat_statements` reader | `EXECUTE` on `pg_stat_statements(boolean)`. |
| `pg_store_plans` reader | `EXECUTE` on installed `pg_store_plans()` or `pg_store_plans(boolean)`; vadv interface also needs `pg_store_plans_get_plan(oid, oid, bigint, bigint)` and `pg_store_plans_textplan(text)`. |
| Installed `*_info` interface | `SELECT` on the info view and `EXECUTE` on its zero-argument function. |
| Each PostgreSQL log-discovery database | `EXECUTE` on `pg_catalog.pg_current_logfile()` and `pg_catalog.pg_control_system()`. |

Schema, view and function privileges are database-local. Extension readers are
called directly. Default PostgreSQL/extension grants supply some of these
permissions; explicitly revoked privileges must be granted to the monitoring
role. The explicit `pg_current_logfile()` grant is needed on PostgreSQL 10–16.

### Discovery and session lifetime

| Item | Contract |
| --- | --- |
| Database sessions | One reused connection per connectable database, maximum healthy age one hour. Discovery adds/removes databases every relation interval. |
| Extension inventory | One inventory query per database each discovery pass; cached schema and callable interface. One usable installation of each extension is selected. |
| `pg_stat_statements` | Supports extension `1.5+` in the `1.x` series; PostgreSQL 14+ requires `1.9+`. Newest compatible layout wins, then current database, then database name. |
| `pg_store_plans` | Separate OSSC and Datasentinel zero-argument layouts; vadv boolean interface requires its four-key getter and native text converter. Implementation selection uses current database, then database name. |
| Info views | `pg_stat_statements_info` and `pg_store_plans_info` are discovered independently of the main readers. |
| Settings | Read each PostgreSQL tick; full snapshot after first successful read, on change, and in every segment. Latest successful snapshot is reused when other sources open a segment. |
| Settings exclusions | `primary_conninfo` and `ssl_passphrase_command` are omitted; other command and custom settings are recorded. |

Sources: [database pool](../../crates/kronika-source-pg/src/pool.rs),
[extension discovery](../../crates/kronika-source-pg/src/extension.rs),
[settings](../../crates/kronika-source-pg/src/settings.rs),
[recorded layouts](../../docs/type-registry/postgresql-metrics.md).

### Query execution

| Item | Value or behavior |
| --- | --- |
| Transport | `NoTls`; direct PostgreSQL or PgBouncer session pooling. Transaction/statement pooling do not retain the session state required by metric reads. |
| Protocol | Administrative reads: Simple Query Protocol. Typed metrics: one-shot unnamed Extended Protocol. One query at a time per connection. |
| Session initialization | One `SET statement_timeout = '30s'` before use. |
| Client fetch deadline | 35 seconds, then a CancelRequest attempt with a one-second deadline and connection close. |
| Collector identity | One unique `application_name` per collector process; Activity/Locks exclude that exact name. |
| Batch bounds | At most 256 rows, targeting 512 KiB decoded logical data; the final SQL-bounded row can exceed the byte target. Each batch reaches WAL before fetching the next. |
| Text bounds | Statement and plan text limited to 65,536 characters in SQL. |
| Stream error | Earlier appended batches remain; the remaining read is skipped and independent sources continue. |
| SQLSTATE `57014` | Counted as query timeout; session is reusable after `ReadyForQuery`. |
| Query logs | Debug `pg_query_finish`; warning `pg_query_slow` when fetch exceeds 500 ms; summary about every five minutes and at shutdown. |

`pg_query_summary` records query count/rate, rows, logical bytes, errors,
timeouts, slow queries, fetch/encoding/WAL times, encoded/appended bytes and
`peak_rss_kib`. Connection labels are `user@host:port`. Source:
[query.rs](../../crates/kronika-source-pg/src/query.rs).

## Log collection

DSN discovery supplies the local path, format, PostgreSQL `log_line_prefix` and
`system_identifier`. Paths/globs directly name files on the collector host.
A file reached through both methods is followed once with discovered metadata.

| Property | Behavior |
| --- | --- |
| Discovery cadence | Five minutes; retries after errors. `system_identifier` is cached after its first successful read. |
| Read bound | 64 KiB physical buffer; batches of at most 4 MiB raw bytes; at most 256 MiB per file per collection. |
| PostgreSQL formats | Filename selects `.csv` → csvlog, `.json` → jsonlog, otherwise stderr. |
| Path-only identity | `system_identifier` is null; every row records its source file. |
| Path-only stderr | Database/user are unavailable; severity, SQLSTATE when present, message and continuations are parsed. Parsed timestamp is used when present, otherwise collection time. |
| Source error | Logged; other collection continues. |

Sources: [log collector](../../crates/kronika-source-log/src),
[PostgreSQL parser](../../crates/kronika-source-log/src/postgres.rs).

## Linux scope

The recorded environment is established at collection time. Machine/VM runs
collect no cgroup workload rows. Container runs collect direct live memberships;
limits, controller paths and resource formulas are defined in the
[Linux metric reference](../../docs/metrics-linux.md).

Filesystem capacity is queried for `ext2`, `ext3`, `ext4`, `xfs`, `btrfs`,
`f2fs`, `zfs`, `tmpfs` and `overlay`. Other types retain null capacity fields.
One helper process handles allowlisted mounts under a single one-second
deadline. Mount rows record exact mount roots and byte/inode capacity;
topology records partition/device and layered-device/slave edges. In containers,
topology is restricted to chains under mounted or cgroup-charged devices.

## Run and signals

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika /usr/local/bin/kronika-collector
```

`SIGINT` and `SIGTERM` stop collection and retain the journal. `SIGUSR2` collects
immediately and requests segment publication when the cycle appends data and the
segment is nonempty. `-h`, `--help` and `--version` exit before
configuration or storage access. Readiness and segment paths go to stdout;
structured logs go to stderr. Each `segment_write_finish` records `rss_kib`,
the process peak resident set size in KiB.
