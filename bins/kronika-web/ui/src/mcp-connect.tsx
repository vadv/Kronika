import { Copy } from "lucide-react"
import { useEffect, useState } from "react"

import type { Translate } from "./help"
import { type Auth, claudePrompt, codexPrompt, credentialsKey, cursorPrompt } from "./mcp-prompts"
import { apiFetch } from "./session"

export function McpPanel({ onClose, t }: { readonly onClose: () => void; readonly t: Translate }) {
  const [auth, setAuth] = useState<Auth | null>(null)
  useEffect(() => {
    const controller = new AbortController()
    apiFetch("/api/mcp-access", { signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) throw new Error("mcp-access unavailable")
        const body: { authorization?: string | null } = await response.json()
        setAuth(
          typeof body.authorization === "string"
            ? { kind: "header", value: body.authorization }
            : { kind: "open" },
        )
      })
      .catch(() => setAuth({ kind: "placeholder" }))
    return () => controller.abort()
  }, [])

  const url = `${window.location.origin}/mcp`
  const prompts =
    auth === null
      ? []
      : [
          { label: "Claude Code", sectionKey: "mcp.section.claude", text: claudePrompt(url, auth, t) },
          { label: "Codex CLI", sectionKey: "mcp.section.codex", text: codexPrompt(url, auth, t) },
          { label: "Cursor", sectionKey: "mcp.section.cursor", text: cursorPrompt(url, auth, t) },
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
      {auth !== null && <p className="text-sm leading-[1.6] text-fg3">{t(credentialsKey(auth))}</p>}
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
