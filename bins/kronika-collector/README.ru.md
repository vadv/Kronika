# kronika-collector

[English version](README.md) · [Установка](../../INSTALL.ru.md)

Collector читает метрики Linux/PostgreSQL и локальные логи PostgreSQL/PgBouncer,
затем пишет `active.wal` и сжатые `.zms`. Конфигурация читается один раз при
запуске; некорректные значения останавливают запуск. `KRONIKA_STORAGE_DIR`
обязательна. Исходники: [конфигурация](src/config.rs), [scheduler](src/scheduler.rs),
[основной цикл](src/main.rs).

## Конфигурация

<a id="storage"></a>
### Хранение

| Переменная | По умолчанию | Допустимое значение и смысл |
| --- | --- | --- |
| `KRONIKA_STORAGE_DIR` | Обязательна | Обычный data-root directory для журнала, сегментов и writer ownership lock. |
| `KRONIKA_SEGMENT_MAX_BYTES` | `67108864` (64 MiB) | Положительные unsigned bytes; порог размера журнала для публикации сегмента. |
| `KRONIKA_SEGMENT_MAX_AGE_S` | `900` | Unsigned seconds; порог возраста открытого сегмента. `0` сразу разрешает публикацию. |
| `KRONIKA_JOURNAL_MAX_BYTES` | `1073741824` (1 GiB) | `36..1073741824` bytes; жёсткий лимит журнала. При достижении сегмент публикуется досрочно. |
| `KRONIKA_RETENTION` | `2147483648` (2 GiB) | Unsigned byte budget, `auto` (= `auto:80`) или `auto:P`, `P=1..99`. |

Для fixed retention budget `B` и segment threshold `S` проверка требует
`B >= 2 × S` (saturating `u64` multiplication). Fixed mode учитывает `active.wal`,
готовые `.zms`, `.idx` sidecars и известные временные файлы. Для `auto:P` зададим `F = f_blocks × f_frsize` и
`U = F − f_bfree × f_frsize` по `statvfs` backing filesystem.
Byte threshold равен `floor(F × P / 100)`. Ротация сравнивает его с
`max(0, U − pending_reclaim)`. `pending_reclaim` накапливает bytes, удалённые
ротацией, но ещё не отражённые в свободном месте filesystem. Каждое наблюдаемое
снижение `U` уменьшает pending value до нуля. Учитывается вся filesystem.

Ротация выполняется после collection cycle с публикацией сегментов и по
минутному timer. Выполняемый сбор может задержать timer. Fixed mode пересчитывает
файлы каждый час, включая новые web indexes. Порядок удаления: устаревшие writer
ZMS temporaries, orphan indexes, затем старейшие готовые сегменты с их indexes.
`active.wal`, самый новый готовый сегмент и посторонние файлы сохраняются.
Если оставшиеся файлы превышают цель, сбор продолжается с записью
`rotation_degraded`. Исходник: [rotation.rs](src/rotation.rs).

### Интервалы сбора

Все интервалы — unsigned whole seconds. Источники имеют независимые due times.
Значение `0` у источника означает чтение на каждом timer cycle.

| Переменная | По умолчанию, s | Данные |
| --- | ---: | --- |
| `KRONIKA_INTERVAL_S` | 5 | Максимальный timer sleep; `0` отключает сбор по timer. Положительные source intervals могут будить timer раньше. |
| `KRONIKA_OS_CORE_INTERVAL_S` | 10 | CPU, memory, disks, network, PSI. |
| `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 | Mounts, filesystem capacity и device topology. |
| `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 | Process counters. |
| `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 | Process status details. |
| `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 | Container cgroup controller rows для direct live memberships. |
| `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | Process-to-cgroup mappings. |
| `KRONIKA_LOG_INTERVAL_S` | 10 | Настроенные логи PostgreSQL/PgBouncer. |
| `KRONIKA_PG_INTERVAL_S` | 30 | Метрики PostgreSQL и settings. |
| `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 | Relations; обнаружение databases и extensions. |

### Подключения и логи

