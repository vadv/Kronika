/** A pattern typed into a table filter. It is looked for anywhere inside a
 *  value, with `*` standing for any run of characters and `?` for exactly one.
 *
 *  Anchoring a pattern to the whole value would be the shell rule, and it is
 *  the wrong one here: a command is a full path, so `kroni*` would have to
 *  match `/pgdata/.../bin/kronika-collector` from its first character and
 *  never would. */
export function globMatcher(pattern: string): ((text: string) => boolean) | null {
  const wanted = pattern.trim()
  if (wanted === "") return null
  const lowered = wanted.toLowerCase()
  if (!wanted.includes("*") && !wanted.includes("?")) {
    return (text) => text.toLowerCase().includes(lowered)
  }
  const expression = new RegExp([...lowered].map(escape).join(""))
  return (text) => expression.test(text.toLowerCase())
}

function escape(character: string): string {
  if (character === "*") return ".*"
  if (character === "?") return "."
  return /[a-z0-9\u0400-\u04ff_\s]/.test(character) ? character : `\\${character}`
}
