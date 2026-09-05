# Kronika

[English version](README.md)

**Посмотрите, что происходило в Linux и PostgreSQL в один и тот же момент.**
Kronika сохраняет историю процессов и ресурсов вместе с активностью базы,
запросами, планами выполнения, блокировками и событиями из логов. Выберите час,
найдите нагруженный интервал на heatmap и перейдите от процесса к его backend
в PostgreSQL, связанным Statements и Plans, сохраняя выбранное время.

**[Открыть интерактивное демо в браузере →](https://vadv.github.io/kronika-reports/reports/kronika-container-demo-20min-77c422e.html)**
Установка и вход не нужны. В настоящем интерфейсе Kronika можно изучить запись
синтетической нагрузки PostgreSQL и Linux, сохранить HTML и открыть его без сети.

**Processes — кто расходовал CPU, память и I/O и в какое время.** Heatmap над
таблицей процессов показывает распределение нагрузки за час. Выберите строку
или ячейку, чтобы открыть историю процесса и его записанную активность в
PostgreSQL.

![Heatmap активности процессов и записанные сведения о процессе](docs/images/processes.png)

**Statements — от нагруженного интервала к SQL-запросам.** Сравните время
выполнения, число вызовов, активность buffers и WAL; текст запроса и его история
открываются рядом с выбранной строкой.

![Statements PostgreSQL: heatmap активности и сведения о запросе](docs/images/statements.png)

**Plans — планы, которые были записаны во время работы базы.** Перейдите по
Query ID из Statements в Plans и сравните метрики выполнения рядом с
сохранённым текстом плана.

![Записанные планы выполнения PostgreSQL и их метрики](docs/images/plans.png)

## История машины для разбора событий

Kronika полезна после замедления: процессы уже заняты другой работой, а текущие
представления PostgreSQL больше не показывают нужный момент. Коллектор ведёт
запись постоянно; браузер и MCP читают эту историю.

- **Один курсор времени для Linux и PostgreSQL.** Нагрузка на ресурсы, CPU и I/O
  процессов, выполняющиеся и ожидающие backends, блокировки и события остаются
  в одном часе при переходах между разделами.
- **Heatmap помогает читать плотную историю.** Сортируйте процессы, Statements,
  Plans, таблицы и индексы по нужной метрике. Выберите интервал для просмотра
  строк, затем откройте график конкретного объекта.
- **Подробные данные без отдельного стека метрик.** Коллектор пишет локальные
  файлы. Web читает их, содержит интерфейс внутри бинарного файла и освобождает
  данные запроса после его обработки. Отдельная база для хранения данных самой
  Kronika не нужна.
- **Результат разбора можно передать одним файлом.** Экспортируйте выбранный
  интервал в самодостаточный HTML с основным интерфейсом и движком запросов.
  Получателю нужен только браузер.

| Область записи | Что можно посмотреть |
| --- | --- |
| Процессы Linux | CPU, RSS, swap, disk и logical I/O, задержки планировщика, page faults, context switches, пользователей, command lines |
| Ресурсы Linux | Активность каждого CPU, память, PSI, throughput и latency дисков, mounts и ёмкость файловых систем, сетевые интерфейсы и TCP-счётчики, частоту и топологию CPU |
| Контейнеры | Cgroup CPU и throttling, память и лимиты, I/O по устройствам и mounts, потоки; данные контейнера, network namespace и host сохраняют собственную область измерения |
| Активность PostgreSQL | Состояния backends и ожидания, транзакции, цепочки блокировок, ход Vacuum, базы и настройки |
| Statements и Plans | Историю `pg_stat_statements` и `pg_store_plans`: вызовы, время выполнения и планирования, строки, активность buffers, WAL, тексты запросов и планов — в пределах установленного layout |
| Таблицы и индексы | Размер, scans, активность buffers, изменения строк, статистику Vacuum и Analyze, возраст транзакций; по объекту, схеме, базе или tablespace |
| События из логов | Ошибки PostgreSQL, медленные запросы, checkpoints, Autovacuum, ожидания блокировок и события жизненного цикла; события из логов PgBouncer |

Доступные поля зависят от ядра, версии PostgreSQL, установленных расширений и
прав доступа. Фактические секции и layouts перечислены в
[справочниках метрик](#документация). Отсутствующие значения не подменяются.

## Попробовать локально

Для работающего демо нужен Docker с Compose v2 на Linux amd64 или arm64.
Клонируйте репозиторий и выполните команды из его корня:

```sh
git clone https://github.com/vadv/Kronika.git kronika
cd kronika
docker compose --file compose.demo.yml up --build --wait
```

Откройте **<http://127.0.0.1:8080/>**, имя **`demo`**, пароль **`forensics`**.
При первом запуске собирается образ. В нём есть PostgreSQL 15, PgBouncer,
collector, web и ограниченная синтетическая нагрузка: OLTP-транзакции, смена
планов, ожидания блокировок, Vacuum, а также CPU, память, disk и loopback I/O
в Linux. Внешняя база данных не нужна.

```sh
docker compose --file compose.demo.yml logs --follow --tail=100
docker compose --file compose.demo.yml stop
```

Остановка сохраняет записанную историю. Другой порт, настройки нагрузки и
удаление данных описаны в [руководстве по демо](bins/kronika-demo/README.ru.md).

## Записать историю своей машины

### Собрать актуальные бинарные файлы

Поддерживаемая статическая native-сборка — **Linux x86-64 с musl**. Установите
`rustup`, инструменты сборки C и `musl-gcc` (`build-essential` и `musl-tools`
в Debian/Ubuntu), затем из корня репозитория выполните:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump -p kronika-report
```

В репозитории закреплена версия Rust 1.96.0. Интерфейс браузера и WebAssembly
для отчётов уже подготовлены: обычная Cargo-сборка не требует Node.js.

Опубликованный [архив v1.0.0](https://github.com/vadv/Kronika/releases/tag/v1.0.0)
содержит collector, web, dump и demo; **в нём ещё нет `kronika-dump slice`,
`kronika-report` и HTML-экспорта**. До публикации обновлённого архива используйте
сборку из исходного кода выше для возможностей, описанных на этой странице. Готового
архива для arm64 нет. [Упаковка и команды скачивания с проверкой](docs/releases.ru.md)
описаны отдельно для существующего релиза и подготовленных артефактов.

### Запустить коллектор

Для PostgreSQL создайте на сервере учётную запись мониторинга:

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE kronika_monitor LOGIN PASSWORD 'replace-with-password';
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
SQL
```

Запустите коллектор на Linux-машине, которую нужно записывать. Замените пароль
из примера в обеих командах. Коллектору нужен доступ к сведениям о процессах и
локальным логам; для простого локального запуска используется `sudo`.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  ./target/x86_64-unknown-linux-musl/release/kronika-collector
```

Одной `KRONIKA_PG_DSNS` достаточно для включения сбора PostgreSQL. Уберите её,
если нужна только запись Linux. Первый DSN задаёт сервер для сбора метрик по
всем доступным этой роли базам; установленные расширения обнаруживаются
автоматически. Логи должны быть доступны локально. Необязательные пути к логам,
права на расширения, интервалы и CPU capacity для PostgreSQL health описаны в
[руководстве коллектора](bins/kronika-collector/README.ru.md).

### Открыть web и MCP

В другом терминале укажите тот же каталог данных:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_SOURCES=3 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  ./target/x86_64-unknown-linux-musl/release/kronika-web
```

Откройте **<http://127.0.0.1:8080/>** и войдите с заданными учётными данными.
По умолчанию web слушает loopback. `KRONIKA_WEB_SOURCES=3` отмечает OS и
PostgreSQL как настроенные семейства источников (`1` — только OS); переменная
не включает сбор и не фильтрует сохранённые данные. Настройки и HTTP API описаны
в [руководстве web](bins/kronika-web/README.ru.md).

Этот же процесс отдаёт MCP по адресу **`http://127.0.0.1:8080/mcp`**, с той же
аутентификацией. MCP-клиент может получать рейтинги, строки, историю и события
из записи для разбора происходившего. Отдельный MCP-сервер не нужен. Например,
получите список инструментов с учётными данными web (`curl` запросит пароль):

```sh
curl --fail --silent --show-error --user kronika \
  --header 'Content-Type: application/json' \
  --header 'Accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
  http://127.0.0.1:8080/mcp
```

Для Claude Code, Codex CLI и Cursor используйте панель MCP в web или
[руководство по MCP-клиентам](docs/mcp-clients.ru.md). MCP читает запись,
не обращаясь к наблюдаемому PostgreSQL или текущему состоянию машины.
У статического HTML-отчёта нет MCP-эндпоинта.

## Сохранить и передать интервал

Кнопка **«Экспорт»** в web скачивает один `.html` за выбранный интервал. Отчёт
содержит основной интерфейс на React, движок `kronika-query` на Rust,
скомпилированный в WebAssembly и работающий в одном потоке Web Worker,
выбранные данные ZMS и их канонический IDX. Файл открывается прямо в браузере:
сервер, сеть, внешние ресурсы и сопутствующие файлы не нужны. Таблицы, heatmap,
поиск и графики остаются интерактивными.

Утилиты командной строки работают с той же записью. Посмотрите размеры секций:

```sh
sudo ./target/x86_64-unknown-linux-musl/release/kronika-dump /var/lib/kronika
```

Создайте срез и отчёт; замените время на интервал своей записи:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  ./target/x86_64-unknown-linux-musl/release/kronika-dump slice \
  --from 2026-09-05T19:00:00Z --to 2026-09-05T19:59:59Z \
  --out incident.zms
sudo chown "$(id -u):$(id -g)" incident.zms

./target/x86_64-unknown-linux-musl/release/kronika-report \
  --from 1788634800000000 --to-exclusive 1788638400000000 \
  incident.zms incident.html
```

Корень хранилища dump — настоящий каталог, не отдельный файл и не symlink.
Обе границы среза включены и задаются целыми секундами в RFC 3339. Новый ZMS
может содержать до 30 секунд соседних отсчётов для вычислений. Границы report
выше заданы в Unix-микросекундах ровно для **19:00–20:00 UTC**: соседний контекст
не попадает в навигацию. Без этих параметров report показывает весь временной
диапазон входного файла. Slice отказывается писать поверх существующего файла;
report атомарно заменяет свой HTML. Подробности — в руководствах
[dump](bins/kronika-dump/README.ru.md) и [report](bins/kronika-report/README.ru.md).

## Стоимость сбора

В замере нагрузки с **примерно 500 таблицами и 3 тыс. индексов** скорость записи
составила около **184 MB в сутки в сжатых ZMS**. Это пересчёт по 43 готовым
сегментам со средним размером 1,92 MB и интервалом записи 15 минут. Число
относится к готовой записи, а не ко всему каталогу хранения или любой нагрузке.

Цель retention по умолчанию — **2 GiB**. В неё входят готовые ZMS, `active.wal`,
IDX и распознанные временные файлы. Число сохраняемых дней зависит от их размера
и вашей нагрузки. `KRONIKA_RETENTION` задаёт бюджет в байтах или автоматическую
цель по заполнению файловой системы; настройки и замер приведены в разделе
[хранилища](bins/kronika-collector/README.ru.md#хранилище).

Проектный предел коллектора — **менее 25 MiB peak RSS на обычной машине**;
[сценарий сбора в контейнере](crates/kronika-bdd/features/container.feature)
проверяет этот порог. При записи каждого сегмента коллектор выводит peak RSS
в поле `rss_kib`. Это не универсальная гарантия для любого числа процессов
или любой нагрузки PostgreSQL.

## Документация

- [Коллектор](bins/kronika-collector/README.ru.md) · [Web](bins/kronika-web/README.ru.md)
  · [MCP-клиенты](docs/mcp-clients.ru.md)
- [Демо](bins/kronika-demo/README.ru.md) · [Dump](bins/kronika-dump/README.ru.md)
  · [HTML-отчёты](bins/kronika-report/README.ru.md) · [Архивы релизов](docs/releases.ru.md)
- Записываемые поля: [Linux](docs/type-registry/os.ru.md),
  [метрики PostgreSQL](docs/type-registry/postgresql-metrics.ru.md),
  [события из логов PostgreSQL](docs/type-registry/postgresql.md),
  [события из логов PgBouncer](docs/type-registry/pgbouncer.md)
- [Формат сегмента](crates/kronika-format/README.ru.md)

Kronika — open source под [лицензией MIT](LICENSE).
