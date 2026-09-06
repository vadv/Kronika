# Время записи, расчёты и heatmaps

[English version](metrics-time.md) · [Указатель справочника](features.ru.md) · [Linux](metrics-linux.ru.md)

Записанные `ts` — Unix microseconds. Пусть `t₀` и `t₁` — фактические timestamps двух samples, `Δt = (t₁ − t₀) / 1,000,000` секунд, `Δx = x(t₁) − x(t₀)`, `R(x) = Δx / Δt`. Gauge использует записанное значение выбранного sample. Counter rate использует пару, которую выбирает соответствующий путь запроса.

## Время и выбор данных

| Элемент управления или значение | Определение |
|---|---|
| Час в календаре | Выбирает записанный интервал `[h, h + 3,600,000,000)` в microseconds. Календарь и часы отображаются в выбранном часовом поясе. |
| Cursor | Запрошенная позиция внутри часа. Таблица выбирает последний подходящий snapshot не позже cursor. Sections с разным cadence могут выбрать разные timestamps. PostgreSQL sections, разделённые по database, выбирают snapshot каждой database независимо. |
| Время sample | Фактический timestamp выбранных данных. Общая подпись времени показывает разрешённое время sample; cursor между samples не создаёт нового sample. |
| Предыдущий/следующий sample | Переход по объединённому, отсортированному списку уникальных timestamps наблюдений текущего экрана. Выбранная timeline lane не задаёт cadence навигации. |
| Hover/readout графика | Читает записанные точки графика. Выбор точки меняет общий cursor. Null остаётся null и завершает соответствующий участок линии. |
| Нажатие ячейки heatmap | Устанавливает cursor в exclusive upper boundary ячейки минус одна microsecond; затем таблица выбирает snapshot. |
| Строка объекта | Выбирает identity и её inspector. Кнопка метрики истории выбирает поле графика этой identity; lens задаёт доступные поля. |
| Live refresh | Видимый текущий час обновляется каждые 15 секунд. Скрытие документа останавливает таймер; возвращение видимости обновляет текущий час. Cursor, следующий за последней точкой, продвигается; выбранный вручную остаётся на месте. Завершённые часы не обновляются периодически. |

Источники: [выбор snapshot](../crates/kronika-query/src/snapshot/mod.rs), [surface selector](../crates/kronika-query/src/snapshot/selector.rs), [timestamps cursor](../bins/kronika-web/ui/src/cursor-timestamps.ts), [refresh](../bins/kronika-web/ui/src/refresh.ts), [cursor heatmap](../bins/kronika-web/ui/src/activity.tsx).

Интервалы collector задаются в секундах. Нулевой интервал источника делает его due при каждом timer wake; сам по себе он не приближает wake. `KRONIKA_INTERVAL_S` — максимальное время ожидания collection timer, default 5 секунд; положительные source deadlines или segment age могут его сократить. `KRONIKA_INTERVAL_S=0` отключает timed collection; `SIGUSR2` принудительно делает due все источники. Rotation сохраняет отдельный таймер. Знаменатель отображаемого rate — фактическое записанное время между samples.

| Источник | Переменная окружения | Default, s |
|---|---|---:|
| Основные Linux counters | `KRONIKA_OS_CORE_INTERVAL_S` | 10 |
| Processes | `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 |
| Process status | `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 |
| Mounts и topology | `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 |
| Cgroup controllers | `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 |
| PID-to-cgroup mapping | `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 |
| Logs | `KRONIKA_LOG_INTERVAL_S` | 10 |
| PostgreSQL | `KRONIKA_PG_INTERVAL_S` | 30 |
| PostgreSQL relations | `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 |

Источники: [defaults scheduler](../bins/kronika-collector/src/scheduler.rs), [конфигурация](../bins/kronika-collector/src/config.rs), [`timer_sleep_delay`](../bins/kronika-collector/src/main.rs).

## Правила пар и единицы