| Переменная | По умолчанию | Значение |
| --- | --- | --- |
| `KRONIKA_PG_DSNS` | Не задана | PostgreSQL keyword DSNs или URLs через `;`. Первый DSN включает метрики сервера; каждый DSN обнаруживает локальные пути/формат логов. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Не задана | Целое `1..4294967295`, effective CPU capacity первого PostgreSQL target. Требует `KRONIKA_PG_DSNS`; записывается как operand для PostgreSQL health. |
| `KRONIKA_PG_LOGS` | Не задана | Локальные PostgreSQL paths или globs через `;`. Только последний компонент поддерживает `*` и `?`. |
| `KRONIKA_PGBOUNCER_DSNS` | Не задана | Admin-console DSNs (`dbname=pgbouncer`) через `;` для `SHOW CONFIG`/`logfile`; account входит в `stats_users`. |
| `KRONIKA_PGBOUNCER_LOGS` | Не задана | Локальные PgBouncer paths или final-component globs через `;`. |

Пустой список не выбирает источников. Пустые элементы между `;` — ошибка.
Первый PostgreSQL DSN даёт метрики начальной database и других доступных для
подключения non-template databases того же сервера. Остальные DSNs дают только
log discovery. PostgreSQL metric rows не содержат server identity column.

### Прочие параметры

| Переменная | По умолчанию | Значение |
| --- | --- | --- |
| `KRONIKA_LOG_LEVEL` | `info` | Без учёта регистра: `error`, `warn`/`warning`, `info`, `debug`, `trace`; structured logs в stderr. |
| `KRONIKA_PROC_ROOT` | `/proc` | procfs root; явная настройка ограничивает container detection cgroup-файлом этого root. |
| `KRONIKA_SYS_ROOT` | `/sys` | sysfs root. |
| `KRONIKA_STATVFS_FIXTURE` | Не задана | Test hook: `path=TOTAL:FREE:INODES:AVAILABLE_INODES;...` заменяет значения `statvfs`. |

## Сбор PostgreSQL

<a id="postgresql-role"></a>
### PostgreSQL role

