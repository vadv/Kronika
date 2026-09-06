# Справочник метрик PostgreSQL

[English version](metrics-postgresql.md) · [Оглавление справочника](features.ru.md)

## Источники, identity и обозначения

Collector читает instance из первого элемента `KRONIKA_PG_DSNS`. Представления с областью database читаются через каждую доступную для подключения database; statements и plans — через одну обнаруженную установку каждого extension. [Справочник layouts](type-registry/postgresql-metrics.ru.md) содержит точные версии PostgreSQL и extensions, физические type IDs и области сбора. [Registry](../crates/kronika-registry/src/codec) определяет каждое записываемое поле.

Для samples с Unix timestamps в микросекундах `t₀ < t₁` используются `Δx = x₁ − x₀`, `d = (t₁ − t₀)/10⁶` секунд и `r(x) = Δx/d`. Отдельное `x` — записанный gauge или накопленное значение в выбранном sample. `B` — положительное целое `pg_settings.block_size` в байтах из snapshot у cursor. Buffer columns показывают `B × r(blocks)` bytes/s; Buffer bytes/call показывает `B × blocks_per_call`. Без записанного `B` перевод в байты недоступен.

Интервальные вычисления Statements/Plans требуют одинакового физического типа и identity, положительного времени между samples и неубывающих operands. Отсутствующий operand или уменьшившийся counter делает недоступной эту пару значений; последующие пары вычисляются обычным образом. Для отношения требуется положительный знаменатель. Записанные `min`, `max`, `mean`, `stddev`, timestamps и ages — gauges, в том числе при уменьшении соседних накопленных counters. У Relations и Overview действуют отдельные правила null, описанные ниже. Источники: [интервальные вычисления](../bins/kronika-web/ui/src/postgres-metrics.ts), [перевод в байты и выбор columns](../bins/kronika-web/ui/src/postgres-view.tsx).

| Stream | Identity объекта в записи |
|---|---|
| Activity, Locks | Числовой `pid` |
| Database | `datid` |
| Statement | `userid, dbid, queryid`; layouts с `toplevel` добавляют его в identity |
| Plan | `userid, dbid, queryid, planid` в одном физическом layout extension |
| Table / Index | `datid, relid` / `datid, indexrelid` |
| I/O | `backend_type, object, context` |
| Settings | `datid, usesysid, name` |

## Activity и Locks

По умолчанию таблица Activity скрывает `state = idle` и backend types, отличные от `client backend`. **Idle** и **System** включают эти строки; явно открытая строка остаётся видимой. Сортировка по умолчанию — по убыванию query duration; если query duration недоступна у обеих строк, сравнивается transaction duration. Источники: [columns и filters Activity](../bins/kronika-web/ui/src/postgres-view.tsx), [функции duration](../bins/kronika-web/ui/src/postgres-activity.ts).

| Показатель / поле | Определение и единица |
|---|---|
| Query duration / `query_duration_ms` | `(t − query_start)/1000` ms, только при `state = active` |
| Transaction duration / `transaction_duration_ms` | `(t − xact_start)/1000` ms для любого state с доступным началом transaction |
| State duration / `state_duration_ms` | `(t − state_change)/1000` ms, скрыта при `state = idle` |
| Backend age / `backend_age_ms` | `(t − backend_start)/1000` ms |
| PID, leader PID | OS PID backend и записанный PID leader parallel group |
| Database, role, application, client | `datname`, `usename`, `application_name`, `client_addr` из `pg_stat_activity` |
| State, wait event type, wait event | Записанные строки сервера; wait event может присутствовать при `state = active` |
| Query, query ID | Записанный SQL и `query_id`; при сборе query text ограничен 65 536 символами |
| XID / xmin age | Серверные `age(backend_xid)` / `age(backend_xmin)`, в transaction IDs |

В каждой формуле duration `t` — timestamp записанной строки. Null, неположительный или будущий timestamp начала даёт null. SQL неактивной строки остаётся её последним записанным query; активная query duration у такой строки отсутствует.

Locks записывает одну строку на backend, участвующий в blocking component. Collector выбирает одну детерминированную ungranted-строку `pg_locks` для каждого waiter и записывает отсортированный список различных `pg_blocking_pids(pid)`. Исключаются PID collector и backends с `application_name`, равным имени его session. `blocked_by = 0` обозначает prepared transaction. Источник: [SQL Locks](../crates/kronika-source-pg/src/locks.rs).

