import { createContext, useContext, useMemo, type ReactNode } from "react"

import { browserTimeZone, createDisplayTimeFormatter, type DisplayTimeFormatter, type DisplayTimeZone } from "./display-time"
import type { Locale } from "./model"

interface DisplayTimeValue extends DisplayTimeFormatter {
  readonly browserOffset: (timestamp: number) => string
  readonly setMode: (mode: DisplayTimeZone) => void
}

const localZone = browserTimeZone()
const fallback = createDisplayTimeFormatter("en", "browser", localZone)
const DisplayTimeContext = createContext<DisplayTimeValue>({
  ...fallback,
  browserOffset: fallback.zone,
  setMode: () => {},
})

export function DisplayTimeProvider({ children, locale, mode, setMode }: {
  readonly children: ReactNode
  readonly locale: Locale
  readonly mode: DisplayTimeZone
  readonly setMode: (mode: DisplayTimeZone) => void
}) {
  const value = useMemo<DisplayTimeValue>(() => {
    const formatter = createDisplayTimeFormatter(locale, mode, localZone)
    return { ...formatter, browserOffset: mode === "browser" ? formatter.zone : createDisplayTimeFormatter(locale, "browser", localZone).zone, setMode }
  }, [locale, mode, setMode])
  return <DisplayTimeContext.Provider value={value}>{children}</DisplayTimeContext.Provider>
}

export function useDisplayTime(): DisplayTimeValue {
  return useContext(DisplayTimeContext)
}
