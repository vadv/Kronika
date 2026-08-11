import { execFileSync } from "node:child_process"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { build } from "esbuild"
import { gzipSync } from "fflate"
import { gunzipSync } from "node:zlib"

import { dictionaryModule } from "./i18n.mjs"

const scriptsDirectory = dirname(fileURLToPath(import.meta.url))
const uiDirectory = resolve(scriptsDirectory, "..")
const repository = resolve(uiDirectory, "../../..")
const fixtureOutputAt = process.argv.indexOf("--fixture-output")
const fixtureOutput = fixtureOutputAt < 0 ? null : process.argv[fixtureOutputAt + 1]
if (fixtureOutputAt >= 0 && (fixtureOutput === undefined || process.argv.length !== fixtureOutputAt + 2)) {
  throw new Error("usage: node scripts/build.mjs [--check | --fixture-output OUTPUT_GZIP]")
}
const artifact = fixtureOutput === null ? join(uiDirectory, "kronika-ui.html.gz") : resolve(fixtureOutput)
const checkOnly = process.argv.includes("--check")
if (checkOnly && fixtureOutput !== null) throw new Error("--check and --fixture-output cannot be combined")
// Raw grew by the lanes of the timeline and the help text each of them
// carries in two languages; gzip, which is what a browser downloads, moved
// by about a kilobyte.
const maximumRawBytes = fixtureOutput === null ? 840_000 : 40_000_000
const maximumGzipBytes = fixtureOutput === null ? 216_000 : 8_000_000
const rustToolchain = process.env.RUST_TOOLCHAIN ?? "1.96.0"
const rustHost = execFileSync("rustc", [`+${rustToolchain}`, "-vV"], { encoding: "utf8" })
  .match(/^host: (.+)$/m)?.[1]
if (rustHost === undefined) {
  throw new Error("rustc did not report its host target")
}

const temporary = await mkdtemp(join(tmpdir(), "kronika-ui-"))
try {
  const registry = execFileSync(
    "cargo",
    [`+${rustToolchain}`, "run", "--locked", "--quiet", "--target", rustHost, "-p", "kronika-registry", "--example", "ui_metadata"],
    { cwd: repository, encoding: "utf8", env: { ...process.env, CARGO_TERM_COLOR: "never" } },
  )
  const translations = await dictionaryModule(new URL("./", import.meta.url))
  const javascript = await bundleJavascript(registry, translations)
  const stylesheet = await compileStylesheet(temporary)
  const latinFont = await readFile(join(uiDirectory, "assets/JetBrainsMono-Latin.woff2"))
  const cyrillicFont = await readFile(join(uiDirectory, "assets/JetBrainsMono-Cyrillic.woff2"))
  const template = await readFile(join(uiDirectory, "src/index.html"), "utf8")
  const fixture = fixtureOutput === null ? "" : await fixtureScript()
  const html = template
    .replaceAll("{{KRONIKA_STYLE}}", () => `${fontFaces(latinFont, cyrillicFont)}\n${stylesheet}`)
    .replaceAll("{{KRONIKA_DATA}}", () => fixture)
    .replaceAll("{{KRONIKA_SCRIPT}}", () => javascript)

  validateHtml(html)
  const compressed = Buffer.from(gzipSync(Buffer.from(html), { level: 9, mtime: 0 }))
  validateGzipHeader(compressed)
  if (Buffer.byteLength(html) > maximumRawBytes || compressed.length > maximumGzipBytes) {
    throw new Error(
      `the UI exceeds its measured size bounds: raw ${Buffer.byteLength(html)}/${maximumRawBytes}, gzip ${compressed.length}/${maximumGzipBytes}`,
    )
  }

  if (checkOnly) {
    const committed = await readFile(artifact)
    if (!committed.equals(compressed)) {
      throw new Error("kronika-ui.html.gz differs from the reproducible build")
    }
  } else {
    await writeFile(artifact, compressed)
  }
  process.stdout.write(`kronika-ui raw=${Buffer.byteLength(html)} gzip=${compressed.length}\n`)
} finally {
  await rm(temporary, { recursive: true, force: true })
}