| Значение / control Locks | Значение |
|---|---|
| PID tree | Родительский blocker перед дочерним waiter; дополнительные blockers показаны как `+PID`; отдельные components расположены вместе |
| Prepared transaction | У `0` есть подпись, но нет синтетической backend row или ребра дерева |
| Количество строк | Количество показанных backend rows, включая blockers; это не число удерживаемых locks |
| `lock_locktype`, `lock_mode` | Тип и запрошенный mode выбранного ungranted lock |
| `lock_target` | Разрешённое имя relation, transaction/virtual-XID, object identifiers или lock type |
| `lock_database`, `lock_relation`, `lock_page`, `lock_tuple` | OIDs database/relation и необязательные координаты page/tuple |
| `lock_classid`, `lock_objid`, `lock_objsubid` | Координаты catalog object для object locks |
| `waitstart` | Серверный timestamp начала lock wait, PostgreSQL 14+; таблица показывает timestamp |
| Backend context | Database, role, application, client, state, wait event и query из того же SQL Locks |
| Search | У подходящих строк сохраняются ancestors-blockers и дополнительный blocker context |

Имя relation разрешается только в database соединения collector. У корневых blockers могут отсутствовать поля ungranted lock. Inspector Locks содержит backend context и `blocked_by`; прямой навигации к related entities из PID и relation cells нет. Источники: [построение и поиск по дереву](../bins/kronika-web/ui/src/postgres-locks.ts), [Locks view](../bins/kronika-web/ui/src/postgres-view.tsx).

## Overview и Databases

Левое число Overview обобщает загруженный час; правое соответствует значению chart у cursor. Rates складываются по точному timestamp позднего sample между identities. Сумма за час складывается из каждого доступного counter difference. Reducer пропускает недоступную пару поля, сохраняя остальные доступные пары, и возвращает null при отсутствии всех пар. Gauge sums/maxima пропускают null; snapshot из одних null остаётся null. Источники: [определения Overview](../bins/kronika-web/ui/src/postgres-overview.tsx), [reducers](../bins/kronika-web/ui/src/postgres-vitals.ts).

В таблице `Σ` складывает databases, если не указана другая область; **total** — сумма differences за час, **peak** — максимум chart points, **last** — последний nonnull chart point.

| Строка Overview | Chart / значение у cursor | Левое число |
|---|---|---|
| Client backends | Число `backend_type = client backend` в Activity; отсутствующий backend type заменяется на `client backend` | Peak; `/ max_connections`, если записан |
| Active vs waiting | Активные client backends без / с `wait_event_type`; parallel followers исключены | Peak running count |
| Idle in transaction | Число states, начинающихся с `idle in transaction`, во всей Activity | Peak |
| Oldest transaction | Максимум `(t − xact_start)/10⁶` s среди client backends без parallel followers; минимум 0 | Peak duration |
| Oldest xmin age | Максимум записанного `backend_xmin_age` во всей Activity | Peak, transaction IDs |
| Prepared transactions | Сумма `prepared_count`; age — максимум `max_age_us` | Peak count и peak age |
| Transactions | `Σ[r(xact_commit)+r(xact_rollback)]`; второй ряд `Σr(xact_rollback)` | Total transactions и `100 × total rollbacks / total transactions` |
| Tuples read | `Σr(tup_returned)`; второй ряд `Σr(tup_fetched)` | Total returned tuples |
| Tuples written | `Σ[r(tup_inserted)+r(tup_updated)+r(tup_deleted)]` | Total changed tuples |
| Buffer hit share | `100Σr(blks_hit)/Σ[r(blks_hit)+r(blks_read)]`, % | То же отношение по differences за час |
| Block I/O time | `Σ[r(blk_read_time)+r(blk_write_time)]`, ms/s | Total ms |
| Temp bytes | `Σr(temp_bytes)`, bytes/s | Total bytes |
| Deadlocks / Checksum failures | `Σr(deadlocks)` / `Σr(checksum_failures)`, events/s | Total events |
| Abnormal session ends | `Σ[r(sessions_fatal)+r(sessions_killed)]`, sessions/s | Total sessions |
| WAL generated | `r(pg_stat_wal.wal_bytes)`, bytes/s | Total bytes |
| Checkpoints | Rates timed / requested checkpoints | Total checkpoints и requested count |
| Checkpoint buffer writes | `r(buffers_checkpoint)`; fallback PostgreSQL 17+ — `r(pg_stat_checkpointer.buffers_written)`; второй ряд `r(buffers_backend)`, если доступен, blocks/s | Total checkpoint blocks |
| WAL archiver | `r(archived_count)` / `r(failed_count)`, events/s | Оба totals |
| WAL buffers full | `r(wal_buffers_full)`, events/s | Total events |
| pg_wal size | Записанное `wal_files_bytes`, bytes | Last |
| Buffer evictions / Buffer reuses | Сумма `r(evictions)` / `r(reuses)` по identities `pg_stat_io`, operations/s | Total operations |
| Relation extends / Fsyncs | Сумма `r(extends)` / `r(fsyncs)` по identities I/O, operations/s | Total operations |
| Vacuum reads | Сумма `r(reads)` при `pg_stat_io.context = vacuum`, operations/s | Total operations |
| Transaction ID age | Максимум database `frozen_xid_age`, transaction IDs | Last; процент от записанного `autovacuum_freeze_max_age` |
| Multixact age | Максимум database `min_mxid_age`, multixact IDs | Last |
| Autovacuum workers | Количество progress rows с `is_autovacuum = true` | Peak; `/ autovacuum_max_workers`, если записан |

