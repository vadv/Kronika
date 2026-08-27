# Подключение MCP-клиентов

`kronika-web` отдаёт MCP-эндпоинт на `POST /mcp` — Streamable HTTP, без
сессий, JSON-ответы, только инструменты. Аутентификация — те же
HTTP Basic-учётные данные, что у веб-интерфейса, заголовком
`Authorization` в каждом запросе. Обычный `http://` в локальной сети
работает во всех клиентах ниже. Эндпоинт отвергает запросы с заголовком
`Origin` и не маршрутизирует `/mcp` с query-строкой; сервер, запущенный
без настроенной учётной записи, отдаёт `/mcp` без пароля.

Подстановки в примерах:

- `<URL>` — эндпоинт, например `http://192.168.0.22:8080/mcp`
- `kronika` — имя регистрации. Для нескольких инстансов Kronika дайте
  каждому своё имя, иначе вторая регистрация заменит первую; панель
  подключения в веб-интерфейсе выводит имя из эндпоинта, например
  `kronika-192-168-0-22-8080`.
- `<USER>` / `<PASSWORD>` — учётные данные веб-интерфейса
- `<BASE64>` — `base64` от `<USER>:<PASSWORD>`:

```text
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

Значение Basic лежит в конфигах открытым текстом (а `claude mcp get`
печатает его обратно без маскировки). Не коммитьте конфиги с настоящими
учётными данными в публичные репозитории; у каждого клиента есть вариант
с переменной окружения.

## Claude Code

Проверено вживую на v2.1.228. Нативная поддержка удалённых HTTP-серверов —
с v1.0.27.

Одна команда, только текущий проект:

```text
claude mcp add --transport http kronika <URL> \
  --header "Authorization: Basic <BASE64>"
```

`--scope user` — для всех проектов. Проверка: `claude mcp list` — сервер
должен показать `✔ Connected`; при отказе `claude mcp get kronika`
называет HTTP-статус.

Общий файл проекта `<project-root>/.mcp.json` (интерактивная сессия один
раз спросит подтверждение):

```json
{
  "mcpServers": {
    "kronika": {
      "type": "http",
      "url": "<URL>",
      "headers": {
        "Authorization": "Basic <BASE64>"
      }
    }
  }
}
```

Поле `"type": "http"` обязательно: запись с `url` без `type`
пропускается с предупреждением, называющим недостающее поле. Чтобы не
класть учётные данные в файл, пишите
`"Authorization": "Basic ${KRONIKA_BASIC}"` и экспортируйте
`KRONIKA_BASIC=<BASE64>` перед запуском — `${VAR}` и `${VAR:-default}`
раскрываются внутри `url` и `headers`.

Промпт, которым работающая сессия Claude Code подключает себя сама —
подставьте три значения и вставьте текст в чат:

```text
Добавь MCP-сервер Kronika в мою конфигурацию Claude Code. Выполни:

claude mcp add --transport http --scope user kronika <URL> \
  --header "Authorization: Basic $(printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n')"

