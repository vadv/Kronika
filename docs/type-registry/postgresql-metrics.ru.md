# Класс 1: снимки PostgreSQL

[English version](postgresql-metrics.md)

Секции метрик PostgreSQL занимают диапазон `1_001_001`–`1_019_001`. Точные
столбцы, единицы, ключи и семантика объявлены в
[`crates/kronika-registry/src/codec`](../../crates/kronika-registry/src/codec).
В этом справочнике указано, какая версия сервера или расширения выбирает каждый
формат.

## Граница сбора

Коллектор читает один настроенный инстанс PostgreSQL: первую запись в
`KRONIKA_PG_DSNS`. Строки метрик не содержат `system_identifier`.

Для системных представлений формат выбирается по основной версии PostgreSQL.
Представления, привязанные к базе, читаются из каждой доступной для подключения
базы этого инстанса. Если источник отсутствует, не поддерживается или не
читается, при этом чтении секция не создаётся.

Представления расширений обнаруживаются в базах настроенного инстанса. При
обнаружении сохраняются база и схема, которым принадлежит каждая установка;
имя представления дополняется этой схемой. `pg_stat_statements` и
`pg_store_plans` показывают счётчики всего инстанса, поэтому достаточно одной
найденной установки. Расширение можно создать, удалить или перенести в другую
базу либо схему; при следующем обнаружении прежнее расположение заменяется.
Если поддерживаемой установки нет, её секции отсутствуют.

Коллектор выполняет безымянные запросы через расширенный протокол PostgreSQL.
Он не сохраняет именованные prepared statements на наблюдаемом сервере или в
соединении пулера.

## Системные представления

| `type_id` | Секция | PostgreSQL | Область сбора | Семантика |
|---|---|---|---|---|
| `1_001_001` | `pg_stat_activity` | 10–12 | инстанс | `snapshot_full` |
| `1_001_002` | `pg_stat_activity` | 13 | инстанс | `snapshot_full` |
| `1_001_003` | `pg_stat_activity` | 14–18 | инстанс | `snapshot_full` |
| `1_005_001` | `pg_stat_database` | 10–11 | инстанс | `snapshot_full` |
| `1_005_002` | `pg_stat_database` | 12–13 | инстанс | `snapshot_full` |
| `1_005_003` | `pg_stat_database` | 14–17 | инстанс | `snapshot_full` |
| `1_005_004` | `pg_stat_database` | 18 | инстанс | `snapshot_full` |
| `1_006_001` | `pg_stat_bgwriter` | 10–16 | инстанс | `snapshot_full` |
| `1_006_002` | `pg_stat_bgwriter` | 17–18 | инстанс | `snapshot_full` |
| `1_007_001` | `pg_stat_wal` | 14–17 | инстанс | `snapshot_full` |
| `1_007_002` | `pg_stat_wal` | 18 | инстанс | `snapshot_full` |
| `1_008_001` | `pg_stat_archiver` | 10–18 | инстанс | `snapshot_full` |
| `1_009_001` | `pg_stat_io` | 16–17 | инстанс | `snapshot_full` |
| `1_009_002` | `pg_stat_io` | 18 | инстанс | `snapshot_full` |
| `1_010_001` | `pg_prepared_xacts` | 10–18 | инстанс | `snapshot_full` |
| `1_011_001` | `pg_locks` | 10–13 | инстанс | `conditional_full` |
| `1_011_002` | `pg_locks` | 14–18 | инстанс | `conditional_full` |
| `1_012_001` | `pg_stat_progress_vacuum` | 10–18 | инстанс | `conditional_full` |
| `1_013_001` | `pg_stat_user_tables` | 10–12 | каждая база | `snapshot_full` |
| `1_013_002` | `pg_stat_user_tables` | 13–15 | каждая база | `snapshot_full` |
| `1_013_003` | `pg_stat_user_tables` | 16–17 | каждая база | `snapshot_full` |
| `1_013_004` | `pg_stat_user_tables` | 18 | каждая база | `snapshot_full` |
| `1_014_001` | `pg_stat_user_indexes` | 10–15 | каждая база | `snapshot_full` |
| `1_014_002` | `pg_stat_user_indexes` | 16–18 | каждая база | `snapshot_full` |
| `1_017_001` | `pg_stat_checkpointer` | 17 | инстанс | `snapshot_full` |
| `1_017_002` | `pg_stat_checkpointer` | 18 | инстанс | `snapshot_full` |
| `1_019_001` | `pg_settings` | 10–18 | инстанс | `on_change` |

До первой указанной версии PostgreSQL секции для `pg_stat_wal`, `pg_stat_io` и
`pg_stat_checkpointer` отсутствуют. Для `pg_stat_progress_vacuum` используется
один `type_id`; недоступные в конкретной версии сервера столбцы допускают
`NULL` по контракту.

## Представления расширений

| `type_id` | Секция | Расширение | Область сбора | Семантика |
|---|---|---|---|---|
| `1_002_001` | `pg_stat_statements` | `pg_stat_statements` 1.6–1.7 | найденная установка | `snapshot_full` |
| `1_002_002` | `pg_stat_statements` | `pg_stat_statements` 1.8 | найденная установка | `snapshot_full` |
| `1_002_003` | `pg_stat_statements` | `pg_stat_statements` 1.9 | найденная установка | `snapshot_full` |
| `1_002_004` | `pg_stat_statements` | `pg_stat_statements` 1.10 | найденная установка | `snapshot_full` |
| `1_002_005` | `pg_stat_statements` | `pg_stat_statements` 1.11 | найденная установка | `snapshot_full` |
| `1_002_006` | `pg_stat_statements` | `pg_stat_statements` 1.12+ | найденная установка | `snapshot_full` |
| `1_003_001` | `pg_store_plans_ossc` | OSSC `pg_store_plans` 1.10+ | найденная установка | `snapshot_full` |
| `1_004_001` | `pg_store_plans_vadv` | vadv `pg_store_plans` 2.x | найденная установка | `snapshot_full` |
| `1_015_001` | `pg_stat_statements_info` | `pg_stat_statements` 1.9+ | найденная установка | `snapshot_full` |
| `1_016_001` | `pg_store_plans_info` | OSSC `pg_store_plans` 1.10 | найденная установка | `snapshot_full` |

`pg_stat_statements` ниже 1.6 не собирается, потому что в нём нет `queryid`.
`pg_store_plans_info` собирается только для известного формата OSSC 1.10; для
формата vadv 2.x секции с этим `type_id` нет.