Для timed/requested checkpoints сначала используются `pg_stat_checkpointer.num_timed/num_requested`, fallback — `pg_stat_bgwriter.checkpoints_timed/checkpoints_req`. Prepared/progress rows относятся к открытому интервалу между соседними timestamps database snapshots; пустой интервал даёт нулевое число prepared/workers и null prepared age. Пунктирные limits Overview используют последнее записанное setting загруженного часа. Passport разрешает settings у cursor, используя первое setting часа, если cursor стоит раньше него. Series Overview текущего часа обновляются не чаще раза в минуту. [Исходник activity lanes](../crates/kronika-query/src/hour/lanes.rs) задаёт отбор client/follower.

Таблица **Databases** показывает `numbackends` как gauge; counters transactions/sessions/tuples/buffers/temp/conflicts/deadlocks — как rates соседних samples; `blk_read_time` и `blk_write_time` — в ms/s; `temp_bytes` — в bytes/s; `frozen_xid_age = age(pg_database.datfrozenxid)` — как gauge. `blks_read` и `blks_hit` переводятся в bytes/s через `B`. `tup_returned` считает tuples, возвращённые scans; `tup_fetched` — live rows, полученные index scans. `conflicts` считает queries, отменённые из-за recovery conflicts. `datid = 0` — строка статистики shared objects; она исключена из показываемого количества databases. Источники: [SQL и поля database](../crates/kronika-source-pg/src/database.rs), [columns таблицы](../bins/kronika-web/ui/src/postgres-view.tsx).

## Statements и Plans

### Интервальные метрики

Название **Execution** соответствует lens token `load`. У Statements также есть **Per call**, **I/O**, **Resources**, **Stability**. У Plans — **Execution**, **Timing**, **I/O**, **Identifiers**. Общие identity columns — database, role и query/plan IDs; первой колонкой идёт SQL или plan summary. Источники: [поля lenses и вычисления](../bins/kronika-web/ui/src/postgres-metrics.ts), [display columns](../bins/kronika-web/ui/src/postgres-view.tsx).

Обозначения: `E = total_exec_time` для statement layouts 1.8+, `E = total_time` для statement 1.5–1.7 и всех plan layouts; `P = total_plan_time`; `C = calls`; `H = shared_blks_hit`; `R = shared_blks_read`; `LH = local_blks_hit`; `LR = local_blks_read`.

| Показатель / derived field | Формула / записанное значение | Единица; lens |
|---|---|---|
| Calls/s | `r(C)` | calls/s; Execution, Per call, Resources, Stability, Timing, Identifiers |
| Exec time/s | `r(E)` | ms/s; Execution, Resources; одновременные executions складываются, поэтому значение может превышать 1000 |
| Mean/call | `ΔE/ΔC` | ms/call; Execution, Per call |
| Rows/s | `r(rows)` | returned/affected rows/s; Execution |
| Rows/call | `Δrows/ΔC` | rows/call; Per call |
| Buffer bytes/call | `B × [r(H)+r(R)+r(LH)+r(LR)]/r(C)` | bytes/call; Per call, I/O |
| Cache hit ratio / `hit_pct` | `100r(H)/[r(H)+r(R)]` | %; I/O; только shared buffers |
| Shared/local buffer hits, reads, dirtied, written | `B × r(shared_blks_hit/read/dirtied/written)` и соответствующие `local_blks_*` | bytes/s; I/O и записанные поля |
| Temp reads / writes | `B × r(temp_blks_read/written)` | bytes/s; I/O, Resources |
| WAL bytes | `r(wal_bytes)` | bytes/s; Resources |
| WAL/call | `Δwal_bytes/ΔC` | bytes/call; Resources |
| Planning time/s | `r(P)` | ms/s; Resources |
| Plan time, % | `100r(P)/[r(P)+r(E)]` | %; Resources |
| WAL records / FPI / buffers full | `r(wal_records/wal_fpi/wal_buffers_full)` | records/full-page images/full-buffer events в секунду; записанные поля |
| Plans | `r(plans)` | planning operations/s; записанное statement field |
| Shared/local/temp read/write time | `r(physical timing field)` | ms/s; записанные поля |
| Mean / Min / Max / Stddev | Записанные execution-time statistics за statistics period extension | ms; Stability, Timing |
| CV | Записанное `stddev / mean` | Безразмерное; Stability; null при неположительном mean |
| Calls | Записанное накопленное целое `calls` | calls; Plans → Identifiers |
| Slow log calls | `r(slow_log_calls)` в vadv layout | calls/s; записанное plan field |
| First call / Last call / Stats since | Записанные `first_call`, `last_call`, `stats_since`, если layout их определяет | Абсолютное время |

