import type { Translate } from "./help"

// The placeholder branch mirrors docs/mcp-clients.md.
export type Auth =
  | { readonly kind: "header"; readonly value: string }
  | { readonly kind: "open" }
  | { readonly kind: "placeholder" }

const PLACEHOLDER_HEADER = "Basic <BASE64>"

// One registration per instance: endpoints on different hosts or ports
// register under different names.
export function serverName(url: string): string {
  let host = ""
  try {
    const parsed = new URL(url)
    host = parsed.port === "" ? parsed.hostname : `${parsed.hostname}-${parsed.port}`
  } catch {
    return "kronika"
  }
  const cleaned = host
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
  return cleaned === "" ? "kronika" : `kronika-${cleaned}`
}

function base64Recipe(t: Translate): readonly string[] {
  return ["", t("mcp.prompt.base64"), "printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\\n'"]
}

export function claudeCommand(url: string, auth: Auth): readonly string[] {
  const name = serverName(url)
  if (auth.kind === "open") return [`claude mcp add --transport http --scope user ${name} ${url}`]
  return [
    `claude mcp add --transport http --scope user ${name} ${url} \\`,
    auth.kind === "header"
      ? `  --header "Authorization: ${auth.value}"`
      : `  --header "Authorization: Basic $(printf '%s' '<USER>:<PASSWORD>' | base64 | tr -d '\\n')"`,
  ]
}

export function claudePrompt(url: string, auth: Auth, t: Translate): string {
  const name = serverName(url)
  return [
    t("mcp.prompt.claude.intro"),
    "",
    ...claudeCommand(url, auth),
    "",
    t("mcp.prompt.claude.verify", { name }),
  ].join("\n")
}

export function codexTable(url: string, auth: Auth): readonly string[] {
  const table = [`[mcp_servers.${serverName(url)}]`, `url = "${url}"`]
  if (auth.kind === "header") table.push(`http_headers = { "Authorization" = "${auth.value}" }`)
  if (auth.kind === "placeholder") {
    table.push(`http_headers = { "Authorization" = "${PLACEHOLDER_HEADER}" }`)
  }
  return table
}

export function codexPrompt(url: string, auth: Auth, t: Translate): string {
  const name = serverName(url)
  return [
    t("mcp.prompt.codex.intro", { name }),
    "",
    ...codexTable(url, auth),
    ...(auth.kind === "placeholder" ? base64Recipe(t) : []),
    "",
    t("mcp.prompt.codex.verify", { name }),
  ].join("\n")
}

export function cursorConfig(url: string, auth: Auth): string {
  const server =
    auth.kind === "open"
      ? [`      "url": "${url}"`]
      : [
          `      "url": "${url}",`,
          '      "headers": {',
          `        "Authorization": "${auth.kind === "header" ? auth.value : PLACEHOLDER_HEADER}"`,
          "      }",
        ]
  return ["{", '  "mcpServers": {', `    "${serverName(url)}": {`, ...server, "    }", "  }", "}"].join("\n")
}

export function cursorPrompt(url: string, auth: Auth, t: Translate): string {
  const name = serverName(url)
  return [
    t("mcp.prompt.cursor.intro"),
    "",
    cursorConfig(url, auth),
    ...(auth.kind === "placeholder" ? base64Recipe(t) : []),
    "",
    t("mcp.prompt.cursor.verify", { name }),
  ].join("\n")
}

export function credentialsKey(auth: Auth): string {
  if (auth.kind === "header") return "mcp.credentials.embedded"
  if (auth.kind === "open") return "mcp.credentials.open"
  return "mcp.credentials.manual"
}
