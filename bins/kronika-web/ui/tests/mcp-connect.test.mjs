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
  // One client at a time behind a segmented tab row, in the app's own
  // lens-tabs shape.
  assert.match(panel, /className="lens-tabs[^"]*" role="group"/)
  assert.match(panel, /aria-pressed=\{client === candidate\.label\}/)
  assert.match(panel, /data-testid=\{`mcp-tab-\$\{candidate\.id\}`\}/)
  // The row must actually switch: a click updates the state and only the
  // selected client's builder runs.
  assert.match(panel, /onClick=\{\(\) => \{ setClient\(candidate\.label\); setCopied\(false\) \}\}/)
  // Copying must go through the shared helper: plain-http origins have no
  // navigator.clipboard, and the raw optional chain swallowed the click.
  assert.match(panel, /void copyText\(prompt\)\.then\(setCopied\)/)
  assert.match(panel, /selected\.builder\(url, auth, t\)/)
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

test("no component calls navigator.clipboard directly", async () => {
  const { readdir } = await import("node:fs/promises")
  const sources = (await readdir(new URL("../src", import.meta.url), { recursive: true })).filter(
    (name) => (name.endsWith(".ts") || name.endsWith(".tsx")) && name !== "clipboard.ts",
  )
  for (const name of sources) {
    const source = await readFile(new URL(`../src/${name}`, import.meta.url), "utf8")
    assert.doesNotMatch(source, /navigator\.clipboard/, name)
  }
})