`Mean/call` использует interval totals; **Mean** использует записанный gauge extension. Buffer bytes/call в таблице складывает доступные finite block rates и требует хотя бы один; его history calculation требует все запрошенные block rates. Остальные derived ratios требуют каждый перечисленный operand. Перевод в байты не изменяет отношения buffer counts.

| Layout | Поля execution / planning time | Поля block timing |
|---|---|---|
| Statements 1.5–1.7 | `total_time`; planning отсутствует | `blk_read_time`, `blk_write_time` соответствуют shared timing |
| Statements 1.8–1.9 | `total_exec_time`, `total_plan_time` | Те же shared aliases |
| Statements 1.10 | Те же | Shared aliases плюс `temp_blk_read_time`, `temp_blk_write_time` |
| Statements 1.11+ | Те же | `shared_blk_*_time`, `local_blk_*_time`, `temp_blk_*_time` |
| OSSC / Datasentinel plans | `total_time`; planning отсутствует | Раздельные shared/local/temp timing fields |
| vadv plans | `total_time`, `total_plan_time` | `blk_read_time`, `blk_write_time` соответствуют shared timing |

Statement timing gauges называются `min_time/max_time/mean_time/stddev_time` в 1.5–1.7 и `min_exec_time/max_exec_time/mean_exec_time/stddev_exec_time` в следующих layouts. Plans используют первые имена. Текущие lenses и их inspectors выбирают подмножества из таблиц lenses; остальные записанные поля сохраняют определения источника. Отсутствующие в layout поля скрыты или недоступны. Точные поля: [statement registry](../crates/kronika-registry/src/codec/pg_stat_statements.rs), [plan registry](../crates/kronika-registry/src/codec/pg_store_plans.rs).

### Plan identity, текст и related controls

В OSSC и Datasentinel `queryid` — statement query ID. В vadv `queryid` — внутренний ID extension и часть identity из четырёх полей; `queryid_stat_statements` — последняя записанная привязка к statement, которая может меняться отдельно. Нулевая привязка не создаёт statement link. Datasentinel дополнительно записывает `relids` (список relation OIDs) и `cmd_type` (command type). Plan text получается через интерфейс читаемого текста extension и ограничивается 65 536 символами.

**Query ID** переводит из Statement в Plans с predicates database, role и query ID. Plans → Statements использует те же predicates с соответствующей statement attribution. **Plan ID** фильтрует по plan ID. Activity → Statements требует ненулевые `datid` и `query_id` и database name; predicates — database/query ID. Поиск SQL text в plan inspector использует только query ID. Источники: [navigation predicates](../bins/kronika-web/ui/src/statement-navigation.ts), [поиск SQL text плана](../bins/kronika-web/ui/src/plan-query.ts), [интерфейсы сбора](../crates/kronika-source-pg/src/store_plans.rs).

**Kronika queries** включает собственные statements collector. По умолчанию scope `workload` их исключает. Непустой search, statement context или явно выбранный collector statement принудительно включает `all` и блокирует checkbox. Этот scope применяется также к запросам statement activity и summary. Источники: [scope resolver](../bins/kronika-web/ui/src/postgres-view.tsx), [определение collector statements](../crates/kronika-query/src/statement_scope.rs).

## Tables и Indexes

### Группировка, storage и null

