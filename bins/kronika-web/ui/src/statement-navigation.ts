import type { DataRow } from "./api"
import { rawText, value } from "./model"

export type StatementTarget = ActivityStatementTarget | PlanStatementTarget

export interface ActivityStatementTarget {
  readonly dbId: string
  readonly origin: "activity"
  readonly queryId: string
}

export interface PlanStatementTarget {
  readonly dbId: string
  readonly origin: "plan"
  readonly planId: string
  readonly queryId: string
  readonly relation: "shared" | "last"
  readonly sourceTypeId: string
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
  return {
    dbId,
    origin: "plan",
    planId,
    queryId,
    relation: vadv ? "last" : "shared",
    sourceTypeId: row.typeId,
    userId,
  }
}

export function statementTargetForActivity(row: DataRow): ActivityStatementTarget | null {
  const queryId = rawText(value(row, "query_id"))
  const dbId = rawText(value(row, "datid"))
  if (queryId === null || queryId === "0" || dbId === null || dbId === "0") return null
  return { dbId, origin: "activity", queryId }
}

export function statementTargetFilters(target: StatementTarget): Readonly<Record<string, string>> {
  return {
    dbid: target.dbId,
    queryid: target.queryId,
    ...(target.origin === "activity" ? {} : { userid: target.userId }),
  }
}
