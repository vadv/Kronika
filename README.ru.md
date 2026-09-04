# Kronika

[English version](README.md)

Kronika сохраняет периодические снимки состояния Linux-машины и работающих на
ней баз данных для последующего просмотра. Коллектор читает метрики
операционной системы и PostgreSQL, а также разбирает журналы PostgreSQL и
PgBouncer. `kronika-web` показывает сохранённые данные в браузере и отдаёт их
через MCP. `kronika-report` превращает один готовый сегмент в самодостаточный
HTML-файл, который открывается без сервера.

Откройте [часовой отчёт с синтетическими данными](https://vadv.github.io/Kronika/).
Workflow для Pages создаёт полный штатный отчёт в одном самодостаточном
HTML-файле без внешних ресурсов.

![Архитектура Kronika](docs/images/architecture.svg)

Коллектор пишет текущий журнал в `active.wal`, а готовые сегменты — в
`YYYY/MM/DD/N.zms` внутри `KRONIKA_STORAGE_DIR`. `kronika-web` читает эти файлы и
создаёт рядом производные файлы `N.idx`.

## Запуск демо

Для демо нужен Docker с Compose v2 на Linux amd64 или arm64.

```sh
make demo-up
```

Откройте <http://127.0.0.1:8080/> и войдите с именем `demo` и паролем
`forensics`. `make demo-stop` сохраняет собранные данные, а `make demo-clean`
удаляет контейнеры и данные демо. Подробности приведены в
[bins/kronika-demo/README.ru.md](bins/kronika-demo/README.ru.md).

## Сборка и запуск из исходного кода

По умолчанию собираются статические бинарные файлы для Linux x86-64. Для
сборки нужны `rustup` и `musl-gcc`. Версия Rust 1.96.0 задана в
`rust-toolchain.toml`.

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked -p kronika-collector -p kronika-dump -p kronika-report -p kronika-web
```

## Создание временного среза

`kronika-dump slice` читает хранилище из `KRONIKA_STORAGE_DIR`. Обе границы
задаются в RFC 3339 с точностью до целой секунды и включаются в диапазон.
Команда пишет точно в путь `.zms`, переданный через `--out`, и не
перезаписывает существующий файл.

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika \
target/x86_64-unknown-linux-musl/release/kronika-dump slice \
  --from 2024-02-29T00:00:00Z \
  --to 2024-02-29T00:59:59Z \
  --out incident.zms
```

Результат — один готовый самостоятельный ZMS. Он может содержать до 30 секунд
контекста для вычислений до и после запрошенного интервала. Если в диапазоне
нет записанных строк, команда завершается с ошибкой. После проверки готового
файла она выводит запрошенные и фактические границы, число строк и секций, а
также размер в байтах. Библиотечная цель пакета предоставляет тот же код среза
для вызова внутри процесса; вызывающий код передаёт собственный дисковый
рабочий файл и приёмник вывода. CLI размещает рабочий файл в файловой системе
выходного файла.

## Создание автономного отчёта

Команда принимает один готовый ZMS с любым именем и суффиксом `.zms`, а также
один выходной путь с суффиксом `.html`. Внутренний идентификатор сегмента
вычисляется из проверенного каталога ZMS:

```sh
target/x86_64-unknown-linux-musl/release/kronika-report \
  /path/to/incident.zms \
  report.html
```

Команда строит изолированный канонический IDX и атомарно заменяет выходной файл
одним детерминированным документом. В документ входят рабочий интерфейс,
WebAssembly-движок запросов, ZMS и IDX. Ему не нужны корень хранилища,
предыдущий сегмент, сервер, внешние дополнительные файлы, аутентификация, MCP,
живое обновление или сетевые запросы. Первое значение скорости, которому нужен
более ранний отсчёт, равно `null`. Подробности приведены в
[bins/kronika-report/README.md](bins/kronika-report/README.md).
Библиотечная цель пакета также пишет такой документ в переданный при вызове
приёмник из байтов ZMS и явного `SegmentId`; CLI локально вычисляет этот
идентификатор из проверенного каталога входного файла.

Создайте учётную запись PostgreSQL, которая используется ниже:

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE kronika_monitor LOGIN PASSWORD 'replace-with-password';
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
SQL
```

Эта роль может читать основные метрики PostgreSQL, находить базы данных,
проверять установленные расширения `pg_stat_statements` и `pg_store_plans` и
определять текущий журнал PostgreSQL. Этот пример рассчитан на стандартные
права доступа к `pg_catalog`. Ограниченные права на базы и объекты расширений
описаны в разделе [Роль
PostgreSQL](bins/kronika-collector/README.ru.md#роль-postgresql).

Запустите коллектор. Этот пример собирает данные PostgreSQL и машины; чтобы
собирать только данные машины, уберите строку `KRONIKA_PG_DSNS`.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
KRONIKA_RETENTION=2147483648 \
KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
target/x86_64-unknown-linux-musl/release/kronika-collector
```

`KRONIKA_RETENTION=2147483648` задаёт фиксированный бюджет хранения 2 GiB;
фиксированный и автоматический режимы описаны в [настройках хранилища
коллектора](bins/kronika-collector/README.ru.md#хранилище).

В одном замере с примерно 500 таблицами и 3 тыс. индексов Kronika записывала
около 184 МБ в сутки. Поэтому 2 ГиБ хватает примерно на 11 суток; `active.wal` и
файлы `.idx` входят в тот же бюджет.

В другом терминале запустите web с тем же каталогом данных.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
KRONIKA_WEB_SOURCES=3 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
target/x86_64-unknown-linux-musl/release/kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите как `kronika` с заданным паролем.
`KRONIKA_WEB_SOURCES` объявляет семейства источников, которые web-каталог
помечает как настроенные: `0` — ни одного, `1` — OS, `2` — PostgreSQL, `3` —
оба семейства. Эта настройка не включает сбор и не фильтрует сохранённые
данные. Коллектор читает метрики PostgreSQL, только если `KRONIKA_PG_DSNS`
содержит хотя бы один DSN.

Настройка коллектора и web:

- [bins/kronika-collector/README.ru.md](bins/kronika-collector/README.ru.md)
- [bins/kronika-web/README.ru.md](bins/kronika-web/README.ru.md)

## MCP

`kronika-web` отдаёт MCP на `POST /mcp` и использует те же учётные данные Basic,
что и API. MCP читает сохранённые данные Kronika: он не подключается к
PostgreSQL и не читает текущее состояние машины. Настройка Claude Code, Codex
CLI, Cursor и `mcp-remote` описана в
[docs/mcp-clients.ru.md](docs/mcp-clients.ru.md).

## Документация

- [bins/kronika-demo/README.ru.md](bins/kronika-demo/README.ru.md) — команды
  демо.
- [bins/kronika-report/README.md](bins/kronika-report/README.md) — автономные
  HTML-отчёты.
- [docs/mcp-clients.ru.md](docs/mcp-clients.ru.md) — настройка MCP-клиентов.
- [docs/type-registry/](docs/type-registry/) — записываемые секции и поля:
  [OS](docs/type-registry/os.ru.md),
  [PostgreSQL](docs/type-registry/postgresql.md),
  [метрики PostgreSQL](docs/type-registry/postgresql-metrics.ru.md) и
  [PgBouncer](docs/type-registry/pgbouncer.md).
- [crates/kronika-format/README.ru.md](crates/kronika-format/README.ru.md) —
  формат сегмента.

Kronika распространяется по лицензии MIT.
