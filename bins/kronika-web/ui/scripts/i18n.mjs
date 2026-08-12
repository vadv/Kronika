import { readFile } from "node:fs/promises"

const KEY = /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$/
const PLACEHOLDER = /\{[a-z][a-z0-9_]*\}/gi

export function parseDictionary(source, name) {
  const dictionary = Object.create(null)
  for (const [offset, raw] of source.split(/\r?\n/).entries()) {
    const line = raw.trim()
    if (line === "" || line.startsWith("#")) continue
    const separator = line.indexOf(":")
    if (separator < 1) fail(name, offset, "expected a flat key and quoted value")
    const key = line.slice(0, separator).trim()
    const encoded = line.slice(separator + 1).trim()
    if (!KEY.test(key)) fail(name, offset, `invalid stable key ${JSON.stringify(key)}`)
    if (Object.hasOwn(dictionary, key)) fail(name, offset, `duplicate key ${JSON.stringify(key)}`)
    let value
    try {
      value = JSON.parse(encoded)
    } catch {
      fail(name, offset, `value for ${JSON.stringify(key)} must be a quoted JSON string`)
    }
    if (typeof value !== "string" || value.trim() === "") {
      fail(name, offset, `value for ${JSON.stringify(key)} must be nonempty`)
    }
    dictionary[key] = value
  }
  return dictionary
}

export function validateDictionaries(english, russian) {
  const enKeys = Object.keys(english).sort()
  const ruKeys = Object.keys(russian).sort()
  if (JSON.stringify(enKeys) !== JSON.stringify(ruKeys)) {
    const missingRu = enKeys.filter((key) => !Object.hasOwn(russian, key))
    const missingEn = ruKeys.filter((key) => !Object.hasOwn(english, key))
    throw new Error(`translation key mismatch; missing ru=[${missingRu}] missing en=[${missingEn}]`)
  }
  for (const key of enKeys) {
    const enSlots = placeholders(english[key])
    const ruSlots = placeholders(russian[key])
    if (JSON.stringify(enSlots) !== JSON.stringify(ruSlots)) {
      throw new Error(`placeholder mismatch for ${JSON.stringify(key)}; en=[${enSlots}] ru=[${ruSlots}]`)
    }
  }
  return enKeys
}

export async function dictionaryModule(directory) {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", directory), "utf8"),
    readFile(new URL("../i18n/ru.yaml", directory), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  const keys = validateDictionaries(english, russian)
  const messages = Object.fromEntries(keys.map((key) => [key, [english[key], russian[key]]]))
  return `const messages=${JSON.stringify(messages)} as const;export type TranslationKey=keyof typeof messages;export const translation=(locale:"en"|"ru",key:string)=>messages[key as TranslationKey]?.[locale==="ru"?1:0];`
}

function placeholders(value) {
  return [...value.matchAll(PLACEHOLDER)].map((match) => match[0].toLowerCase()).sort()
}

function fail(name, offset, message) {
  throw new Error(`${name}:${offset + 1}: ${message}`)
}