[Команды создания роли](../../INSTALL.ru.md#5-postgresql) находятся в установке.
Runtime privileges:

| Scope | Требуемое право |
| --- | --- |
| Role | Наследуемое членство в `pg_monitor`; collector не выполняет `SET ROLE`. |
| Каждая собираемая database | `CONNECT`; обычный доступ к чтению catalog и functions. |
| Выбранная extension schema | `USAGE`. |
| Reader `pg_stat_statements` | `EXECUTE` на `pg_stat_statements(boolean)`. |
| Reader `pg_store_plans` | `EXECUTE` на установленную `pg_store_plans()` или `pg_store_plans(boolean)`; vadv interface также требует `pg_store_plans_get_plan(oid, oid, bigint, bigint)` и `pg_store_plans_textplan(text)`. |
| Установленный `*_info` interface | `SELECT` на info view и `EXECUTE` на его zero-argument function. |
| Каждая PostgreSQL log-discovery database | `EXECUTE` на `pg_catalog.pg_current_logfile()` и `pg_catalog.pg_control_system()`. |

Schema, view и function privileges локальны для database. Extension readers
вызываются напрямую. Default grants PostgreSQL/extensions дают часть этих прав;
явно отозванные права нужно выдать monitoring role. Явный grant
`pg_current_logfile()` нужен на PostgreSQL 10–16.

### Обнаружение и срок жизни sessions

| Объект | Контракт |
| --- | --- |
| Database sessions | Одно повторно используемое подключение на доступную database, максимальный healthy age — один час. Discovery добавляет/удаляет databases на каждом relation interval. |
| Extension inventory | Один inventory query на database при discovery; schema и callable interface кешируются. Выбирается одна usable installation каждого extension. |
| `pg_stat_statements` | Поддерживает extension `1.5+` серии `1.x`; PostgreSQL 14+ требует `1.9+`. Выбирается новейший compatible layout, затем current database, затем database name. |
| `pg_store_plans` | Отдельные zero-argument layouts OSSC и Datasentinel; vadv boolean interface требует four-key getter и native text converter. Выбор implementation: current database, затем database name. |
| Info views | `pg_stat_statements_info` и `pg_store_plans_info` обнаруживаются независимо от основных readers. |
| Settings | Читаются на каждом PostgreSQL tick; полный snapshot после первого успешного чтения, при изменении и в каждом сегменте. Последний успешный snapshot используется, когда другие источники открывают сегмент. |
| Исключения settings | `primary_conninfo` и `ssl_passphrase_command` пропускаются; остальные command и custom settings записываются. |

Исходники: [database pool](../../crates/kronika-source-pg/src/pool.rs),
[обнаружение extensions](../../crates/kronika-source-pg/src/extension.rs),
[settings](../../crates/kronika-source-pg/src/settings.rs),
[recorded layouts](../../docs/type-registry/postgresql-metrics.ru.md).

### Выполнение запросов

| Объект | Значение или поведение |
| --- | --- |
| Transport | `NoTls`; прямой PostgreSQL или PgBouncer session pooling. Transaction/statement pooling не сохраняют session state, нужный metric reads. |
| Protocol | Administrative reads: Simple Query Protocol. Typed metrics: one-shot unnamed Extended Protocol. Один запрос одновременно на подключение. |
| Session initialization | Один `SET statement_timeout = '30s'` перед использованием. |
| Client fetch deadline | 35 секунд, затем попытка CancelRequest с deadline одна секунда и закрытие подключения. |
| Collector identity | Один уникальный `application_name` на процесс collector; Activity/Locks исключают это точное имя. |
| Batch bounds | До 256 rows, целевой размер — 512 KiB decoded logical data; последняя SQL-bounded row может превысить byte target. Batch записывается в WAL до получения следующего. |
| Text bounds | Statement и plan text ограничены в SQL до 65,536 characters. |
| Stream error | Уже записанные batches остаются; остаток чтения пропускается, независимые источники продолжают сбор. |
| SQLSTATE `57014` | Считается query timeout; session пригодна для повторного использования после `ReadyForQuery`. |
| Query logs | Debug `pg_query_finish`; warning `pg_query_slow` при fetch свыше 500 ms; summary примерно каждые пять минут и при остановке. |

`pg_query_summary` записывает query count/rate, rows, logical bytes, errors,
timeouts, slow queries, fetch/encoding/WAL times, encoded/appended bytes и
`peak_rss_kib`. Connection labels имеют вид `user@host:port`. Исходник:
[query.rs](../../crates/kronika-source-pg/src/query.rs).

## Сбор логов

DSN discovery даёт локальный path, format, PostgreSQL `log_line_prefix` и
`system_identifier`. Paths/globs напрямую задают файлы на машине collector.
Файл, найденный обоими способами, читается один раз с обнаруженными metadata.

| Свойство | Поведение |
| --- | --- |
| Discovery cadence | Пять минут; повторные попытки после ошибок. `system_identifier` кешируется после первого успешного чтения. |
| Read bound | Physical buffer 64 KiB; batches до 4 MiB raw bytes; до 256 MiB на файл за один сбор. |
| PostgreSQL formats | Имя файла выбирает `.csv` → csvlog, `.json` → jsonlog, остальные → stderr. |
| Path-only identity | `system_identifier` — null; каждая строка содержит source file. |
| Path-only stderr | Database/user недоступны; severity, SQLSTATE при наличии, message и continuations разбираются. Используется parsed timestamp при наличии, иначе время сбора. |
| Source error | Записывается в лог; другой сбор продолжается. |

Исходники: [log collector](../../crates/kronika-source-log/src),
[PostgreSQL parser](../../crates/kronika-source-log/src/postgres.rs).

## Linux scope

Recorded environment определяется при сборе. Запуск на machine/VM не собирает
cgroup workload rows. Запуск в container собирает direct live memberships;
limits, controller paths и resource formulas определены в
[справочнике метрик Linux](../../docs/metrics-linux.ru.md).

Filesystem capacity запрашивается для `ext2`, `ext3`, `ext4`, `xfs`, `btrfs`,
`f2fs`, `zfs`, `tmpfs` и `overlay`. У остальных типов capacity fields остаются
null. Один helper process обрабатывает allowlisted mounts с общим deadline
одна секунда. Mount rows записывают точные mount roots и byte/inode capacity;
topology — edges partition/device и layered-device/slave. В container topology
ограничена цепочками mounted или cgroup-charged devices.

## Запуск и signals

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika /usr/local/bin/kronika-collector
```

`SIGINT` и `SIGTERM` останавливают сбор и сохраняют журнал. `SIGUSR2` немедленно
собирает данные и запрашивает публикацию сегмента, если цикл добавил данные и
сегмент непустой. `-h`, `--help` и `--version` завершаются до
конфигурации и обращения к хранилищу. Readiness и segment paths идут в stdout;
structured logs — в stderr. Каждый `segment_write_finish` содержит `rss_kib`,
peak resident set size процесса в KiB.
