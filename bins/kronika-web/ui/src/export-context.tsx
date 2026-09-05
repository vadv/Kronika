import { createContext, useContext, useMemo, type ReactNode } from "react"

// The Export strip describes a selection on the hour timeline, so the timeline
// draws that selection and hosts the strip beneath its plot. The app owns the
// range; views need no new props.
export interface ExportSelection {
  readonly from: number
  readonly to: number
}

interface ExportSlot {
  readonly selection: ExportSelection | null
  readonly strip: ReactNode
}

const ExportContext = createContext<ExportSlot>({ selection: null, strip: null })

export function ExportProvider({ children, selection, strip }: {
  readonly children: ReactNode
  readonly selection: ExportSelection | null
  readonly strip: ReactNode
}) {
  const value = useMemo<ExportSlot>(() => ({ selection, strip }), [selection, strip])
  return <ExportContext.Provider value={value}>{children}</ExportContext.Provider>
}

export function useExportSlot(): ExportSlot {
  return useContext(ExportContext)
}
