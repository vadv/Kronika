# MCP client configuration

[Русская версия](mcp-clients.ru.md)

`kronika-web` exposes `POST /mcp`: stateless Streamable HTTP, JSON responses, tools only. With authentication enabled, every request carries `Authorization: Basic <BASE64>` using the web interface credentials. `KRONIKA_WEB_AUTH=disabled` disables verification; `KRONIKA_WEB_USER` and `KRONIKA_WEB_PASSWORD` remain required. Requests carrying `Origin` and URLs with a query string are rejected.

## Connection parameters

| Value | Definition |
| --- | --- |
| `<URL>` | Endpoint URL, for example `http://127.0.0.1:8080/mcp`. |
| `kronika` | Server name in the client configuration. |
| `<USER>`, `<PASSWORD>` | Values of `KRONIKA_WEB_USER`, `KRONIKA_WEB_PASSWORD`. |
| `<BASE64>` | Base64 encoding of `<USER>:<PASSWORD>` without a trailing newline. |

```bash
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

The **Connect an AI agent** panel generates configuration for the selected client. The registration name contains the normalized largest recorded database name and endpoint address, for example `kronika-billing-192-168-0-22-8080`. With authentication disabled, the header is omitted.

## Claude Code

User-level registration:

```bash
claude mcp add --transport http --scope user kronika '<URL>' \
  --header 'Authorization: Basic <BASE64>'
```

Project configuration in `.mcp.json`:

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

Entry in `~/.codex/config.toml` or a trusted project's `.codex/config.toml`:

```toml
[mcp_servers.kronika]
url = "<URL>"
http_headers = { "Authorization" = "Basic <BASE64>" }
```

## Cursor

Entry in the project's `.cursor/mcp.json` or `~/.cursor/mcp.json`:

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

Tool inventory, parameter types, units and results: [MCP reference](features.md#mcp). Configuration sources: [generator](../bins/kronika-web/ui/src/mcp-prompts.ts), [connection panel](../bins/kronika-web/ui/src/mcp-connect.tsx).
