# Class 1: PostgreSQL snapshots

[Русская версия](postgresql-metrics.ru.md)

PostgreSQL metric sections occupy `1_001_001`–`1_019_001`. Their exact
columns, units, keys, and semantics are declared in
[`crates/kronika-registry/src/codec`](../../crates/kronika-registry/src/codec).
This reference records which server or extension version selects each layout.

## Collection boundary

The collector reads one configured PostgreSQL instance: the first entry in
`KRONIKA_PG_DSNS`. Metric rows do not carry `system_identifier`.

Native server views use the PostgreSQL major version to select their layout.
Views tied to a database are read from each connectable database in that
instance. A source that is absent, unsupported, or unreadable produces no
section for that read.

The collector keeps one healthy connection per database across collection
cycles. Database and extension discovery runs about every five minutes. A
database that appears gets a connection; one that disappears loses it.

Extension inventory runs once in every database during discovery and records
the database, schema, and usable interfaces of each installation.
`pg_stat_statements` and `pg_store_plans` expose instance-wide counters, so the
collector reads one usable installation of each and does not duplicate shared
rows. An extension may be created, dropped, or moved to another database or
schema; the next discovery selects its new location. If no supported
installation is present, its sections are absent.
Each readable info view is selected independently of the main extension reader.
When installations expose different layouts, the collector chooses the newest
`pg_stat_statements` layout. `pg_store_plans` implementations are not ranked:
the current database wins when usable, otherwise database name is the
deterministic tie-break.

The `pg_stat_statements` layout follows its extension version. The
`pg_store_plans` family is selected by function signatures and result columns,
not by `extversion`. OSSC, Datasentinel, and vadv have distinct layouts.
Datasentinel adds relation OIDs and command type to the OSSC-shaped counters;
vadv has an internal `queryid`, `queryid_stat_statements`, and a four-key plan
getter.

Small administrative reads use Simple Query Protocol. Typed metric reads use
one-shot unnamed Extended Protocol queries. Queries run sequentially without
named prepared statements or pipelining. Large results are streamed in batches
of at most 256 rows, targeting approximately 512 KiB of decoded application
data. The final SQL-bounded row may exceed the byte target. A batch reaches the
WAL before the collector fetches another row. Statement and plan text is
limited in SQL to 65,536 characters per field. The collector does not select a
top-N subset or apply a shared text budget.
A timed-out query gets one bounded PostgreSQL CancelRequest before its
connection is closed.

## Native server views

