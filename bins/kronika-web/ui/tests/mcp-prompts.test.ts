import assert from "node:assert/strict"
import test from "node:test"

import {
  type Auth,
  claudeCommand,
  claudePrompt,
  codexPrompt,
  codexTable,
  credentialsKey,
  cursorConfig,
  cursorPrompt,
} from "../src/mcp-prompts.ts"

const ENDPOINT = "http://192.168.0.22:8080/mcp"
const HEADER: Auth = { kind: "header", value: "Basic ZGVtbzpmb3JlbnNpY3M=" }
const OPEN: Auth = { kind: "open" }
const PLACEHOLDER: Auth = { kind: "placeholder" }
const t = (key: string) => key

test("the cursor config parses as JSON in every mode and carries the header only when one exists", () => {
  for (const auth of [HEADER, OPEN, PLACEHOLDER]) {
    const parsed = JSON.parse(cursorConfig(ENDPOINT, auth)) as {
      mcpServers: { kronika: { url: string; headers?: { Authorization: string } } }
    }
    assert.equal(parsed.mcpServers.kronika.url, ENDPOINT)
    if (auth.kind === "open") assert.equal(parsed.mcpServers.kronika.headers, undefined)
    else if (auth.kind === "header") {
      assert.equal(parsed.mcpServers.kronika.headers?.Authorization, HEADER.value)
    } else assert.equal(parsed.mcpServers.kronika.headers?.Authorization, "Basic <BASE64>")
  }
})

test("the codex table omits http_headers on an open server and inlines the served value", () => {
  assert.deepEqual(codexTable(ENDPOINT, OPEN), ["[mcp_servers.kronika]", `url = "${ENDPOINT}"`])
  assert.equal(
    codexTable(ENDPOINT, HEADER).at(-1),
    `http_headers = { "Authorization" = "${HEADER.value}" }`,
  )
  assert.equal(
    codexTable(ENDPOINT, PLACEHOLDER).at(-1),
    'http_headers = { "Authorization" = "Basic <BASE64>" }',
  )
})

test("the claude command drops the continuation backslash together with the header line", () => {
  assert.deepEqual(claudeCommand(ENDPOINT, OPEN), [
    `claude mcp add --transport http --scope user kronika ${ENDPOINT}`,
  ])
  const [first, second] = claudeCommand(ENDPOINT, HEADER)
  assert.ok(first?.endsWith("\\"))
  assert.equal(second, `  --header "Authorization: ${HEADER.value}"`)
  // The placeholder form computes base64 in place, wrap-safe for long
  // credentials.
  assert.match(claudeCommand(ENDPOINT, PLACEHOLDER)[1] ?? "", /base64 \| tr -d '\\n'/)
})

test("the base64 recipe appears only where a <BASE64> placeholder is left to fill", () => {
  for (const [prompt, auth, expected] of [
    [codexPrompt, PLACEHOLDER, true],
    [codexPrompt, HEADER, false],
    [codexPrompt, OPEN, false],
    [cursorPrompt, PLACEHOLDER, true],
    [cursorPrompt, HEADER, false],
    [claudePrompt, HEADER, false],
    [claudePrompt, PLACEHOLDER, false],
  ] as const) {
    assert.equal(prompt(ENDPOINT, auth, t).includes("mcp.prompt.base64"), expected)
  }
})

test("the credentials note names what the prompts carry", () => {
  assert.equal(credentialsKey(HEADER), "mcp.credentials.embedded")
  assert.equal(credentialsKey(OPEN), "mcp.credentials.open")
  assert.equal(credentialsKey(PLACEHOLDER), "mcp.credentials.manual")
})
