# Интерфейс и справочник данных

[English version](features.md) · [Руководство оператора](operator-guide.ru.md) · [README](../README.ru.md)

Kronika записывает снимки Linux, статистику PostgreSQL и разобранные события журналов. Web-приложение и экспортированный HTML читают эти записи через общий query engine.

| Справочник | Содержание |
| --- | --- |
| [Время, агрегация и Health](metrics-time.ru.md) | Час/cursor, интервалы снимков, разности counters, heatmap, статистика графиков, Health и отметки. |
| [Linux](metrics-linux.ru.md) | Processes, Host, container cgroups, CPU, память, storage, файловые системы, сеть, topology и USE. |
| [PostgreSQL](metrics-postgresql.ru.md) | Overview, Databases, Activity, Locks, Vacuum, Statements, Plans, Tables, Indexes и Settings. |
| [Events](#events) | Ключи группировки журналов, агрегация, единицы и представительные записи. |
| [MCP](#mcp) | Четырнадцать tools, типизированные входы, ограничения и поля ответа. |
| [Export](#export) | Редактор времени, включительные целые секунды и автономный HTML. |
| [Установка](../INSTALL.ru.md) | Готовые программы, сбор данных, web-доступ и сборка из исходников. |

## Контролы

| Контрол | Состояние или операция |
| --- | --- |
| День/час; предыдущий/следующий час | Выбранный календарный час в Browser time или UTC. Графики сохраняют диапазон полного часа. |
| Browser time / UTC | Отображение календарного времени и выбор часа. Записанные Unix microseconds сохраняют своё значение. |
| Часы workspace; указатель timeline | Cursor. Указатель показывает предварительное записанное время; отпускание фиксирует его. Уход из preview возвращает выбранный cursor. |
| ← / →; предыдущее/следующее наблюдение | Предыдущая/следующая точка упорядоченного множества загруженных timestamps текущего view, включая разные частоты источников. Кнопки, поля ввода, selects и редактируемый текст сохраняют свои действия стрелок. |
| Refresh | Заново загрузить catalog и выбранный view. Текущий видимый час обновляется через 15 секунд после завершения загрузки; возврат видимости обновляет сразу. Исторические часы не опрашиваются. Закреплённый cursor сохраняется; следующий за данными переходит к последнему наблюдению. Выбранный час сохраняется. |
| View | Processes, Host, PostgreSQL или Events. |
| Lens | Набор колонок и начальная сортировка поверхности; определения метрик остаются указанными в предметном справочнике. |
| Заголовок колонки | Сортировка полного подходящего набора перед pagination; иерархические режимы сохраняют порядок дерева/цепочки. |
| Search; Apply / Enter | Применить корректное выражение. Некорректный черновик сохраняет предыдущий результат и URL. Chips удаляют отдельные применённые условия. |
| Load more / Retry | Получить следующую страницу / повторить неудачный запрос. Статус pending/error относится к сохранённым строкам до завершения запроса. |
| Строка; Inspector Detail / Chart | Выбрать identity; показать записанные факты или выбранную метрику истории. Связанные вкладки зависят от типа строки. |
| Метрика Chart; series / All | Выбрать показатель и его ряды. Legend связывает цвет с рядом. Hover показывает значения без изменения выбора. |
| Expand / restore chart | Изменить размер графика Inspector. |
| Разделитель Inspector | Изменить размер; стрелки меняют ширину, Home/End выбирают пределы. Узкие layouts используют overlay или нижнюю панель. |
| Заголовок Activity | Загрузить/открыть ranking полного часа. Компактный вид оставляет восемь строк, остальные вклады входят в Other; полный экран предлагает Top 10/25/50/100, начальное значение 25. |
| Global / Per row | Знаменатель цветовой шкалы heatmap: общий максимум / максимум каждой строки. Total всегда имеет свою шкалу. |
| Ячейка heatmap | Установить cursor в `cell.to − 1 µs`; затем таблица выбирает снимки своих источников. |
| Имя объекта Activity | Применить фильтр объекта/группы на поддерживаемых поверхностях; при отсутствии объекта у cursor перейти к наиболее занятому интервалу. Имена cgroup не фильтруют таблицу. |
| `?`; справка поля; Esc | Открыть общую справку / определение метрики; закрыть открытую панель или выбор. |
| Copy exact value | Скопировать неокруглённое значение; запасное действие выделяет точный текст. |
| Язык; тема | EN/RU; светлая/тёмная. Сохраняются в браузере вместе с открытым состоянием Activity и шириной Inspector. |
| Sign out | Завершить сессию работающего web-сервера. |

URL содержит час/cursor, view, lens, sort, search и поддерживаемый выбор строки. Back/Forward восстанавливает их. Обычный переход между поисковыми поверхностями очищает `find`; переход к связанному объекту задаёт выражение целевой поверхности и сохраняет время.

Исходники: [address и navigation](../bins/kronika-web/ui/src/address.ts), [keyboard](../bins/kronika-web/ui/src/keyboard.ts), [refresh](../bins/kronika-web/ui/src/refresh.ts), [Activity](../bins/kronika-web/ui/src/activity.tsx), [Inspector](../bins/kronika-web/ui/src/inspector.tsx).

## Search

| Синтаксис | Значение |
| --- | --- |
| Обычный текст | Текстовый поиск текущей поверхности. |
| `field:value` | Условие по строковому полю/identity; кавычки сохраняют пробелы, `*` и `?` задают строковый шаблон. |
| `field>quantity`, `field<quantity` | Строгое сравнение в единицах поля; null не соответствует количественному условию. |
| `AND`, `OR`, скобки | Регистр операторов неважен, `AND` выше `OR`; максимум 8 условий, 31 token, 4 вложенные группы и 1024 символа. |
| `MB`, `MiB`, `/s` | Десятичные байты, двоичные байты, величины в секунду. Регистр единиц значим. |
| Группировка Tables/Indexes | Имена фильтруют объекты перед агрегацией; величины фильтруют рассчитанные группы. `AND` соединяет эти стадии; смешивающий их `OR` отклоняется. |

`NOT`, неявные boolean-операторы, `=`, `==`, `!=`, `>=` и `<=` отклоняются. Справка Search перечисляет допустимые поля и единицы поверхности, включая поля вне текущего lens. Фильтры применяются перед сортировкой и pagination.

```text
command:postgres* AND rss>100MiB
state:active AND wait_type:Lock
query_id:-665077864269413128
schema:shop AND table_name:orders
```

Исходники: [parser](../bins/kronika-web/ui/src/search.ts), [общие определения поиска](../crates/kronika-query/src/snapshot/search.rs).

## Настроенные и записанные источники

| Вход | Значение |
| --- | --- |
| Collector `KRONIKA_PG_DSNS` | Настроенные подключения включают сбор PostgreSQL. |
| Записанный `instance_metadata` | Окружение, частота сбора, флаг включённого PostgreSQL и необязательная эффективная CPU capacity базы. Используется для scope/time/Health. |
| Обязательный web `KRONIKA_WEB_SOURCES` | Беззнаковый bitset catalog: `0` оба configured-флага выключены, `1` OS, `2` PostgreSQL, `3` оба. Обозначает настроенные источники; не фильтрует записи, не управляет сбором, не скрывает вкладки и не рассчитывает Health. |
| UI-флаг настроенного PostgreSQL | Вместе с наличием записанного PostgreSQL управляет подавлением tooltip об отсутствии PostgreSQL-данных. Настроенный OS-бит не имеет UI-потребителя. |
| Записанный physical layout | Определяет доступные поля версии PostgreSQL или варианта расширения. |

Исходники: [web config](../bins/kronika-web/src/config.rs), [source availability](../bins/kronika-web/ui/src/source-availability.ts), [collector config](../bins/kronika-collector/src/config.rs).

## Events

Events читает выбранный диапазон длиной до одного часа. Для группы `firstTs=min(t)`, `lastTs=max(t)`. Её 60 минутных ячеек суммируют веса событий с `bucket=floor((t−from)/60,000,000)` для timestamps в Unix microseconds. Число группы может иметь другую агрегацию, указанную ниже. Числовые длительности записаны в миллисекундах и отображаются общим адаптивным formatter длительности.

| Группа / записанная section | Ключ группы | Количество, метрики и представительная запись |
| --- | --- | --- |
| Errors / `pg_log_errors` | `(severity, category, pattern)` | `Σ count` (отсутствующий count даёт 1). Severity, SQLSTATE и category из самой ранней записи. Database/user показаны, только если все записи имеют одно непустое значение. |
| Slow queries / `pg_log_slow_queries` | `pattern` | `Σ count`; `totalMs=Σ total_duration_ms`; `maxMs=max(max_duration_ms)`. Отсутствующие числовые значения дают 0 для этих агрегаций длительности. Представительная запись имеет максимальную длительность, при равенстве — самое раннее время. Threshold — последнее записанное неотрицательное `log_min_duration_statement`, с переводом s/min в ms. |
| Autovacuum / `pg_log_autovacuum` | `(kind, relation)` | Runs = количество записанных строк; total duration = сумма доступных `elapsed_ms`; removed tuples = сумма доступных `tuples_removed`; dead-not-removable tuples = значение последней строки. `kind=1` означает analyze. Представительная запись — самая ранняя. |
| Checkpoints / `pg_log_checkpoints` | Общая группа starts/completions | `starts=count(phase=0)`, `completes=count(phase=1)`, отображаемое число `max(starts,completes)`. Timed = starts с `time` в reason; requested = starts − timed. Максимум `sync_ms` и сумма `buffers_written` из завершений. Представительная запись — первая прочитанная. |
| Checkpoint warnings | `phase=2` | Число предупреждений; минимум доступного `seconds_apart` (секунды); первая прочитанная запись. |
| Lock waits / `pg_log_lock_waits` | Точный текст `holding_pids` | Waiting-строки `kind=0` создают группы. Acquired-строки присоединяются к последней предшествующей waiting-строке с теми же `(pid,lock_target)`. Count = waiting-записи (минимум 1); waiters = уникальные записанные PID-строки; max duration = максимум доступного `duration_ms`; targets = уникальные тексты targets. Несопоставленные acquired-записи образуют отдельную группу с количеством occurrences. |
| Lifecycle / `pg_log_lifecycle` | Группа на каждую записанную строку | Kind, PID, signal и shutdown mode этой строки. Count 1. |
| PgBouncer / `pgbouncer_events` | `(level, exact text)` | Количество строк; самая ранняя представительная запись. Database/user/host/source file показаны, только если все строки имеют одно непустое значение. |

Необязательные суммы/максимумы остаются null, если ни одна строка не содержит значения. При одинаковом времени раннюю представительную запись определяет порядок чтения. Группы сортируются по tier, убыванию count, убыванию last time и ключу.

| Tier | Точный состав |
| --- | --- |
| Critical | PostgreSQL FATAL/PANIC (`severity=1/2`); lifecycle `kind=0`; PgBouncer `level=0`. |
| Notable | Остальные PostgreSQL errors, кроме WARNING/LOG; slow queries; checkpoint warnings; lock waits; остальные lifecycle; PgBouncer `level=1/2`. |
| Routine | PostgreSQL WARNING/LOG (`severity=3/4`); autovacuum/analyze; checkpoints; остальные PgBouncer levels. |

`(nodb)` и `(nouser)` — буквальные значения контекста соединения PgBouncer для неустановленных database/user. Collector сохраняет их; отсутствующее поле контекста равно null. Host хранится без порта соединения. PgBouncer представлен группами console без отметок общей timeline.

Digest источника/типа фильтрует группы; Search сопоставляет отображаемые title/chips через `text`, `kind`, `source`, `category`. Раскрытие показывает метрики группы; выбор представительной записи получает её полную записанную строку. Cluster timeline выбирает свой интервал и источники; Show all возвращает час. Threshold и sharp-rise marks находятся в отдельном списке. `pg_log_temp_files` доступен через MCP `occurrences`, вне группированной console.

Исходники: [event query и поля](../crates/kronika-query/src/events.rs), [агрегация групп](../crates/kronika-query/src/events/group.rs), [контролы console](../bins/kronika-web/ui/src/events-view.tsx), [parser PgBouncer](../crates/kronika-source-log/src/pgbouncer.rs).

## MCP

`POST /mcp` предоставляет четырнадцать tools по записанным данным через stateless Streamable HTTP. Каждый tool возвращает structured JSON. HTTP Basic использует web-credentials, кроме режима `KRONIKA_WEB_AUTH=disabled`; user и password обязательны в конфигурации обоих режимов. Endpoint отклоняет `Origin` и query strings. Tools читают записанные файлы и не выполняют административных команд host или PostgreSQL.

| Tool | Входы | Ответ |
| --- | --- | --- |
| `kronika_list_recorded_sections` | Необязательный `section` | `recorded_from`, исключительный `recorded_to`; sections с source family, числом строк/байтов, полями, классами и единицами. |
| `kronika_get_instance` | `settings`: `non_default` (по умолчанию) или `all` | Последние host metadata и PostgreSQL settings с отдельными `host_as_of`/`settings_as_of`, числом строк и scope. `non_default` удаляет только точный записанный `source="default"`. |
| `kronika_rank_metrics` | `from`, `to`, непустой `rankings:[{section,fields,top?}]`; 1–4 поля; top 1–500, по умолчанию 25 | Отдельный упорядоченный результат на поле, включая повторы. Разности counters или максимумы gauges, unit, identities, labels, detail refs, total/other и число объектов. У каждого поля свой top limit. |
| `kronika_find_processes` | Общие входы finder | Строки процессов. |
| `kronika_find_postgresql_activity` | Общие входы finder | Состояния backend, waits и timestamps начала. |
| `kronika_find_postgresql_locks` | Общие входы finder | Записанные locks и контекст blockers. |
| `kronika_find_postgresql_vacuum` | Общие входы finder | Строки progress Vacuum. |
| `kronika_find_postgresql_databases` | Общие входы finder | Статистика database. |
| `kronika_find_postgresql_statements` | Общие входы finder | Интервальные метрики и identities statements. |
| `kronika_find_postgresql_plans` | Общие входы finder | Интервальные метрики и identities plans. |
| `kronika_find_postgresql_tables` | Общие входы finder; обязательный `group`: `object`, `schema`, `database`, `tablespace` | Агрегированные строки tables. |
| `kronika_find_postgresql_indexes` | Общие входы finder; обязательный `group`, те же значения | Агрегированные строки indexes. |
| `kronika_find_events` | `from`, `to`, `limit`; `representation=groups` (по умолчанию) или `occurrences`; необязательный `sources` | `groups` или `occurrences` и `truncated`; диапазон до одного часа. Отсутствующий/null sources выбирает поддерживаемые источники; `[]` не выбирает ни одного. Temp files требуют occurrences. |
| `kronika_get_row_detail` | Обязательный неизменённый `detail_ref` | Полная записанная строка; текстовые объекты содержат `stored_text`, десятичный `full_len`, `truncated`, `sha256`. |

Общие входы finder: необязательный `at` (по умолчанию последний timestamp всего хранилища), необязательные `filters` и `sort:{field,direction}`, обязательный `limit` 1–5000. Direction — `asc`/`desc`, nulls в конце; без sort сохраняется порядок identity. Ответ `{rows,truncated}` без pagination cursors. Ссылки detail идентифицируют записанные объекты; агрегаты без одной исходной строки не содержат ссылок.

Типизированные filters содержат `field`, `op`, `value`, а для `in` — `values`. До восьми filters соединяются AND; `in` содержит 1–8 значений, соединённых OR. Text принимает нечувствительные к регистру `eq`, буквальный `contains`, `in`; identifiers принимают `eq`/`in` с целыми или точными десятичными строками; quantities принимают строгие `gt`/`lt` с неотрицательными JSON integers в указанной базовой единице поля. `tools/list` содержит имена полей и единицы каждой поверхности.

Время принимает целые Unix microseconds или канонические знаковые десятичные строки, RFC 3339 с timezone, `now`, `now-Nus/ms/s/m/h/d/w`. `now` — время запроса. Диапазоны tools используют `[from,to)`; finders выбирают наблюдения не позже `at` с учётом cadence section. Кодированные аргументы finder/Events/ranking ограничены 65,536 байтами. Ошибки содержат `isError`, `record="error"`, `message` и при наличии `valid_options` или `ranking_index`.

Панель **Connect an AI agent** содержит endpoint `<origin>/mcp`, состояние credentials, выбор Claude Code/Codex CLI/Cursor, сгенерированный текст подключения и Copy. Она получает authorization header через `/api/mcp-access`; при недоступности этих данных выводит placeholder. Панель настраивает подключение клиента. [Команды и конфигурация клиентов](mcp-clients.ru.md).

Исходники: [схемы tools](../bins/kronika-web/src/mcp/catalog.rs), [типизированные filters](../bins/kronika-web/src/mcp/filter.rs), [время](../bins/kronika-web/src/mcp/time.rs), [форматы ответов](../bins/kronika-web/src/mcp/semantics.rs), [панель подключения](../bins/kronika-web/ui/src/mcp-connect.tsx).

## Export

| Вход/контрол | Определение |
| --- | --- |
| From / To | Включительные целые Unix seconds `F,T`, `0<F≤T`; длительность `T−F+1` секунд. Редактор использует выбранную Browser time/UTC zone и отдельные даты границ. |
| This hour | `F=floor(hour_us/10⁶)`, `T=F+3599`. |
| Around cursor ±5/15/30 min | При `C=floor(cursor_us/10⁶)` и `N` минутах: `F=C−60N`, `T=C+60N−1`; длительность `120N` секунд. |
| Предыдущий/следующий час | Добавить −3600/+3600 к обеим границам. |
| День и редакторы `HH:MM:SS` | Разрешить календарную дату/время в выбранной zone. Несуществующее время DST отклоняется; повторённое время требует выбора первого/второго вхождения. |
| Имя файла | `kronika-YYYY-MM-DD-HHMMSS-YYYY-MM-DD-HHMMSS-utc.html`, обе границы в UTC. |
| Download | `GET /api/export?from=F&to=T`; видимый диапазон экспорта `[10⁶F,10⁶(T+1))`. Включены все записанные sections диапазона. |
| Progress | Секунды подготовки, затем полученные/полные байты при известном размере; после завершения — имя, байты и затраченное время. Предыдущая длительность подготовки сохраняется в браузере. |

HTML содержит запись, UI, fonts и WASM query engine. Он открывается из локального файла и синхронно исполняет запросы в WASM на основном потоке браузера. Видимый диапазон фиксирован; live refresh, login, export и MCP connection отключены. Записанные query text, plans, logs и command lines входят в файл.

Четыре устанавливаемые программы: `kronika-collector`, `kronika-web`, `kronika-dump`, `kronika-report`. CLI [slice](../bins/kronika-dump/README.ru.md) создаёт диапазон записи; [report](../bins/kronika-report/README.ru.md) преобразует его в HTML. Их `--help` описывает параметры установленных программ.

Исходники: [арифметика диапазона](../bins/kronika-web/ui/src/export-range.ts), [календарное время](../bins/kronika-web/ui/src/export-time.ts), [диалог загрузки](../bins/kronika-web/ui/src/export-dialog.tsx), [offline transport](../bins/kronika-web/ui/src/report-transport.ts).