| Путь расчёта | Пара и недоступный результат |
|---|---|
| Process snapshot rates | Тот же числовой PID в предыдущем process snapshot с равным записанным `starttime`. Отсутствие predecessor, изменение `starttime`, отсутствующее optional value, уменьшение counter или неположительный `Δt` дают null для соответствующего rate. Равные counters дают ноль. |
| Process summary | Сумма пригодных rates отдельных процессов на каждом process snapshot. Gauges суммируют присутствующие значения. Метрика без слагаемых равна null; counts процессов/runnable/PostgreSQL могут быть нулевыми. |
| Process inspector history | Соседние записанные наблюдения выбранных PID и поля. Явный null очищает predecessor; отрицательная разность или неположительный интервал дают null. Этот расчёт истории не проверяет `starttime`. |
| Host/container rate lanes | Соседние элементы counter series соответствующей lane. Отрицательная разность даёт null для пары. Явный null в nullable series очищает predecessor. |
| Device/cgroup derived history | Целочисленное вычитание выполняется до преобразования в floating point. Отсутствующие operands, отрицательные разности компонентов или неположительный интервал дают null. Latency дополнительно требует положительной разности числа операций. |
| Heatmap counter summary | Последний counter минус первый для identity внутри запрошенного диапазона. Нужны два timestamps и неотрицательная разность крайних значений. Расчёт не суммирует только положительные соседние разности. |
| Health | Использует собственные правила, scope и ограничение возраста PostgreSQL sample из раздела Health ниже. |

В пределах часа записанная identity процесса — только числовой PID. Проверка `starttime` выше относится к расчёту rates snapshot и summary. Источники: [identity процесса](../crates/kronika-registry/src/codec/os_process.rs), [snapshot predecessor](../crates/kronika-query/src/snapshot/mod.rs), [summary reducers](../crates/kronika-query/src/hour/process_summary.rs), [история inspector](../bins/kronika-web/ui/src/detail.tsx), [rates lanes](../crates/kronika-query/src/hour/lanes.rs), [расчёты устройств](../bins/kronika-web/ui/src/system-view.tsx).

| Преобразование отображения | Правило |
|---|---|
| Process CPU | Записанные jiffies делятся на записанный положительный `clock_ticks_per_sec`; CPU seconds на wall second — core equivalents. |
| Linux memory | Записанные KiB умножаются на 1,024. |
| PostgreSQL blocks | Записанное число blocks умножается на записанный `block_size`. Heatmap cuts показывают raw counts при отсутствии размера; derived byte metrics, которым нужен размер, остаются недоступными. |
| Формат bytes | Двоичные ступени по 1,024; до одного десятичного знака ниже 100 scaled units, иначе целые units. |
| Формат процентов | До одного десятичного знака; `0 < x < 0.1` отображается как `<0.1%`. |
| Формат cores | До трёх десятичных знаков. |
| Формат duration | Преобразует заявленную единицу входа в ns, µs, ms, s, min или h по величине. `TIME` использует целые CPU seconds в `MM:SS`, `H:MM:SS` или `D-HH:MM:SS`. |
| Null | Отображается как `—`; ноль остаётся числом. |

Форматирование выполняется после расчёта. Источники: [formatters](../bins/kronika-web/ui/src/model.ts), [масштабирование heatmap cuts](../bins/kronika-web/ui/src/activity-cuts.ts).

## Статистика графиков

Вход статистики каждой линии — конечные числовые значения этой линии в отображаемом chart frame; null и nonfinite values исключаются. Каждый sample имеет одинаковый вес. Для возрастающего ряда `x₁ … xₙ`, nearest-rank `p(q) = x[max(1, ceil(q × n))]`, где `q = 0.50, 0.90, 0.99`. `Min = x₁`, `Max = xₙ`; `Last` — последний конечный sample в порядке времени графика. Интерполяции и взвешивания по duration нет. Пустой вход не создаёт строку статистики. Источник: [`seriesStats`, `chartStatsRows`](../bins/kronika-web/ui/src/uplot-chart.tsx).

## Heatmaps

### Ячейки и ranking

UI использует 60 колонок на час для Processes, Statements, Plans, databases, cgroup CPU и cgroup I/O; Tables и Indexes используют 12. Для `C` колонок boundary `bⱼ = h + floor(j × 3,600,000,000 / C)`, поэтому ячейка `j` представляет `[bⱼ, bⱼ₊₁)`.

Engine относит наблюдение к колонке, содержащей середину интервала между ним и предыдущим наблюдением этой identity; первое наблюдение использует собственный timestamp. При входе counter в новую колонку предыдущая точка переносится в её расчёт. Интервал относится к одной колонке; counter difference не распределяется пропорционально по всем пересечённым boundaries.

| Значение | Counter | Gauge |
|---|---|---|
| Ячейка entity | `(last − first) / elapsed_seconds` по наблюдениям ячейки, включая перенесённый predecessor | Последнее наблюдение, отнесённое к ячейке |
| Ranking entity и итог справа | Последний counter минус первый в запрошенном диапазоне | Максимальное наблюдение диапазона, кроме RSS Grid ниже |
| Ranking группы | Сумма summaries её entities | Сумма summaries её entities |
| Ячейка группы | Сумма доступных ячеек её entities | Сумма доступных ячеек её entities |
| Ячейка Total | Сумма доступных ячеек всех entities | Сумма доступных ячеек всех entities |
| Ячейка Other | Сумма для entities/групп вне отображаемого top | То же |
| Итог Total/Other справа | Сумма соответствующих counter summaries | Максимум соответствующих сумм ячеек; RSS Grid использует аддитивное среднее ниже |