**Databases → Schemas → Objects** применяет database OID, затем predicates database/schema. **Tablespaces → Objects** использует effective tablespace OID уровня instance. У index собственное placement. `reltablespace = 0` разрешается в `dattablespace` database. У partitioned table parents без storage placement равен null; в tablespace groups они не входят. **Table indexes** и **Table** используют точные `datid, relid`. **Index definition** загружает записанный `indexdef` object row. Источники: [сбор tables](../crates/kronika-source-pg/src/user_tables.rs), [сбор indexes](../crates/kronika-source-pg/src/user_indexes.rs), [навигация](../bins/kronika-web/ui/src/postgres-relations.ts).

Группа складывает rates объектов после деления каждой difference на её собственное elapsed time: `R(x) = Σᵢ Δxᵢ/dᵢ`. Gauge totals — `G(x) = Σᵢ xᵢ`. Group ratios используют эти суммы числителей и знаменателей. XID/MXID ages используют максимум. Timestamp groups сохраняют oldest, latest и число записанных null timestamps (**Never…**). Table без TOAST не добавляет TOAST timestamp или Never-TOAST count.

Отсутствующее layout field или недоступная object pair делает соответствующий grouped metric равным null. Два явных null у table index/TOAST counters означают структурную неприменимость и не добавляют значение. Null TOAST gauges нейтральны при null `toast_bytes`. Составные throughput/buffer/storage metrics допускают эти структурные исключения; отдельный component без применимых значений остаётся null. `reltuples = -1` недоступен. Источник: [relation reducer, `Aggregate::metric`, `counter_input`, `gauge_input`](../crates/kronika-query/src/hour/relation.rs).

| Storage field | Точный источник; bytes |
|---|---|
| Table data / `main_fork_bytes` | `pg_relation_size(table_oid)`: heap main fork |
| TOAST / `toast_bytes` | `pg_total_relation_size(reltoastrelid)`: TOAST relation с auxiliary forks и indexes; null без TOAST |
| Table + TOAST / `displayed_storage_bytes` | Heap main fork + TOAST; user indexes и heap FSM/VM forks не входят |
| Index data / `main_fork_bytes` | `pg_relation_size(index_oid)`: index main fork |

### Table lenses

Для одного объекта `R = r`, `G(x) = x`; для groups применяются определения выше. Пусть `D = R(n_tup_ins)+R(n_tup_upd)+R(n_tup_del)`.

| Lens: показатель / поле | Формула, единица и значение |
|---|---|
| Access: Tuples scanned | `R(seq_tup_read)+R(idx_tup_fetch)`, tuples/s |
| Seq scans / Index scans | `R(seq_scan)` / `R(idx_scan)`, scans/s |
| Seq scans, % | `100R(seq_scan)/[R(seq_scan)+R(idx_scan)]` |
| Tuples per seq/index scan | `R(seq_tup_read)/R(seq_scan)` / `R(idx_tup_fetch)/R(idx_scan)` |
| Last seq/index scan | Записанные timestamps, PostgreSQL 16+; groups содержат oldest/latest/Never counts |
| Changes: DML total | `D`, inserted + updated + deleted tuples/s |
| Inserts / Updates / Deletes, % | `100R(n_tup_ins)/D`, `100R(n_tup_upd)/D`, `100R(n_tup_del)/D` |
| HOT updates, % | `100R(n_tup_hot_upd)/R(n_tup_upd)` |
| New-page updates, % | `100R(n_tup_newpage_upd)/R(n_tup_upd)`, PostgreSQL 16+ |
| Dead tuples, % | `100G(n_dead_tup)/[G(n_live_tup)+G(n_dead_tup)]`, оценки snapshot |
| Modified since analyze | `G(n_mod_since_analyze)`, оценка changed tuples после последнего analyze |
| Inserted since vacuum | `G(n_ins_since_vacuum)`, оценка inserted tuples после последнего vacuum, PostgreSQL 13+ |
| Maintenance: Manual vacuums / Autovacuums / Manual analyzes / Autoanalyzes | `R(vacuum_count/autovacuum_count/analyze_count/autoanalyze_count)`, operations/s |
| Mean manual vacuum / autovacuum / manual analyze / autoanalyze | `R(total_vacuum_time)/R(vacuum_count)` и соответствующие пары time/count, ms/operation; PostgreSQL 18+ |
| Last manual vacuum / autovacuum / manual analyze / autoanalyze / TOAST autovacuum | Записанные timestamps; groups содержат oldest/latest/Never counts |
| Size and buffers: Table + TOAST / Table data | Соответствующие суммы storage gauges, bytes |
| TOAST, % | `100G(toast_bytes)/[G(main_fork_bytes)+G(toast_bytes)]` |
| Estimated rows | `G(reltuples)`, `pg_class.reltuples`, приведённый к целому при сборе |
| TOAST dead tuples, % | `100G(toast_n_dead_tup)/[G(toast_n_live_tup)+G(toast_n_dead_tup)]` |
| Heap / Index / TOAST / TOAST-index buffer hit share | Для prefix `q = heap, idx, toast, tidx`: `100R(q_blks_hit)/[R(q_blks_hit)+R(q_blks_read)]` |
| Buffer hit share | Та же формула с суммами по четырём prefixes |
| Buffer read/hit bytes | `B × R(q_blks_read/hit)`, bytes/s; соответствующие четыре storage components |
| Freeze: XID age / MXID age | Максимум `age(pg_class.relfrozenxid)` / `mxid_age(pg_class.relminmxid)`; transaction/multixact IDs |
| Freeze: Inserted since vacuum; last vacuum/autovacuum | Gauges/timestamps, определённые выше |

