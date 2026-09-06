# kronika-web

[English version](README.md) · [Установка](../../INSTALL.ru.md)

Web читает `active.wal` и готовые `.zms` через `kronika-reader`. Он обслуживает
встроенный browser interface, HTTP API и MCP и пишет derived `.idx` в каталог
записи.

## Конфигурация

Environment проверяется до открытия listener.
Исходник: [config.rs](src/config.rs).

| Переменная | По умолчанию | Допустимое значение и смысл |
| --- | --- | --- |
| `KRONIKA_STORAGE_DIR` | Обязательна | Существующий обычный корень хранения collector; read/write access для `.idx` и `.kronika-index.owner.lock`. |
| `KRONIKA_WEB_LISTEN` | `127.0.0.1:8080` | IP address и port, включая IPv6 в виде `[::1]:8080`. Plain HTTP. |
| `KRONIKA_WEB_SOURCES` | Обязательна | Decimal bitset `0..3`: bit 0 отмечает OS configured; bit 1 — PostgreSQL configured. `0` ни одного, `1` OS, `2` PostgreSQL, `3` оба. |
| `KRONIKA_WEB_USER` | Обязательна | Непустой user name, обязателен и при отключённой authentication. |
| `KRONIKA_WEB_PASSWORD` | Обязательна | Непустой password, обязателен и при отключённой authentication. |
| `KRONIKA_WEB_AUTH` | `required` | `required` проверяет credentials/session; `disabled` разрешает доступ без аутентификации. |
| `KRONIKA_WEB_DEMO` | Не задана | Единственное значение: `synthetic`; помечает каталог и интерфейс как синтетическую запись. |
| `TMPDIR` | Системный временный каталог, обычно `/tmp` | Доступная на запись filesystem location для временных файлов export. |

Source bitset задаёт поля `configured` каталога. В браузере PostgreSQL bit
подавляет его no-data tooltip; записанные PostgreSQL data также подавляют его.
OS bit остаётся метаданными каталога. Все tabs и записанные sections доступны.
Recorded health использует метаданные collector.

## Запуск

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

При запуске stdout получает `ready <addr>`. Откройте <http://127.0.0.1:8080/>
и войдите с заданным account. API и MCP принимают HTTP Basic credentials;
защищённые API requests также принимают browser session cookie.

## Endpoints

| Route | Methods | Контракт |
| --- | --- | --- |
| `/` | `GET`, `HEAD` | Встроенный browser interface. |
| `/auth/session` | `GET`, `POST`, `DELETE` | Проверить, создать из Basic credentials или удалить browser session. Cookies получают `Secure` при HTTPS. |
| `/api/export?from=<unix_second>&to=<unix_second>` | `GET` | HTML attachment с аутентификацией за inclusive whole-second bounds. |
| Другие `/api/*` | `GET` | JSON/NDJSON resources записанных данных. |
| `/mcp` | `POST` | Stateless Streamable HTTP; та же authentication. Query strings и `Origin` headers отклоняются. [Справочник MCP](../../docs/mcp-clients.ru.md). |

## Файлы export

Export создаёт два временных файла: срез ZMS и файл, используемый сначала для
scratch data среза, затем для готового HTML. Оба существуют одновременно и
удаляются при закрытии. Service account нужны доступ на запись и место для
обоих. В ограниченном systemd unit можно задать:

```ini
[Service]
Environment=TMPDIR=/var/tmp/kronika-web
ReadWritePaths=/var/tmp/kronika-web
```

Создайте этот каталог для service account при
[настройке сервиса](../../docs/services.ru.md). Подготовка запросов ограничена
одним export одновременно на процесс. Исходники: [export.rs](src/export.rs),
[config.rs](src/config.rs).

## Process interface

`-h`, `--help` и `--version` пишут в stdout и завершаются до конфигурации,
обращения к хранилищу и запуска listener. Ошибки request, connection и export,
а также timings export идут в stderr; у web нет log-level setting. `Ctrl+C`
или `SIGTERM` завершает web. Ошибки startup/configuration дают ненулевой exit status.
