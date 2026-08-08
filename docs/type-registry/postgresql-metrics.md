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

Extension views are discovered in the databases of the configured instance.
Discovery records the database and schema that own each installation, and the
collector qualifies the view with that schema. `pg_stat_statements` and
`pg_store_plans` expose instance-wide counters, so one discovered installation
is enough. An extension may be created, dropped, or moved to another database
or schema; a later discovery replaces the previous location. If no supported
installation is present, its sections are absent.

Collector queries use unnamed statements over PostgreSQL's extended query
protocol. The collector does not retain named prepared statements on the
monitored server or pooler connection.

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
| `1_012_001` | `pg_stat_progress_vacuum` | 10–18 | instance | `conditional_full` |
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
the first PostgreSQL release listed above. `pg_stat_progress_vacuum` keeps one
`type_id`; columns unavailable in a server release are nullable in that
contract.

## Extension views

| `type_id` | Section | Extension | Collection scope | Semantics |
|---|---|---|---|---|
| `1_002_001` | `pg_stat_statements` | `pg_stat_statements` 1.6–1.7 | discovered installation | `snapshot_full` |
| `1_002_002` | `pg_stat_statements` | `pg_stat_statements` 1.8 | discovered installation | `snapshot_full` |
| `1_002_003` | `pg_stat_statements` | `pg_stat_statements` 1.9 | discovered installation | `snapshot_full` |
| `1_002_004` | `pg_stat_statements` | `pg_stat_statements` 1.10 | discovered installation | `snapshot_full` |
| `1_002_005` | `pg_stat_statements` | `pg_stat_statements` 1.11 | discovered installation | `snapshot_full` |
| `1_002_006` | `pg_stat_statements` | `pg_stat_statements` 1.12+ | discovered installation | `snapshot_full` |
| `1_003_001` | `pg_store_plans_ossc` | OSSC `pg_store_plans` 1.10+ | discovered installation | `snapshot_full` |
| `1_004_001` | `pg_store_plans_vadv` | vadv `pg_store_plans` 2.x | discovered installation | `snapshot_full` |
| `1_015_001` | `pg_stat_statements_info` | `pg_stat_statements` 1.9+ | discovered installation | `snapshot_full` |
| `1_016_001` | `pg_store_plans_info` | OSSC `pg_store_plans` 1.10 | discovered installation | `snapshot_full` |

`pg_stat_statements` below 1.6 is not collected because it has no `queryid`.
`pg_store_plans_info` is collected only for the known OSSC 1.10 layout; the
vadv 2.x layout has no section with that `type_id`.