### Index lenses

| Lens: показатель / поле | Формула, единица и значение |
|---|---|
| Usage: Index scans | `R(idx_scan)`, scans/s |
| Index tuples read / Tuples fetched | `R(idx_tup_read)` / `R(idx_tup_fetch)`, index entries / live heap tuples в секунду |
| Tuples per scan / Fetches per scan | `R(idx_tup_read)/R(idx_scan)` / `R(idx_tup_fetch)/R(idx_scan)` |
| Last index scan | Записанный timestamp, PostgreSQL 16+; oldest/latest/Never counts в groups |
| Low activity | Применяет `no_scans = true`: доступное `Δidx_scan = 0` в текущей calculation pair |
| No scans / No-scan count / Known-scan count | Object boolean; group count с `Δidx_scan = 0`; group count с `Δidx_scan > 0` |
| Size and buffers: Index data | `G(main_fork_bytes)`, bytes |
| Buffer hit share | `100R(idx_blks_hit)/[R(idx_blks_hit)+R(idx_blks_read)]` |
| State: valid / ready / unique / primary / exclusion | Booleans `pg_index.indisvalid/indisready/indisunique/indisprimary/indisexclusion` |
| State group counts | Число false valid/ready или true unique/primary/exclusion flags |
| State order / `state_severity` | `2` invalid; `1` valid, но unready; `0` valid и ready; максимум группы |
| Access method | `pg_am.amname`; Index definition — записанный `pg_get_indexdef(index_oid)` |

Low activity относится к текущему интервалу. Нулевой rate не означает, что cumulative counter или предыдущие rates этого index равны нулю. Источники обеих таблиц lenses: [relation definitions и histories](../bins/kronika-web/ui/src/postgres-relations.ts), [query reducer](../crates/kronika-query/src/hour/relation.rs).

## Vacuum progress

Источник — `pg_stat_progress_vacuum` с join к Activity для `is_autovacuum` и к catalogs соединения для relation names. Строка определяется `pid, datid, relid`; физические layouts соответствуют PostgreSQL 10–16, 17 и 18. Источники: [сбор](../crates/kronika-source-pg/src/progress_vacuum.rs), [вычисления episodes](../bins/kronika-web/ui/src/postgres-vacuum.ts), [отображение](../bins/kronika-web/ui/src/postgres-view.tsx).

| Поле / показатель | Определение |
|---|---|
| Kind | `is_autovacuum = (backend_type = autovacuum worker)`; false, если Activity join не нашёл строку |
| Phase | Записанная строка phase |
| Scan progress | `clamp(100 × heap_blks_scanned / heap_blks_total, 0, 100)`; null при неположительном/отсутствующем total |
| Heap scanned / total | `B × heap_blks_scanned` / `B × heap_blks_total`, bytes; total — heap blocks на начало scan |
| Heap vacuumed | `B × heap_blks_vacuumed`, bytes |
| Index cycles | Записанный `index_vacuum_count`; значения >1 получают подпись повторного cycle |
| Index progress | `indexes_processed / indexes_total`, PostgreSQL 17+, показывается только в phases vacuuming/cleaning-up indexes |
| Dead tuple capacity/use, PostgreSQL 10–16 | `max_dead_tuples`, `num_dead_tuples`, tuple counts |
| TID store, PostgreSQL 17+ | `max_dead_tuple_bytes`, `dead_tuple_bytes`, bytes; `num_dead_item_ids`, count |
| Cost delay, PostgreSQL 18 | Накопленный `delay_time`, ms; `+Δdelay_time` по двум последним samples episode при неубывающем counter |
| In phase | Количество последних samples с одинаковыми phase и index cycle; span между первым и последним timestamp этой серии |
| No movement | Не менее 3 последних samples с неизменным designated counter в этой phase/cycle; показывает count и записанный span |

