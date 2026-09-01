# kronika-web

[English version](README.md)

`kronika-web` показывает сохранённые данные Kronika в браузере и отдаёт их
через HTTP API и MCP. Он читает `active.wal` и готовые сегменты `.zms` через
`kronika-reader`, а также создаёт производные файлы `.idx` в каталоге данных.
Сервер настраивается только переменными окружения.

## Настройка

Если обязательной переменной нет или значение неверно, сервер завершается до
открытия сетевого сокета.

| Переменная | По умолчанию | Описание |
| --- | ---: | --- |
| `KRONIKA_STORAGE_DIR` | — | Обязательный каталог данных. В нём должны находиться результаты работы коллектора; web должен иметь право создавать и заменять файлы `.idx` и `.kronika-index.owner.lock`. |
| `KRONIKA_WEB_LISTEN` | `127.0.0.1:8080` | Адрес, на котором HTTP-сервер принимает соединения. |
| `KRONIKA_WEB_SOURCES` | — | Обязательное десятичное число. Бит 0 включает данные OS, бит 1 — PostgreSQL. Остальные биты недопустимы. |
| `KRONIKA_WEB_USER` | — | Обязательное непустое имя пользователя Basic, в том числе при отключённой аутентификации. |
| `KRONIKA_WEB_PASSWORD` | — | Обязательный непустой пароль Basic, в том числе при отключённой аутентификации. |
| `KRONIKA_WEB_AUTH` | `required` | `required` защищает `/api/*` и `/mcp`; `disabled` отключает проверку учётных данных. |
| `KRONIKA_WEB_DEMO` | не задана | Допустимо только значение `synthetic`. Оно помечает ответы и интерфейс как синтетические; данные по-прежнему читаются из `KRONIKA_STORAGE_DIR`. |

## Запуск

Установите `rustup` и `musl-gcc`, затем из корня репозитория выполните:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked -p kronika-web

sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
KRONIKA_WEB_SOURCES=1 \
KRONIKA_WEB_USER=kronika \
KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
target/x86_64-unknown-linux-musl/release/kronika-web
```

Каталог данных должен существовать до запуска. После успешного старта процесс
печатает `ready <addr>` и начинает принимать запросы HTTP/1.1.

Откройте <http://127.0.0.1:8080/> и войдите как `kronika` с заданным паролем.

В `KRONIKA_WEB_SOURCES` включите биты данных, которые должен показывать web:
бит 0 для OS и бит 1 для PostgreSQL. Для обоих включите оба бита.

При `KRONIKA_WEB_AUTH=required` защищённые запросы принимают учётные данные
Basic или сессионный cookie, созданный запросом `POST /auth/session`.

## Эндпоинты

- `/` — встроенный веб-интерфейс; принимает `GET` и `HEAD`.
- `/auth/session` — проверяет сессию браузера запросом `GET`, создаёт её из
  учётных данных Basic запросом `POST` и удаляет запросом `DELETE`. Для HTTPS
  браузерная сессия автоматически использует cookie с `Secure`.
- `/api/*` — ресурсы JSON и NDJSON для веб-интерфейса; принимают `GET`.
- `/mcp` — MCP через Streamable HTTP без серверных сессий; принимает `POST`.
  Запросы со строкой параметров или заголовком `Origin` отклоняются. MCP читает
  сохранённые данные Kronika. См.
  [настройку MCP-клиентов](../../docs/mcp-clients.ru.md).