Отсутствие слагаемых даёт null. Одно наблюдение counter не образует rate или counter total. Counter summary проверяет два крайних значения; промежуточное уменьшение отдельно не аннулирует весь диапазон. `RankingOnly` возвращает максимальный индивидуальный gauge summary в `totals_total`; Grid Total — максимум агрегированной ячейки. Источники: [`Obs`, `Accumulator`, `column_of_span`](../crates/kronika-query/src/heatmap/execution.rs), [типизированные запросы](../crates/kronika-query/src/heatmap/query.rs).

### Среднее RSS Grid

Для `os_process.rmem_kb` в Grid mode пусть `T` — множество уникальных timestamps, на которых запрос наблюдает пригодное RSS, `N = |T|`. Summary PID `p`: `meanRSS(p) = Σ recordedRSS(p,t) / N`. PID, отсутствующий на timestamp, ничего не добавляет в числитель; знаменатель общий для всех PID. Summaries групп, Total и Other суммируют эти средние и сохраняют тот же знаменатель. KiB умножаются на 1,024 для bytes. Это среднее samples без взвешивания по времени. Ячейки сохраняют gauge rule. `RankingOnly`, включая MCP rankings, сохраняет gauge maxima. Источник: [`RssMean`, `score`, `additive_summary`](../crates/kronika-query/src/heatmap/execution.rs); существующая проверка: [RSS artifact test](../bins/kronika-web/src/tests/artifacts/heatmap_rss.rs).

### Cuts, grouping и scales

Processes группируются по записанному `comm`; нажатие подписи группы передаёт этот текст в поиск процессов. Группа entity назначается при первом её наблюдении engine внутри диапазона. Подписи cgroup activity не фильтруют таблицу. Grouping Tables/Indexes соответствует выбранному уровню relations. Компактные heatmaps показывают восемь строк; развёрнутый top: 10, 25, 50 или 100. Скрытые строки включаются в Other.

| Heatmap | Доступные cuts: записанные operands |
|---|---|
| Processes | CPU: `utime + stime`; RSS: `rmem_kb`; Read: `read_bytes`; Write: `write_bytes`; Major faults: `majflt`; Run delay: `rundelay_ns` |
| Statements | Execution time: `total_exec_time`; Calls: `calls`; Rows: `rows`; Shared read: `shared_blks_read`; Shared dirtied: `shared_blks_dirtied`; Temp written: `temp_blks_written`; WAL: `wal_bytes` |
| Plans | Execution time: `total_time`; Calls: `calls`; Rows: `rows`; Shared read: `shared_blks_read`; Temp written: `temp_blks_written` |
| Tables | Writes: `n_tup_ins + n_tup_upd + n_tup_del`; Sequential read: `seq_tup_read`; Heap read: `heap_blks_read`; Dead tuples: `n_dead_tup` (gauge); Autovacuum time: `total_autovacuum_time` |
| Indexes | Scans: `idx_scan`; Tuples read: `idx_tup_read`; Blocks read: `idx_blks_read` |
| Databases | Commits: `xact_commit`; Rollbacks: `xact_rollback`; Read: `blks_read`; Temp bytes: `temp_bytes`; Deadlocks: `deadlocks` |
| Cgroup CPU | CPU: `usage_usec`; Throttled: `throttled_usec` |
| Cgroup I/O | Read/write bytes: `rbytes`, `wbytes`; Read/write operations: `rios`, `wios` |

Положительная интенсивность цвета: `min(6, max(1, ceil(6 × sqrt(value / scaleMax))))`; у нуля intensity zero, для null ячейка не рисуется. Global scale использует максимум ячеек отображаемых строк и Other. Row scale использует максимум внутри строки. Total всегда использует собственный максимум. Эти controls меняют цветовые пороги; вычисленные значения сохраняются. Источники: [cuts](../bins/kronika-web/ui/src/activity-cuts.ts), [grouping и controls](../bins/kronika-web/ui/src/activity.tsx), [цветовая шкала](../bins/kronika-web/ui/src/heatmap.ts).

## Health

Health — целочисленные проценты. Пусть `E = t₁ − t₀` в microseconds; `Sᵣ` — разность PSI `some_total` для CPU, memory или I/O. Определим `W = min(E, max(S_cpu, S_memory, S_io))`.