Episode key — физический тип + PID + database OID + relation OID. Новый episode начинается при уменьшении `index_vacuum_count`, `heap_blks_scanned` или `heap_blks_vacuumed`, либо когда соседние samples отстоят больше чем на `2.5 × recorded postgresql_interval_seconds`. Без положительного записанного cadence действуют только identity/counter rules. Строка — последний sample episode. **At cursor** означает, что последний timestamp episode равен последнему progress timestamp не позже cursor; у остальных episodes показано их собственное последнее записанное время.

| Phase | Фиксированная отметка phase | Operand No movement |
|---|---|---|
| `initializing`, `performing final cleanup` | Ordinary | Нет |
| `scanning heap` | Ordinary | `heap_blks_scanned` |
| `vacuuming heap` | Heavy | `heap_blks_vacuumed` |
| `vacuuming indexes`, `cleaning up indexes` | Heavy | `indexes_processed`, PostgreSQL 17+ |
| `truncating heap` | Dangerous | Повторные samples phase; phase устанавливается до условной попытки exclusive lock |

Process load разрешает OS samples этого PID не позже первого и последнего timestamps episode. При записанном clock rate `H` ticks/s, CPU ms = `1000Δ(utime+stime)/H`; CPU share = `min(100,100 × CPU seconds / OS sample elapsed seconds)`; block-wait ms = `1000Δblkdelay_ticks/H`. Read/write bytes и major faults равны `Δread_bytes`, `Δwrite_bytes`, `Δmajflt`. Read share = `min(100,100Δread_bytes/(B × final heap_blks_scanned))`. Эти deltas включают всю работу PID между выбранными OS samples. Отсутствующий clock/block size скрывает только зависящее от него вычисление.

## Summary strips и цветовые отметки

Summary strips разрешают snapshot всей surface не позже cursor, независимо от pagination и text search таблицы. Summary Statements и Plans используют выбранный statement scope. Каждое отношение включает только объекты с необходимой operand pair; числители и знаменатели накапливаются вместе. Здесь `ΣΔ` — допущенные differences в выбранном summary snapshot, а не сумма за час. Источники: [summary stream](../crates/kronika-query/src/hour/postgres_summary.rs), [реализация формул](../crates/kronika-query/src/hour/postgres_summary/facts.rs).

| Summary | Формула |
|---|---|
| Active statements / Used plans | Число доступных `Δcalls > 0`; процент от объектов с доступным `Δcalls` |
| Execution per call | `ΣΔE / ΣΔcalls` |
| WAL per call | `ΣΔwal_bytes / ΣΔcalls` |
| Executions per used plan | `ΣΔcalls / count(Δcalls > 0)` |
| Reads outside buffers | `100ΣΔread / ΣΔ(read+hit)`; Statements/Plans: shared+local; Databases: `blks_*`; Tables: heap+index+TOAST+TOAST-index; Indexes: только index |
| Rollbacks / Temp per transaction | `100ΣΔxact_rollback / ΣΔ(xact_commit+xact_rollback)`; `ΣΔtemp_bytes / ΣΔ(xact_commit+xact_rollback)` |
| Scan methods | `100ΣΔseq_scan / ΣΔ(seq_scan+idx_scan)`; index share дополняет до 100% |
| HOT updates / Dead rows | `100ΣΔn_tup_hot_upd/ΣΔn_tup_upd`; `100Σn_dead_tup/Σ(n_live_tup+n_dead_tup)` |
| Vacuumed tables | `100 × count[Δ(vacuum_count+autovacuum_count)>0] / count[usable Δ(vacuum_count+autovacuum_count)]` |
| Storage | `100Σtoast_bytes/Σ(main_fork_bytes+toast_bytes)`; main share дополняет до 100% |
| XID boundary | `100 × count(xid_age ≥ 1,600,000,000) / count(usable xid_age)` |
| Scanned indexes / Without scans | Процент положительных / нулевых среди доступных `Δidx_scan` |
| Usable indexes | `100 × count(indisvalid AND indisready) / count(objects with both flags)` |

Цвета значений — фиксированные сравнения из [value-tone.ts](../bins/kronika-web/ui/src/value-tone.ts). Цвета query и transaction duration применяются только к active client backends. Нулевые rates получают inactive color до этих сравнений.

| Значение | Good | Warning | Critical |
|---|---|---|---|
| Query duration | — | 1000–<5000 ms | ≥5000 ms |
| Transaction duration | — | 5000–<60000 ms | ≥60000 ms |
| Mean/call, Mean | — | — | ≥5000 ms |
| Cache hit ratio | ≥99% | 90–<99% | <90% |
| CV | <1 | 1–<3 | ≥3 |
| Plan time | <50% | 50–<80% | ≥80% |
| State | — | `idle in transaction` | `idle in transaction (aborted)` |
| Wait event type | — | Любое непустое значение | — |

