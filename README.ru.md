# Kronika

[English version](README.md)

Kronika записывает историю машины и работающих на ней баз данных — так же,
как `atop` записывает историю системы, — и потом воспроизводит её. Коллектор
периодически снимает метрики операционной системы и PostgreSQL, разбирает
логи и преобразует значимые события из логов в метрики. Web читает записанное и
показывает его людям в браузере и LLM-клиентам через MCP.

![Архитектура Kronika](docs/images/architecture.svg)

Коллектор работает на наблюдаемой машине постоянно; его пиковый RSS остаётся
ниже 25 MiB, и каждая запись сегмента логирует измеренный `rss_kib`. Всё
собранное попадает в один каталог: окна дописываются в `active.wal`, а журнал
записывается сегментами с именами `YYYY/MM/DD/N.zms`. Каждый сегмент
независим и самодостаточен: чтобы открыть его, не нужны другие файлы, внешняя
схема или обращение к реестру. `kronika-web` открывает тот же каталог только
на чтение — готовые сегменты плюс корректный префикс `active.wal`, всё через
единственный крейт `kronika-reader`.

## Попробовать демо

Нужен Docker с Compose v2.

```sh
make demo-up
```

Команда собирает один контейнер с PostgreSQL 15, PgBouncer, генератором
нагрузки, коллектором и web и ждёт, пока он станет здоровым. Откройте
<http://127.0.0.1:8080/> и войдите как `demo` / `forensics`. `make demo-stop`
сохраняет собранные сегменты; `make demo-clean` удаляет контейнеры и данные.
Подробности: [bins/kronika-demo/README.ru.md](bins/kronika-demo/README.ru.md).

## Запустить на своей машине

Сборка закреплённым тулчейном Rust (1.96.0, выбирается автоматически через
`rust-toolchain.toml`). npm не нужен: `kronika-web` собирается из
закоммиченного артефакта интерфейса.

```sh
cargo build --release --locked -p kronika-collector -p kronika-web
```

По умолчанию workspace собирает статические бинарники x86-64 Linux
(`.cargo/config.toml`; нужен `musl-gcc`), поэтому бинарники попадают в
`target/x86_64-unknown-linux-musl/release/`. Чтобы собрать под текущую
машину, добавьте `--target "$(rustc -vV | sed -n 's/^host: //p')"`.

Запустите коллектор. `KRONIKA_OUT_DIR` — единственная обязательная
переменная; добавьте `KRONIKA_PG_DSNS`, чтобы записывать PostgreSQL:

```sh
KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_PG_DSNS='host=/var/run/postgresql user=postgres' \
kronika-collector
```

Запустите web на том же каталоге:

```sh
KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
KRONIKA_WEB_SOURCES=3 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD=secret \
kronika-web
```

Каждая переменная, интервал и лимит описаны в
[bins/kronika-collector/README.ru.md](bins/kronika-collector/README.ru.md) и
[bins/kronika-web/README.ru.md](bins/kronika-web/README.ru.md).

## Подключить LLM

`kronika-web` отдаёт MCP на `POST /mcp`: Streamable HTTP без сессий, те же
Basic-учётные данные, что у интерфейса, четырнадцать инструментов, которые
читают записанное и никогда не обращаются к наблюдаемой машине. Панель MCP в
верхней панели веб-интерфейса выдаёт готовый к вставке промпт настройки;
[docs/mcp-clients.ru.md](docs/mcp-clients.ru.md) описывает Claude Code,
Codex CLI, Cursor и мост `mcp-remote`.

## Документация

- [DESIGN.ru.md](DESIGN.ru.md) — что такое Kronika и по каким правилам она
  строится.
- [bins/kronika-collector/README.ru.md](bins/kronika-collector/README.ru.md) —
  настройка и эксплуатация коллектора.
- [bins/kronika-web/README.ru.md](bins/kronika-web/README.ru.md) — настройка
  web, эндпоинты, MCP.
- [bins/kronika-demo/README.ru.md](bins/kronika-demo/README.ru.md) —
  демо-образ и бинарник `kronika-demo`.
- [docs/mcp-clients.ru.md](docs/mcp-clients.ru.md) — настройка MCP-клиентов.
- [docs/type-registry/](docs/type-registry/) — каждая записываемая секция и
  поле: [OS](docs/type-registry/os.ru.md),
  [PostgreSQL](docs/type-registry/postgresql.md),
  [метрики PostgreSQL](docs/type-registry/postgresql-metrics.ru.md),
  [PgBouncer](docs/type-registry/pgbouncer.md).
- [crates/kronika-format/README.ru.md](crates/kronika-format/README.ru.md) —
  бинарный формат сегмента.

Диаграммы генерируются: исходники лежат в [docs/diagrams/](docs/diagrams/),
`make diagrams` пересобирает закоммиченные SVG (нужен
[d2](https://d2lang.com)).

Лицензия MIT.
