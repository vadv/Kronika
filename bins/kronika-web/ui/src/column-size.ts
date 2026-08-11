/** The width a column is given when its grip is double clicked: what the
 *  widest cell needs, kept inside bounds a table can live with. */
export function fittedWidth(widest: number): number {
  return Math.min(MAX_COLUMN, Math.max(MIN_COLUMN, Math.ceil(widest) + GRIP_ROOM))
}

const MIN_COLUMN = 64
const MAX_COLUMN = 720
const GRIP_ROOM = 10

/** Widest cell of one column, header included. Only the rows the virtualiser
 *  kept are measured; the rest are not in the document.
 *
 *  Cells are clipped to the width they already have, so the answer is taken
 *  from copies laid out with nothing holding them back. */
export function widestCell(table: HTMLElement, index: number): number {
  const document_ = table.ownerDocument
  const shadow = document_.createElement("div")
  shadow.style.cssText = "position:absolute;left:-9999px;top:0;visibility:hidden"
  const copies: HTMLElement[] = []
  for (const row of table.querySelectorAll<HTMLElement>("[role=\"row\"]")) {
    const cell = row.children[index]
    if (!(cell instanceof HTMLElement)) continue
    const copy = cell.cloneNode(true) as HTMLElement
    unbind(copy)
    for (const nested of copy.querySelectorAll<HTMLElement>("*")) unbind(nested)
    copies.push(copy)
    shadow.append(copy)
  }
  document_.body.append(shadow)
  const widest = copies.reduce((most, copy) => Math.max(most, copy.getBoundingClientRect().width), 0)
  shadow.remove()
  return widest
}

function unbind(element: HTMLElement): void {
  element.style.width = "max-content"
  element.style.maxWidth = "none"
  element.style.minWidth = "0"
  element.style.overflow = "visible"
  element.style.position = "static"
}
