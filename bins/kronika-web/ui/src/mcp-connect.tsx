import { Copy } from "lucide-react"

import type { Translate } from "./help"

// The endpoint URL comes from the page's own origin; credentials stay
// placeholders — after sign-in the page holds only the HttpOnly session
// cookie, nothing it could substitute — and the operator replaces them
// before pasting. The command and config fragments mirror
// docs/mcp-clients.md and stay identical in both locales.
function claudePrompt(url: string, t: Translate): string {
  return [
    t("mcp.prompt.claude.intro"),
    "",
    `claude mcp add --transport http --scope user kronika ${url} \\`,
    `  --header "Authorization: Basic $(printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\\n')"`,
    "",
    t("mcp.prompt.claude.verify"),
  ].join("\n")
}

function codexPrompt(url: string, t: Translate): string {
  return [
    t("mcp.prompt.codex.intro"),
    "",
    "[mcp_servers.kronika]",
    `url = "${url}"`,
    'http_headers = { "Authorization" = "Basic <BASE64>" }',
    "",
    t("mcp.prompt.base64"),
    "printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\\n'",
    "",
    t("mcp.prompt.codex.verify"),
  ].join("\n")
}

function cursorPrompt(url: string, t: Translate): string {
  return [
    t("mcp.prompt.cursor.intro"),
    "",
    "{",
    '  "mcpServers": {',
    '    "kronika": {',
    `      "url": "${url}",`,
    '      "headers": {',
    '        "Authorization": "Basic <BASE64>"',
    "      }",
    "    }",
    "  }",
    "}",
    "",
    t("mcp.prompt.base64"),
    "printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\\n'",
    "",
    t("mcp.prompt.cursor.verify"),
  ].join("\n")
}

export function McpPanel({ onClose, t }: { readonly onClose: () => void; readonly t: Translate }) {
  const url = `${window.location.origin}/mcp`
  const prompts = [
    { label: "Claude Code", sectionKey: "mcp.section.claude", text: claudePrompt(url, t) },
    { label: "Codex CLI", sectionKey: "mcp.section.codex", text: codexPrompt(url, t) },
    { label: "Cursor", sectionKey: "mcp.section.cursor", text: cursorPrompt(url, t) },
  ]
  return (
    <aside aria-label={t("mcp.title")} className="fixed bottom-0 right-0 top-0 z-[100] w-[min(92vw,430px)] max-w-[430px] overflow-auto border-l border-line4 bg-s1 p-[18px] shadow-[-20px_0_50px_var(--color-shadow-a)]" data-testid="mcp-panel">
      <div className="flex items-center justify-between">
        <div>
          <p className="m-0 text-xs text-fg4">MCP</p>
          <h2>{t("mcp.title")}</h2>
        </div>
        <button aria-label={t("mcp.close")} className="icon-button" onClick={onClose} type="button">×</button>
      </div>
      <p className="border border-line3 bg-s2 p-2 text-sm text-accent2">{t("mcp.endpoint")}: <code>{url}</code></p>
      <p className="text-sm leading-[1.6] text-fg3">{t("mcp.credentials")}</p>
      {prompts.map((prompt) => (
        <section className="border-t border-line2 py-3" key={prompt.label}>
          <div className="flex items-center justify-between">
            <h3 className="m-0 text-sm text-fg">{prompt.label}</h3>
            <button aria-label={`${t("mcp.copy")} — ${prompt.label}`} className="inline-flex cursor-pointer items-center gap-1 rounded-[var(--radius-sm)] border border-line3 bg-s2 px-1.5 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s3" onClick={() => void navigator.clipboard?.writeText(prompt.text)} type="button"><Copy aria-hidden="true" size={12} />{t("mcp.copy")}</button>
          </div>
          <p className="mb-1 mt-[5px] text-xs text-fg4">{t(prompt.sectionKey)}</p>
          <pre className="m-0 select-all whitespace-pre-wrap break-words border border-line3 bg-s2 p-2 text-xs leading-[1.5] text-fg2">{prompt.text}</pre>
        </section>
      ))}
      <p className="mt-3 text-xs text-fg4">{t("mcp.docs")}</p>
    </aside>
  )
}
