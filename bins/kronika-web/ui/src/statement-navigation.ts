import type { DataRow } from "./api"
import { rawText, value } from "./model"

export interface StatementTarget {
  readonly dbId: string
  readonly match: "exact" | "last"
  readonly planId: string
  readonly queryId: string
  readonly sourceTypeId: string
  readonly topLevel: boolean | null
  readonly userId: string
}

const VADV_PLAN_TYPE_ID = "1004001"

export function statementTargetForPlan(row: DataRow): StatementTarget | null {
  const vadv = row.typeId === VADV_PLAN_TYPE_ID
  const queryId = rawText(value(row, vadv ? "queryid_stat_statements" : "queryid"))
  const dbId = rawText(value(row, "dbid"))
  const userId = rawText(value(row, "userid"))
  const planId = rawText(value(row, "planid"))
  if (queryId === null || queryId === "0" || dbId === null || userId === null || planId === null) return null
  const storedTopLevel = value(row, "toplevel")
  return {
    dbId,
    match: vadv ? "last" : "exact",
    planId,
    queryId,
    sourceTypeId: row.typeId,
    topLevel: typeof storedTopLevel === "boolean" ? storedTopLevel : null,
    userId,
  }
}

export function statementTargetFilters(target: StatementTarget): Readonly<Record<string, string>> {
  return {
    dbid: target.dbId,
    queryid: target.queryId,
    userid: target.userId,
    ...(target.topLevel === null ? {} : { toplevel: String(target.topLevel) }),
  }
}
