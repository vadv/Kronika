# Kronika

[English version](README.md)

Kronika сохраняет периодические снимки состояния Linux-машины и работающих на
ней баз данных для последующего просмотра. Коллектор читает метрики
операционной системы и PostgreSQL, а также разбирает журналы PostgreSQL и
PgBouncer. `kronika-web` показывает сохранённые данные в браузере и отдаёт их
через MCP.

![Архитектура Kronika](docs/images/architecture.svg)

Коллектор пишет текущий журнал в `active.wal`, а готовые сегменты — в
`YYYY/MM/DD/N.zms` внутри `KRONIKA_OUT_DIR`. `kronika-web` читает эти файлы и
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
cargo build --release --locked -p kronika-collector -p kronika-web
```

Запустите коллектор. Переменная `KRONIKA_PG_DSNS` включает сбор данных
PostgreSQL; без неё коллектор записывает только данные машины.

```sh
sudo env KRONIKA_OUT_DIR=/var/lib/kronika \
target/x86_64-unknown-linux-musl/release/kronika-collector
```

В другом терминале запустите web с тем же каталогом данных.

```sh
sudo env KRONIKA_OUT_DIR=/var/lib/kronika \
KRONIKA_WEB_SOURCES=1 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
target/x86_64-unknown-linux-musl/release/kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите как `kronika` с заданным паролем.
Для доступа по сети используйте обратный прокси с TLS, если только вы явно не
доверяете этой сети, и задайте `KRONIKA_WEB_COOKIE_SECURE=true`. Если коллектор
записывает PostgreSQL, укажите `KRONIKA_WEB_SOURCES=3`.

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

- [DESIGN.ru.md](DESIGN.ru.md) — архитектура и правила продукта.
- [bins/kronika-demo/README.ru.md](bins/kronika-demo/README.ru.md) — команды
  демо.
- [docs/mcp-clients.ru.md](docs/mcp-clients.ru.md) — настройка MCP-клиентов.
- [docs/type-registry/](docs/type-registry/) — записываемые секции и поля:
  [OS](docs/type-registry/os.ru.md),
  [PostgreSQL](docs/type-registry/postgresql.md),
  [метрики PostgreSQL](docs/type-registry/postgresql-metrics.ru.md) и
  [PgBouncer](docs/type-registry/pgbouncer.md).
- [crates/kronika-format/README.ru.md](crates/kronika-format/README.ru.md) —
  формат сегмента.

Команда `make diagrams` пересоздаёт SVG с помощью CLI draw.io, доступного как
`drawio`. Другой путь можно передать через `DRAWIO=/путь/к/drawio`.

Kronika распространяется по лицензии MIT.
