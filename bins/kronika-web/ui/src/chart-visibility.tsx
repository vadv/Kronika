import { createContext, useContext, type ReactNode } from "react"

const ChartVisibility = createContext(true)

export const ChartVisibilityProvider = ChartVisibility.Provider

export function useChartsVisible(): boolean {
  return useContext(ChartVisibility)
}

export function ChartOnly({ children }: { readonly children: ReactNode }) {
  return useChartsVisible() ? children : null
}

export function loadChartVisibility(storage: Pick<Storage, "getItem">): boolean {
  try { return storage.getItem("kronika.charts") !== "0" } catch { return true }
}
