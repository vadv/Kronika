# Connecting MCP clients

`kronika-web` serves an MCP endpoint at `POST /mcp` — Streamable HTTP,
stateless, JSON responses, tools only. It authenticates with the same HTTP
Basic credentials as the web UI, sent as an `Authorization` header on every
request. Plain `http://` on a LAN works in every client below. The
endpoint rejects requests carrying an `Origin` header and does not route
`/mcp` with a query string; a server started without a configured account
serves `/mcp` without credentials.

Placeholders used throughout:

- `<URL>` — the endpoint, e.g. `http://192.168.0.22:8080/mcp`
- `kronika` — the registration name. With several Kronika instances, give
  each its own name, or the second registration replaces the first; the
  web UI's connection panel derives one from the largest recorded
  database and the endpoint, e.g. `kronika-billing-192-168-0-22-8080`.
  When another database outgrows the current one, the derived name
  changes and a re-run prompt adds a new registration — remove the
  stale entry yourself.
- `<USER>` / `<PASSWORD>` — the web UI credentials
- `<BASE64>` — `base64` of `<USER>:<PASSWORD>`:

```text
printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\n'
```

The Basic value sits in plaintext in every config below (and `claude mcp
get` prints it back unmasked). Keep configs with real credentials out of
public repositories; each client has an environment-variable form for
that.

For a localized, ready-to-paste setup prompt, open the MCP panel in
`kronika-web`'s top bar and choose the client there.

## Claude Code

Verified live on v2.1.228. Native remote HTTP support exists since v1.0.27.

One command, current project only:

```text
claude mcp add --transport http kronika <URL> \
  --header "Authorization: Basic <BASE64>"
```

Add `--scope user` for all projects. Check with `claude mcp list` — the
server must show `✔ Connected`; on failure `claude mcp get kronika` names
the HTTP status.

Shareable project file `<project-root>/.mcp.json` (interactive sessions ask
once to approve it):

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

The `"type": "http"` field is mandatory: an entry with `url` but no
`type` is skipped with a warning naming the missing field. To keep the
credential out of the file, write
`"Authorization": "Basic ${KRONIKA_BASIC}"` and export
`KRONIKA_BASIC=<BASE64>` before launching — `${VAR}` and `${VAR:-default}`
expand inside `url` and `headers`.

Claude Desktop's custom-connector UI rejects non-`https://` URLs (issue
anthropics/claude-ai-mcp#9, closed as not planned) — use the
[mcp-remote bridge](#fallback-the-mcp-remote-bridge) there.

## Codex CLI

Verified live on codex-cli 0.147.0. Native `url` servers exist since
rust-v0.45.0, `http_headers` shortly after — every 2026 build has both. In
`~/.codex/config.toml` (global) or a trusted project's `.codex/config.toml`:

```toml
[mcp_servers.kronika]
url = "<URL>"
http_headers = { "Authorization" = "Basic <BASE64>" }
```

To keep the credential out of the file, read the header value from an
environment variable instead:

```toml
[mcp_servers.kronika]
url = "<URL>"
env_http_headers = { "Authorization" = "KRONIKA_AUTH" }
```

with `KRONIKA_AUTH="Basic <BASE64>"` exported in the environment Codex is
launched from. `codex mcp add kronika --url <URL>` scaffolds the entry but
has no header flag — add `http_headers` by hand. `--bearer-token-env-var`
cannot express Basic auth: it always sends `Bearer <token>`.

Check with `codex mcp get kronika` — it prints the parsed entry with the
header value masked. In the interactive TUI, the first tool call asks for
approval. Non-interactive `codex exec` auto-cancels MCP tool calls
regardless of the approval settings (openai/codex#29857); the invocation
that works — verified against the live demo — is
`codex exec --dangerously-bypass-approvals-and-sandbox`, which also drops
the sandbox around model-generated shell commands. This server has no
OAuth — `codex mcp login` does not apply.

## Cursor

Per the official docs (checked August 2026): `<project-root>/.cursor/mcp.json`
(project, shareable) or `~/.cursor/mcp.json` (global); both files are
read. Cursor infers Streamable HTTP from the `url` key — no transport
field:

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

The environment-variable form is `"Authorization": "Basic ${env:KRONIKA_BASIC}"`
with `KRONIKA_BASIC=<BASE64>` exported in the shell Cursor is launched
from. MCP servers load at startup — restart Cursor after editing the
file, then enable the server with its toggle on the Customize page in the
sidebar. Cursor asks for approval before the first tool call; a
working setup lists the server's tools under Available Tools. This section
follows the official documentation, not a live run; the headless
`cursor-agent` CLI has forum-reported bugs ignoring `mcp.json` auth
headers, so the desktop IDE is the documented path.

## Fallback: the mcp-remote bridge

For clients without native header support over plain HTTP (Claude Desktop
today), the [mcp-remote](https://github.com/geelen/mcp-remote) package
bridges a remote server into a local stdio one. Requires Node.js 18+:

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

`--allow-http` is mandatory for non-HTTPS URLs. `Authorization:${AUTH_HEADER}`
has no space after the colon on purpose: several clients mangle argument
strings containing spaces when invoking `npx`, and the environment-variable
indirection sidesteps that.
