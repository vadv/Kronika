# Kronika

[English version](README.md)

**Вернитесь к моменту замедления Linux и PostgreSQL.** Kronika записывает
процессы, использование ресурсов, активность базы, SQL-запросы, планы выполнения
и события из логов. Найдите, когда изменились загрузка CPU, дисковый I/O
или активность запросов. Выберите процесс, запрос или план и посмотрите,
что происходило в тот момент.

**Экспортируйте свои данные в один интерактивный HTML-файл.** В работающей
Kronika выберите интервал и нажмите **«Экспорт»**. Отправьте скачанный `.html`
коллеге: он откроет его в браузере без установки Kronika, сервера и подключения
к интернету. Таблицы, heatmap, поиск и графики останутся интерактивными.

**[Открыть интерактивное демо →](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html)**
Попробуйте такой HTML-отчёт на часовой записи синтетической нагрузки Linux
и PostgreSQL: 5 сентября 2026 года, 19:00–20:00 UTC.
Установка и вход не нужны; сохраните файл, чтобы открыть его без сети.

![Processes: heatmap нагрузки на CPU над таблицей процессов](docs/images/processes.png)

*Processes в синтетическом демо. Найдите нагруженный интервал на heatmap,
затем выберите процесс, чтобы изучить историю его CPU, памяти и I/O.*

## Попробовать локально

Для работающего демо нужен Docker с Compose v2 на Linux amd64 или arm64.
Команды выбирают текущую ветку для ревью с описанными здесь возможностями
и запускают демо из корня репозитория:

```sh
git clone --branch fix/events-count-scope https://github.com/vadv/Kronika.git kronika
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

## Найти запросы и планы за нужный интервал

**Statements: найти SQL, на который приходилась нагрузка.** Сортируйте запросы
по нагрузке выполнения, вызовам, активности buffers или WAL. Выберите строку,
чтобы прочитать SQL и увидеть, как менялась его активность за этот интервал.

![Statements: heatmap активности запросов, текст SQL и история в синтетическом демо](docs/images/statements.png)

**Plans: посмотреть, как выполнялся запрос.** Откройте записанные планы по
Query ID, сравните метрики выполнения и прочитайте выбранный план рядом с SQL.
Statements и Plans используют историю `pg_stat_statements` и `pg_store_plans`,
если эти расширения установлены.

![Plans: записанный план выполнения и связанный SQL в синтетическом демо](docs/images/plans.png)

*Выбранный план и SQL взяты из того же часа. В этой записи Plans Activity
недоступна из-за повторяющихся идентификаторов одного записанного плана;
таблица и Inspector работают.*

За тот же период можно посмотреть **ожидания backends и блокирующие процессы**,
**ход Vacuum**, **активность таблиц и индексов**, **события из логов PostgreSQL**.
В Linux доступны **история дисков, сети, памяти и CPU**, а в контейнерах —
**потребление ресурсов cgroup, лимиты и throttling**.

## Записать историю своей машины

Запустите коллектор на своей Linux-машине и укажите web каталог с записью.
Kronika хранит историю в локальных файлах; web предоставляет доступ к этой
записи через браузер и MCP.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-ru-dark.svg">
  <img alt="Linux и PostgreSQL передают данные коллектору; web читает запись для браузера и MCP-клиентов" src="docs/images/architecture-ru.svg">
</picture>

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
описаны отдельно для существующего релиза и сборки архива из исходного кода.

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
если нужна только запись Linux. Установленные расширения обнаруживаются автоматически,
логи должны быть доступны локально. Доступ к базам, расширения, пути к логам
и интервалы сбора описаны в
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

## Экспортировать интервал из командной строки

Кнопка **«Экспорт»** — самый короткий путь к отправке своей записи коллеге.
Для готовой записи или экспорта из скрипта создайте HTML-файл командами
`kronika-dump slice` и `kronika-report`.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/report-export-ru-dark.svg">
  <img alt="Экспорт интервала из web или готовой записи в один HTML-файл для просмотра без сети и отправки коллеге" src="docs/images/report-export-ru.svg">
</picture>

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

Slice принимает целые секунды в RFC 3339 с обеими включёнными границами.
Параметры report задают **19:00–20:00 UTC** в Unix-микросекундах и исключают
из просмотра соседние отсчёты, сохранённые для вычислений. Без этих параметров
report показывает весь временной диапазон входного файла. Форматы и параметры
вывода описаны в руководствах [dump](bins/kronika-dump/README.ru.md)
и [report](bins/kronika-report/README.ru.md).

## Хранение и реализация

В замере нагрузки с **примерно 500 таблицами и 3 тыс. индексов** объём сжатой
записи составил около **184 MB в сутки**: пересчёт по 43 готовым сегментам
со средним размером 1,92 MB и интервалом записи 15 минут. Цель retention
по умолчанию — **2 GiB** для записанных данных, включая журналы и индексы.
Замер и настройки retention приведены в разделе
[хранилища](bins/kronika-collector/README.ru.md#хранилище).

Проектный предел коллектора — **менее 25 MiB пиковой памяти на обычной машине**;
при записи каждого сегмента он выводит пиковое потребление памяти в лог.
Коллектор и движок запросов написаны на Rust, интерфейс — на React.
HTML-отчёт содержит данные, интерфейс и движок запросов WebAssembly,
который работает в основном потоке браузера.
Подробности — в [руководстве по HTML-отчётам](bins/kronika-report/README.ru.md).

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