| `type_id` | Section | PostgreSQL | Collection scope | Semantics |
|---|---|---|---|---|
| `1_001_001` | `pg_stat_activity` | 10–12 | instance | `snapshot_full` |
| `1_001_002` | `pg_stat_activity` | 13 | instance | `snapshot_full` |
| `1_001_003` | `pg_stat_activity` | 14–18 | instance | `snapshot_full` |
| `1_005_001` | `pg_stat_database` | 10–11 | instance | `snapshot_full` |
| `1_005_002` | `pg_stat_database` | 12–13 | instance | `snapshot_full` |
| `1_005_003` | `pg_stat_database` | 14–17 | instance | `snapshot_full` |
| `1_005_004` | `pg_stat_database` | 18 | instance | `snapshot_full` |
| `1_006_001` | `pg_stat_bgwriter` | 10–16 | instance | `snapshot_full` |
| `1_006_002` | `pg_stat_bgwriter` | 17–18 | instance | `snapshot_full` |
| `1_007_001` | `pg_stat_wal` | 14–17 | instance | `snapshot_full` |
| `1_007_002` | `pg_stat_wal` | 18 | instance | `snapshot_full` |
| `1_008_001` | `pg_stat_archiver` | 10–18 | instance | `snapshot_full` |
| `1_009_001` | `pg_stat_io` | 16–17 | instance | `snapshot_full` |
| `1_009_002` | `pg_stat_io` | 18 | instance | `snapshot_full` |
| `1_010_001` | `pg_prepared_xacts` | 10–18 | instance | `snapshot_full` |
| `1_011_001` | `pg_locks` | 10–13 | instance | `conditional_full` |
| `1_011_002` | `pg_locks` | 14–18 | instance | `conditional_full` |
| `1_012_001` | `pg_stat_progress_vacuum` | 10–16 | instance | `conditional_full` |
| `1_012_002` | `pg_stat_progress_vacuum` | 17 | instance | `conditional_full` |
| `1_012_003` | `pg_stat_progress_vacuum` | 18 | instance | `conditional_full` |
| `1_013_001` | `pg_stat_user_tables` | 10–12 | each database | `snapshot_full` |
| `1_013_002` | `pg_stat_user_tables` | 13–15 | each database | `snapshot_full` |
| `1_013_003` | `pg_stat_user_tables` | 16–17 | each database | `snapshot_full` |
| `1_013_004` | `pg_stat_user_tables` | 18 | each database | `snapshot_full` |
| `1_014_001` | `pg_stat_user_indexes` | 10–15 | each database | `snapshot_full` |
| `1_014_002` | `pg_stat_user_indexes` | 16–18 | each database | `snapshot_full` |
| `1_017_001` | `pg_stat_checkpointer` | 17 | instance | `snapshot_full` |
| `1_017_002` | `pg_stat_checkpointer` | 18 | instance | `snapshot_full` |
| `1_019_001` | `pg_settings` | 10–18 | instance | `on_change` |

`pg_stat_wal`, `pg_stat_io`, and `pg_stat_checkpointer` have no section before
the first PostgreSQL release listed above. PostgreSQL 17 and 18 changed
`pg_stat_progress_vacuum`, so those layouts have separate `type_id` values.
`pg_settings` is read on each PostgreSQL cycle and emitted on its first
successful read, when it changes, and in every new segment. The latest
successful snapshot is reused when another source opens a segment between
PostgreSQL cycles.
The `primary_conninfo` row is excluded because its value may contain a password.

## Extension views

| `type_id` | Section | Extension | Collection scope | Semantics |
|---|---|---|---|---|
| `1_002_001` | `pg_stat_statements` | `pg_stat_statements` 1.5–1.7 | discovered installation | `conditional_full` |
| `1_002_002` | `pg_stat_statements` | `pg_stat_statements` 1.8 | discovered installation | `conditional_full` |
| `1_002_003` | `pg_stat_statements` | `pg_stat_statements` 1.9 | discovered installation | `conditional_full` |
| `1_002_004` | `pg_stat_statements` | `pg_stat_statements` 1.10 | discovered installation | `conditional_full` |
| `1_002_005` | `pg_stat_statements` | `pg_stat_statements` 1.11 | discovered installation | `conditional_full` |
| `1_002_006` | `pg_stat_statements` | `pg_stat_statements` 1.12+ | discovered installation | `conditional_full` |
| `1_003_001` | `pg_store_plans_ossc` | OSSC-compatible zero-argument reader | discovered installation | `conditional_full` |
| `1_004_001` | `pg_store_plans_vadv` | vadv-compatible boolean reader and four-key getter | discovered installation | `conditional_full` |
| `1_015_001` | `pg_stat_statements_info` | `pg_stat_statements` 1.9+ | discovered installation | `snapshot_full` |
| `1_016_001` | `pg_store_plans_info` | exact readable `dealloc, stats_reset` view | discovered installation | `snapshot_full` |
| `1_018_001` | `pg_store_plans_datasentinel` | Datasentinel-compatible zero-argument reader with `relids` and `cmd_type` | discovered installation | `conditional_full` |

`pg_stat_statements` below 1.5 is not collected. Rows with a privilege-masked
`queryid` have no usable identity and are omitted. `pg_store_plans_info` is
selected by its exact readable view shape; the vadv interface does not expose
that view.