## Settings, reset timestamps и WAL storage

`pg_settings` записывает effective configuration metric session collector с identity database и login role. Запись создаётся при первом успешном чтении, изменении и каждом новом segment. `primary_conninfo` и `ssl_passphrase_command` исключены. Passport показывает server version, max connections, shared buffers, max WAL size, checkpoint timeout, autovacuum и I/O-timing settings. Список изменений сравнивает последовательные записанные строки `setting` по setting name. Источники: [сбор settings](../crates/kronika-source-pg/src/settings.rs), [поиск passport](../bins/kronika-web/ui/src/postgres-vitals.ts).

Поля `stats_reset` в database, WAL, I/O, writer/checkpointer, archiver и extension-info sections — серверные timestamps сброса. `stats_since` — timestamp начала статистики statement row, если layout его содержит; `first_call`/`last_call` задают границы calls записанного плана. Это абсолютные записанные timestamps; текущий интерфейс не вычисляет reset-age value.

`pg_wal_storage.wal_files_bytes = COALESCE(SUM(size), 0)` по regular files, возвращённым `pg_ls_waldir()` на `statement_timestamp()` этого query. Это текущий размер файлов directory в байтах. Файлы включаются по возвращённым размерам без классификации по имени. Collector не обходит subdirectories; при отказе в permission section отсутствует. Источник: [WAL storage query](../crates/kronika-source-pg/src/wal_storage.rs).

### Правила maintenance PostgreSQL

Эти формулы описывают scheduling сервера PostgreSQL. Kronika записывает table statistics и metric-session settings выше; table `reloptions`, `relpages` и `relallfrozen` не собираются, table eligibility verdict не вычисляется.

Пусть `N = max(reltuples,0)`, `D = n_dead_tup`, `I = n_ins_since_vacuum`, `M = n_mod_since_analyze`. `V₀/Vₛ`, `I₀/Iₛ`, `A₀/Aₛ` — effective `autovacuum_vacuum_threshold/scale_factor`, `autovacuum_vacuum_insert_threshold/scale_factor`, `autovacuum_analyze_threshold/scale_factor`. Table storage parameter переопределяет соответствующее global value.

| Правило сервера | Формула |
|---|---|
| Dead-tuple vacuum | `D > V₀ + VₛN`; PostgreSQL 18 ограничивает правую часть значением `autovacuum_vacuum_max_threshold`, если оно неотрицательно |
| Insert vacuum, PostgreSQL 13–17 | `I > I₀ + IₛN`; отключён при `I₀ = -1` |
| Insert vacuum, PostgreSQL 18 | `I > I₀ + IₛNf`, где `f = 1 − min(relallfrozen,relpages)/relpages` при положительных обоих inputs, иначе `f = 1`; отключён при `I₀ = -1` |
| Analyze | `M > A₀ + AₛN` |
| XID / MXID forced vacuum | `age(relfrozenxid) > Xmax` или `mxid_age(relminmxid) > MXmax` для valid IDs с серверными сравнениями, учитывающими wraparound |

`Xmax` — меньшее из заданного table freeze-max-age и global maximum; без table override используется global maximum. `MXmax` аналогично ограничивает table value effective multixact maximum сервера, который может быть ниже configured maximum. Точная реализация thresholds: [PostgreSQL 17](https://github.com/postgres/postgres/blob/REL_17_STABLE/src/backend/postmaster/autovacuum.c), [PostgreSQL 18](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/postmaster/autovacuum.c), `relation_needs_vacanalyze`.

Первое правило insert появилось в PostgreSQL 13. Определения по версиям: [PostgreSQL 13](https://www.postgresql.org/docs/13/routine-vacuuming.html#AUTOVACUUM), [PostgreSQL 17](https://www.postgresql.org/docs/17/routine-vacuuming.html#AUTOVACUUM), [PostgreSQL 18](https://www.postgresql.org/docs/18/routine-vacuuming.html#AUTOVACUUM). Forced age work действует и при отключённом обычном autovacuum. Freezing продвигает `relfrozenxid`/`relminmxid`; `VACUUM` освобождает tuple space для повторного использования и поддерживает visibility, а `ANALYZE` обновляет planner statistics. Эти серверные правила отделены от записанных configured limits Overview и фиксированной отметки summary в 1,6 миллиарда XIDs.
