// Copies through the Clipboard API where it works, falling back to the
// deprecated execCommand path — the only route on plain-http origins,
// Kronika's normal LAN deployment, and the second chance when the API
// rejects (unfocused document, denied permission).
export async function copyText(text: string): Promise<boolean> {
  if (navigator.clipboard !== undefined) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // fall through to the selection path
    }
  }
  const previous = document.activeElement
  const area = document.createElement("textarea")
  area.value = text
  area.setAttribute("readonly", "")
  area.style.position = "fixed"
  area.style.opacity = "0"
  document.body.append(area)
  area.select()
  let copied = false
  try {
    copied = document.execCommand("copy")
  } catch {
    copied = false
  }
  area.remove()
  if (previous instanceof HTMLElement) previous.focus()
  return copied
}
