# Класс 2: события логов PostgreSQL

[English version](postgresql.md)

Секции занимают `2_001_001`–`2_099_999`. [Codec](../../crates/kronika-registry/src/codec/pg_log.rs) определяет поля и единицы; [справочник Events](../features.ru.md#events) — группировку в интерфейсе.

## Источники и форматы

`KRONIKA_PG_DSNS` обнаруживает файлы через `pg_current_logfile()`; `KRONIKA_PG_LOGS` задаёт пути и шаблоны. Файл, найденный обоими способами, читается один раз. Ранее не читанный файл открывается с начала; известный файл продолжает чтение с сохранённого offset.

| Формат по имени файла | Разбор |
| --- | --- |
| `.csv` | csvlog: поля читаются по позиции; новые хвостовые столбцы игнорируются. Открытые кавычки продолжают запись на следующей строке. |
| `.json` | jsonlog: один JSON-объект на строку. |
| Любое другое имя | stderr: severity, сообщение и строки продолжения. `log_line_prefix` обнаруживается через DSN; для явно заданного пути без известного prefix база и пользователь отсутствуют. |

`system_identifier` читается из `pg_control_system()` для DSN-источника; для явно заданного пути равен `null`. `source_file` хранит имя файла. Формат выбирается по суффиксу имени, содержимое для выбора формата не анализируется.

В stderr строки `DETAIL:`, `HINT:`, `CONTEXT:`, `STATEMENT:` и `QUERY:` присоединяются к записи; строка с начальной табуляцией продолжает предыдущее поле. `STATEMENT:` и `QUERY:` записываются в `statement`.

Время текстового лога разбирается как локальное время хоста коллектора; обозначение зоны в строке пропускается. В неоднозначный час выбирается первое вхождение. Если время отсутствует или не разбирается, parser PostgreSQL использует время сбора. Дробная часть сохраняется до микросекунд. Для корректного времени текстового лога `log_timezone` должен совпадать с зоной хоста коллектора.

## Зарегистрированные типы

| `type_id` | Секция | Семантика | Ключ сортировки |
|-----------|---------|-----------|----------|
| `2_001_001` | `pg_log_errors` | `event_stream` | `(severity, category, pattern, ts)` |
| `2_002_001` | `pg_log_checkpoints` | `event_stream` | `(ts, phase)` |
| `2_003_001` | `pg_log_autovacuum` | `event_stream` | `(ts, kind, relation)` |
| `2_004_001` | `pg_log_slow_queries` | `event_stream` | `(pattern, ts)` |
| `2_005_002` | `pg_log_lock_waits` | `event_stream` | `(ts, kind, pid)` |
| `2_006_001` | `pg_log_lifecycle` | `event_stream` | `(ts, kind)` |
| `2_007_001` | `pg_log_temp_files` | `event_stream` | `(ts, size_bytes)` |

## Группировка при сборе

| Секция | Ключ одного пакета чтения | Сохранённый пример |
| --- | --- | --- |
| `pg_log_errors` | `(severity, category, pattern)` | Первое вхождение с исходными значениями. Pattern нормализует имена в кавычках, списки в скобках, номера после `transaction`/`relation`/`process`/`PID`/`signal` и адреса WAL. |
| `pg_log_slow_queries` | SQL с нормализованными литералами | Самое медленное вхождение и его длительность. |

Группа, пересекающая границу пакета чтения, записывается отдельной строкой в каждом пакете. Остальные секции содержат отдельные события.

## Распознаваемые записи

`WARNING` и более высокие уровни создают error groups. Для `LOG` распознаются следующие английские формы; остальные сообщения `LOG` пропускаются. Локализованные сообщения сохраняют error groups, но не распознаются по английским формам `LOG`. Из сообщений длительности Extended Protocol поддерживаются `statement:` и `execute`; `parse` и `bind` пропускаются.

| Секция | Начало/форма сообщения | Настройка источника |
| --- | --- | --- |
| `pg_log_checkpoints` | `checkpoint starting:`, `checkpoint complete:`, `checkpoints are occurring too frequently` | `log_checkpoints` |
| `pg_log_autovacuum` | `automatic vacuum of table`, `automatic analyze of table`, включая aggressive и anti-wraparound | `log_autovacuum_min_duration` |
| `pg_log_slow_queries` | `duration: <ms> ms  statement: <sql>`, `duration: <ms> ms  execute <name>: <sql>` | `log_min_duration_statement` |
| `pg_log_lock_waits` | `process <pid> still waiting for`, `process <pid> acquired`; PID-списки из `holding the lock:` и `Wait queue:` → `holding_pids`, `wait_queue` | `log_lock_waits` |
| `pg_log_lifecycle` | `server process (PID …) was terminated`, `received … shutdown request`, `database system is ready to accept connections` | Сообщения жизненного цикла сервера |
| `pg_log_temp_files` | `temporary file: path …, size …` | `log_temp_files` |

## Категории ошибок

`PANIC` всегда получает категорию `5`. Сообщение с `terminated by signal` и `: killed` получает `4`. Для остальных выбирается первая совпавшая категория в порядке таблицы; нераспознанный `FATAL` получает `6`, остальные — `10`.

| Код | Категория | Содержание |
| ---: | --- | --- |
| `0` | lock | Deadlock, lock timeout, ожидание lock. |
| `1` | constraint | Duplicate key, foreign key, not-null, check, exclusion. |
| `2` | serialization | `could not serialize access`. |
| `3` | timeout | Statement, transaction, idle-session timeout; cancellation. |
| `4` | resource | Память, connection slots, диск, OOM kill. |
| `5` | data corruption | Повреждённые страницы, нечитаемые блоки, каждый `PANIC`. |
| `6` | system | Файлы, I/O, crash, остальные `FATAL`. |
| `7` | connection | Reset, неожиданный EOF, broken pipe. |
| `8` | auth | Пароли, `pg_hba.conf`, права. |
| `9` | syntax | Синтаксис, отсутствующие объекты, некорректный ввод. |
| `10` | other | Остальные ошибки. |

## Границы чтения

Буфер чтения — 64 KiB; пакет — до 4 MiB исходных байтов. Один сбор читает до 256 MiB из каждого файла; остаток переходит в следующий сбор. Сохранённая исходная запись ограничена 64 KiB, текст сообщения/statement/продолжений — 5 KiB, pattern — 256 байт. Усечённые CSV-записи пропускаются, поскольку границы полей после усечения не определены.

Источники: [parser](../../crates/kronika-source-log/src/postgres.rs), [время](../../crates/kronika-source-log/src/timestamp.rs), [tail](../../crates/kronika-source-log/src/tail.rs), [цикл коллектора](../../bins/kronika-collector/src/log_sources.rs).
