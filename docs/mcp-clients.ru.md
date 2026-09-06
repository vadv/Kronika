# Подключение MCP-клиентов

[English version](mcp-clients.md)

`kronika-web` предоставляет `POST /mcp`: Streamable HTTP без сессий, JSON-ответы, только инструменты. При включённой аутентификации каждый запрос содержит `Authorization: Basic <BASE64>` с учётными данными веб-интерфейса. `KRONIKA_WEB_AUTH=disabled` отключает проверку; `KRONIKA_WEB_USER` и `KRONIKA_WEB_PASSWORD` остаются обязательными. Запросы с `Origin` и URL со строкой параметров отклоняются.

## Параметры подключения

| Значение | Определение |
| --- | --- |
| `<URL>` | URL эндпоинта, например `http://127.0.0.1:8080/mcp`. |
| `kronika` | Имя сервера в конфигурации клиента. |
| `<USER>`, `<PASSWORD>` | Значения `KRONIKA_WEB_USER`, `KRONIKA_WEB_PASSWORD`. |
| `<BASE64>` | Base64-кодирование строки `<USER>:<PASSWORD>` без перевода строки. |

```bash
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

Панель **Connect an AI agent** формирует конфигурацию для выбранного клиента. Имя регистрации содержит нормализованные имя крупнейшей записанной базы и адрес эндпоинта, например `kronika-billing-192-168-0-22-8080`. При отключённой аутентификации заголовок пропускается.

## Claude Code

Регистрация на уровне пользователя:

```bash
claude mcp add --transport http --scope user kronika '<URL>' \
  --header 'Authorization: Basic <BASE64>'
```

Конфигурация проекта в `.mcp.json`:

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

## Codex CLI

Запись в `~/.codex/config.toml` или `.codex/config.toml` доверенного проекта:

```toml
[mcp_servers.kronika]
url = "<URL>"
http_headers = { "Authorization" = "Basic <BASE64>" }
```

## Cursor

Запись в `.cursor/mcp.json` проекта или `~/.cursor/mcp.json`:

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

Состав инструментов, типы параметров, единицы и результаты: [справочник MCP](features.ru.md#mcp). Источники конфигураций: [генератор](../bins/kronika-web/ui/src/mcp-prompts.ts), [панель подключения](../bins/kronika-web/ui/src/mcp-connect.tsx).