Затем выполни `claude mcp list` и убедись, что запись kronika
показывает "Connected". Инструменты появятся в следующей сессии;
точка входа — kronika_get_context.
```

Интерфейс custom-коннекторов Claude Desktop отвергает не-`https://` URL
(issue anthropics/claude-ai-mcp#9, закрыт как not planned) — там нужен
[мост mcp-remote](#запасной-путь-мост-mcp-remote).

## Codex CLI

Проверено вживую на codex-cli 0.147.0. Нативные `url`-серверы — с
rust-v0.45.0, `http_headers` появились вскоре после — в любой сборке 2026
года есть и то и другое. В `~/.codex/config.toml` (глобально) или в
`.codex/config.toml` доверенного проекта:

```toml
[mcp_servers.kronika]
url = "<URL>"
http_headers = { "Authorization" = "Basic <BASE64>" }
```

Чтобы не класть учётные данные в файл, читайте значение заголовка из
переменной окружения:

```toml
[mcp_servers.kronika]
url = "<URL>"
env_http_headers = { "Authorization" = "KRONIKA_AUTH" }
```

с `KRONIKA_AUTH="Basic <BASE64>"` в окружении, из которого запускается
Codex. `codex mcp add kronika --url <URL>` создаёт запись, но флага для
заголовков не имеет — допишите `http_headers` вручную.
`--bearer-token-env-var` не выражает Basic-аутентификацию: он всегда шлёт
`Bearer <token>`.

Промпт, которым работающая сессия Codex подключает себя сама —
подставьте значения и вставьте текст в чат:

```text
Добавь MCP-сервер Kronika в мою конфигурацию Codex: в
~/.codex/config.toml замени целиком таблицу [mcp_servers.kronika], если
она уже есть, иначе допиши

[mcp_servers.kronika]
url = "<URL>"
http_headers = { "Authorization" = "Basic <BASE64>" }

где <BASE64> — вывод команды:
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'

Затем выполни `codex mcp get kronika` и покажи мне разобранную запись.
Инструменты загрузятся в следующей сессии; точка входа —
kronika_get_context.
```

Проверка: `codex mcp get kronika` печатает разобранную запись с
замаскированным значением заголовка. В интерактивном TUI первый вызов
инструмента запрашивает подтверждение. Неинтерактивный `codex exec`
отменяет MCP-вызовы сам, независимо от настроек подтверждений
(openai/codex#29857); рабочий запуск, проверенный на живом демо, —
`codex exec --dangerously-bypass-approvals-and-sandbox`, который заодно
снимает песочницу с shell-команд модели. OAuth у этого сервера нет —
`codex mcp login` не применяется.

## Cursor

По официальной документации (сверено в августе 2026):
`<project-root>/.cursor/mcp.json` (проектный, общий) или
`~/.cursor/mcp.json` (глобальный); читаются оба файла. Streamable HTTP
Cursor выводит из ключа `url` — поле транспорта не нужно:

```json
{
  "mcpServers": {
    "kronika": {
      "url": "<URL>",
      "headers": {
        "Authorization": "Basic <BASE64>"
      }
    }
  }
}
```

Вариант с переменной окружения — `"Authorization": "Basic ${env:KRONIKA_BASIC}"`
с `KRONIKA_BASIC=<BASE64>` в окружении, из которого запущен Cursor.
MCP-серверы загружаются на старте — после правки файла перезапустите
Cursor, затем включите сервер тумблером на странице Customize в боковой
панели. Перед первым вызовом инструмента Cursor запросит подтверждение; у
рабочей настройки инструменты сервера перечислены в Available Tools.
Раздел следует официальной документации, живого прогона не было; у
headless `cursor-agent` есть описанные на форуме ошибки с игнорированием
auth-заголовков из `mcp.json`, поэтому документированный путь —
десктопный IDE.

Промпт для агента самого Cursor — подставьте значения и вставьте текст
в чат:

```text
Создай или обнови .cursor/mcp.json в этом проекте, чтобы он содержал:

{
  "mcpServers": {
    "kronika": {
      "url": "<URL>",
      "headers": {
        "Authorization": "Basic <BASE64>"
      }
    }
  }
}

где <BASE64> — вывод команды:
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'

Объедини с серверами, уже описанными в файле. Когда закончишь, напомни
мне перезапустить Cursor и включить kronika тумблером на странице
Customize; точка входа — kronika_get_context.
```

## Запасной путь: мост mcp-remote

Для клиентов без нативной поддержки заголовков поверх обычного HTTP
(сегодня — Claude Desktop) пакет
[mcp-remote](https://github.com/geelen/mcp-remote) превращает удалённый
сервер в локальный stdio. Нужен Node.js 18+:

```json
{
  "mcpServers": {
    "kronika": {
      "command": "npx",
      "args": [
        "mcp-remote",
        "<URL>",
        "--allow-http",
        "--transport", "http-only",
        "--header",
        "Authorization:${AUTH_HEADER}"
      ],
      "env": {
        "AUTH_HEADER": "Basic <BASE64>"
      }
    }
  }
}
```

`--allow-http` обязателен для не-HTTPS URL. В `Authorization:${AUTH_HEADER}`
нет пробела после двоеточия намеренно: несколько клиентов ломают аргументы
с пробелами при вызове `npx`, а косвенность через переменную окружения это
обходит.
