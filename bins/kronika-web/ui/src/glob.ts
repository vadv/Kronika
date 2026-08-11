/** A pattern typed into a table filter. `*` stands for any run of characters
 *  and `?` for exactly one, the way a shell glob reads.
 *
 *  A pattern without either is taken as a substring: someone typing a command
 *  name means "rows mentioning this", not "rows equal to this". */
export function globMatcher(pattern: string): ((text: string) => boolean) | null {
  const wanted = pattern.trim()
  if (wanted === "") return null
  const lowered = wanted.toLowerCase()
  if (!wanted.includes("*") && !wanted.includes("?")) {
    return (text) => text.toLowerCase().includes(lowered)
  }
  const expression = new RegExp(`^${[...lowered].map(escape).join("")}$`)
  return (text) => expression.test(text.toLowerCase())
}

function escape(character: string): string {
  if (character === "*") return ".*"
  if (character === "?") return "."
  return /[a-z0-9Ѐ-ӿ_\s]/.test(character) ? character : `\\${character}`
}
