import assert from "node:assert/strict"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { build } from "esbuild"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  external: ["react", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  stdin: { contents: 'export { Login } from "../src/login.tsx"', loader: "tsx", resolveDir: directory },
  write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const { Login } = loaded.exports
const copy = {
  "app.title": "KRONIKA",
  "auth.expired": "Your session ended. Sign in again.",
  "auth.password": "Password",
  "auth.submit": "Sign in",
  "auth.subtitle": "Credentials stay in memory.",
  "auth.title": "Sign in to Kronika",
  "auth.user": "Username",
  "locale.en": "EN",
  "locale.ru": "RU",
  "locale.switch": "Language",
}
const t = (key) => copy[key] ?? key

test("the login shell is an in-app bilingual form without embedded credentials", () => {
  const html = renderToStaticMarkup(createElement(Login, { expired: true, locale: "en", onLocale: () => {}, t }))
  assert.match(html, />Username</)
  assert.match(html, />Password</)
  assert.match(html, />RU</)
  assert.match(html, />EN</)
  assert.match(html, /Your session ended/)
  assert.doesNotMatch(html, /<form[^>]+action=/)
  assert.doesNotMatch(html, /name="password"[^>]+value="[^"].*"/)
})