async function bundleJavascript(registry, translations) {
  const result = await build({
    absWorkingDir: uiDirectory,
    entryPoints: ["src/app.tsx"],
    bundle: true,
    define: { "process.env.NODE_ENV": '"production"' },
    format: "iife",
    legalComments: "none",
    mangleProps: /^g[a]p$/,
    minify: true,
    platform: "browser",
    sourcemap: false,
    target: ["es2022"],
    write: false,
    plugins: [{
      name: "kronika-registry",
      setup(context) {
        context.onResolve({ filter: /^kronika:registry$/ }, () => ({
          path: "registry.ts",
          namespace: "kronika",
        }))
        context.onLoad({ filter: /.*/, namespace: "kronika" }, () => ({
          contents: registry,
          loader: "ts",
        }))
        context.onResolve({ filter: /^kronika:i18n$/ }, () => ({
          path: "i18n.ts",
          namespace: "kronika-i18n",
        }))
        context.onLoad({ filter: /.*/, namespace: "kronika-i18n" }, () => ({
          contents: translations,
          loader: "ts",
        }))
      },
    }],
  })
  const output = result.outputFiles[0]
  if (output === undefined) {
    throw new Error("esbuild produced no JavaScript")
  }
  return output.text
}

async function fixtureScript() {
  const compressed = await readFile(join(uiDirectory, "fixtures/real-hour.json.gz"))
  const raw = gunzipSync(compressed).toString("utf8")
  JSON.parse(raw)
  const safe = raw.replaceAll("<", "\\u003c").replaceAll("\u2028", "\\u2028").replaceAll("\u2029", "\\u2029")
  return `globalThis.__KRONIKA_REAL_HOUR__=${safe};`
}

async function compileStylesheet(temporary) {
  const output = join(temporary, "kronika.css")
  execFileSync(
    join(uiDirectory, "node_modules/.bin/tailwindcss"),
    ["-i", join(uiDirectory, "src/styles.css"), "-o", output, "--minify"],
    { cwd: uiDirectory, stdio: "pipe" },
  )
  return readFile(output, "utf8")
}

function fontFaces(latin, cyrillic) {
  return [
    fontFace(latin, "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD"),
    fontFace(cyrillic, "U+0301,U+0400-045F,U+0490-0491,U+04B0-04B1,U+2116"),
  ].join("\n")
}

function fontFace(font, range) {
  return `@font-face{font-family:"JetBrains Mono";font-style:normal;font-weight:100 800;font-display:swap;src:url(data:font/woff2;base64,${font.toString("base64")}) format("woff2-variations");unicode-range:${range}}`
}

function validateHtml(html) {
  if (!html.startsWith("<!doctype html>")) {
    throw new Error("the built UI is not an HTML document")
  }
  const markers = ["{{KRONIKA_STYLE}}", "{{KRONIKA_DATA}}", "{{KRONIKA_SCRIPT}}"].filter((marker) => html.includes(marker))
  if (markers.length !== 0) {
    throw new Error(`the built UI contains unreplaced markers: ${markers.join(", ")}`)
  }
  if (/sourceMappingURL/i.test(html)) {
    throw new Error("source maps are forbidden in the production UI")
  }
  if (/\b(?:src|href)\s*=\s*["']\s*(?:https?:)?\/\//i.test(html)
      || /url\(\s*["']?\s*https?:\/\//i.test(html)) {
    throw new Error("the production UI contains an external asset URL")
  }
  if (html.toLowerCase().includes("</script")
      && html.toLowerCase().split("</script").length !== 2) {
    throw new Error("the JavaScript bundle contains an HTML script terminator")
  }
}

function validateGzipHeader(bytes) {
  if (bytes[0] !== 0x1f || bytes[1] !== 0x8b || bytes[2] !== 8 || bytes[3] !== 0) {
    throw new Error("gzip must use deflate without a stored name")
  }
  if (!bytes.subarray(4, 8).equals(Buffer.alloc(4))) {
    throw new Error("gzip modification time must be zero")
  }
}
