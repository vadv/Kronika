import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("the MCP panel is reachable from the top bar and self-addresses the page origin", async () => {
  const [app, panel] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/mcp-connect.tsx", import.meta.url), "utf8"),
  ])
  assert.match(app, /data-testid="mcp-trigger"/)
  // Opening one side panel closes the other: two full-height asides overlap.
  assert.match(app, /setMcpOpen\(\(current\) => !current\);?\s*setHelpOpen\(false\)/)
  assert.match(app, /setHelpOpen\(\(current\) => !current\);?\s*setMcpOpen\(false\)/)
  // The keyboard path holds the same invariant: Esc closes both panels and
  // "?" never opens help underneath an open MCP panel.
  assert.match(app, /event\.key === "\?"(?:(?!event\.key)[\s\S])*?setMcpOpen\(false\)/)
  assert.match(app, /event\.key === "Escape"(?:(?!event\.key)[\s\S])*?setMcpOpen\(false\)/)
  assert.match(app, /\{mcpOpen && <McpPanel/)
  assert.match(panel, /window\.location\.origin\}\/mcp/)
  for (const label of ["Claude Code", "Codex CLI", "Cursor"]) assert.match(panel, new RegExp(label))
  // The Authorization value comes from the server, through the session
  // fetch wrapper that keeps UI 401s challenge-free.
  assert.match(panel, /apiFetch\("\/api\/mcp-access"/)
  assert.doesNotMatch(panel, /\bfetch\(/)
  assert.match(panel, /kind: "header", value: body\.authorization/)
  assert.match(panel, /catch\(\(\) => setAuth\(\{ kind: "placeholder" \}\)\)/)
})

test("every client prompt carries the endpoint, wrap-safe base64, and the entry point", async () => {
  const panel = await readFile(new URL("../src/mcp-prompts.ts", import.meta.url), "utf8")
  // tr -d keeps long credentials from folding a newline into the header:
  // once in the placeholder claude command, once in the shared recipe.
  assert.equal(panel.match(/base64 \| tr -d '\\\\n'/g)?.length, 2)
  assert.match(panel, /--transport http --scope user kronika \$\{url\}/)
  assert.match(panel, /\[mcp_servers\.kronika\]/)
  const english = await readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8")
  assert.match(english, /replace the whole \[mcp_servers\.kronika\] table/)
  assert.match(english, /kronika_get_context is the entry point/)
  const russian = await readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8")
  assert.match(russian, /замени целиком таблицу \[mcp_servers\.kronika\]/)
  assert.match(russian, /точка входа — kronika_get_context/)
})