`OS health = 100 − floor((100 × W + floor(E / 2)) / E)`.

Нужны все три PSI-компонента и положительный интервал; уменьшение компонента даёт null для пары. Null PSI snapshot очищает предыдущий snapshot. Machine recordings используют host PSI (`scope = 0`); container recordings — pod/container PSI (`scope = 1` или `3`). Записанные environment и boot identity выбирают scope и связывают predecessor samples. Неоднозначная metadata даёт unknown health.

Пусть `A` — число записанных строк `pg_stat_activity` с состоянием ровно `active`, `C` — записанная положительная целая CPU capacity `postgresql_effective_cpus`. Учитывается каждая active row, включая active non-client backend. Service slots `K = 2 × C`:

`PG penalty = 0`, если `A ≤ K`; иначе `floor((100 × (A − K) + floor(A / 2)) / A)`.

`PostgreSQL health = 100 − PG penalty`.

Capacity берётся из `KRONIKA_POSTGRES_EFFECTIVE_CPUS` и записывается collector; вычисляемого host/container fallback нет. `KRONIKA_PG_DSNS` включает PostgreSQL collection. Отсутствующий active count или отсутствующая/нулевая capacity дают null. Конфликтующие activity layouts на одном timestamp дают unknown count.

`C` относится к наблюдаемому инстансу PostgreSQL. У удалённого сервера или
отдельного cgroup ёмкость CPU может отличаться от ёмкости коллектора.
Записанное для PostgreSQL значение одинаково используется при чтении WAL,
ZMS и HTML-отчёта; число CPU машины, на которой открыта запись, в формулу
не входит.

На каждом OS health timestamp overall health использует последний PostgreSQL health не позже него, возрастом не более записанного `postgresql_interval_seconds`:

`Overall health = max(0, OS health − PG penalty)`.

Отключённый PostgreSQL добавляет нулевой penalty. Включённый PostgreSQL с unknown или более старым sample делает overall health равным null; unknown OS health всегда даёт null. Web source flags в формулах не участвуют. Источники: [целочисленные формулы](../crates/kronika-index/src/health.rs), [scope, activity counts и выбор времени](../crates/kronika-index/src/build.rs), [metadata collector](../bins/kronika-collector/src/service_sections.rs).

## Timeline marks

Каждая mark хранит source locator: segment, physical layout, row, field, timestamp и kind. `KnownBad` использует фиксированные predicates. Predicates Linux CPU/load/memory/filesystem/OOM и overall health приведены в [Linux marks](metrics-linux.ru.md#фиксированные-marks-и-цвета-cells); PostgreSQL predicates:

| Источник | Predicate |
|---|---|
| Activity | Active-row count `> 2 × recorded postgresql_effective_cpus`; нужны пригодная capacity и однозначный activity layout на timestamp |
| Locks | Записанный список `blocked_by` непустой |
| Database deadlocks | Более поздний `deadlocks` превышает predecessor того же layout и `datid` |
| Database checksum failures | Более поздний доступный `checksum_failures` превышает предыдущее доступное значение database/layout; явный null разрывает пару |
| Database fatal/killed sessions | Положительный прирост `sessions_fatal` или `sessions_killed`, проверяемых независимо для database/layout |
| Database transaction-ID age | `frozen_xid_age ≥ 1,600,000,000` или `min_mxid_age ≥ 1,600,000,000`; каждое поле проверяется отдельно |
| Archiver | Более поздний `failed_count` превышает predecessor |
| Slow-query log group | Конечный `max_duration_ms ≥ 5,000` |
| Error log group | Записанная category `5` (`data_corruption`) |

Age mark использует фиксированную границу 1.6 billion. Текущие freeze/failsafe settings сервера в неё не входят. Все counter-increase predicates требуют более позднего timestamp; один counter sample не создаёт increase mark. `Event` marks адресуют записанные строки шести поддерживаемых indexed PostgreSQL log layouts: errors, checkpoints, autovacuum/autoanalyze, slow queries, lock waits, lifecycle. Data-corruption row также получает KnownBad locator. Locators сортируются, дедуплицируются и ограничиваются 4,096 на physical-section block; block сохраняет число до ограничения.

`Spike` / «Резкий рост» — зарезервированный locator kind для совместимости с записанными indexes. Текущий index builder не создаёт Spike marks и не реализует threshold или формулу резкого роста. Источники: [все predicates](../crates/kronika-index/src/detect/direct.rs), [выбор event layouts и построение blocks](../crates/kronika-index/src/detect/mod.rs), [locator kinds и предел](../crates/kronika-index/src/findings.rs).
