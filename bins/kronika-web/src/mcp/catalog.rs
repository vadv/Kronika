//! The fixed MCP tool catalog: names, input schemas, descriptions.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::route::{MAX_HEATMAP_TOP, MAX_SNAPSHOT_PAGE_SIZE, Order, RelationGroup};

use super::filter::FilterInput;

pub(crate) const OVERVIEW_TOOL: &str = "kronika_overview";
pub(crate) const GET_CONTEXT_TOOL: &str = "kronika_get_context";
pub(crate) const FIND_POSTGRESQL_TABLES_TOOL: &str = "kronika_find_postgresql_tables";
pub(crate) const FIND_POSTGRESQL_INDEXES_TOOL: &str = "kronika_find_postgresql_indexes";
pub(crate) const FIND_POSTGRESQL_ACTIVITY_TOOL: &str = "kronika_find_postgresql_activity";
pub(crate) const FIND_POSTGRESQL_LOCKS_TOOL: &str = "kronika_find_postgresql_locks";
pub(crate) const FIND_POSTGRESQL_VACUUM_TOOL: &str = "kronika_find_postgresql_vacuum";
pub(crate) const FIND_POSTGRESQL_DATABASES_TOOL: &str = "kronika_find_postgresql_databases";
pub(crate) const FIND_POSTGRESQL_STATEMENTS_TOOL: &str = "kronika_find_postgresql_statements";
pub(crate) const FIND_POSTGRESQL_PLANS_TOOL: &str = "kronika_find_postgresql_plans";
pub(crate) const FIND_PROCESSES_TOOL: &str = "kronika_find_processes";
pub(crate) const GET_ROW_DETAIL_TOOL: &str = "kronika_get_row_detail";
pub(crate) const FIND_EVENTS_TOOL: &str = "kronika_find_events";

/// Input for `kronika_overview`: rank one recorded section's identities by
/// a chosen numeric field, over an explicit window.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct OverviewInput {
    /// Recorded logical section to rank within, e.g. "`os_cpu`",
    /// "`os_diskstats`", "`pg_stat_user_tables`", "`pg_stat_statements`",
    /// "`pg_store_plans`". Call `kronika_get_context` first to see which
    /// sections this host actually recorded — not every section is
    /// present on every host.
    pub(crate) section: String,
    /// One to four numeric field names on that section to rank by. More
    /// than one field is summed per identity before ranking.
    pub(crate) fields: Vec<String>,
    /// Inclusive start of the ranking window, Unix seconds.
    pub(crate) from: i64,
    /// Exclusive end of the ranking window, Unix seconds.
    pub(crate) to: i64,
    /// How many top-ranked identities to return.
    #[schemars(range(min = 1, max = MAX_HEATMAP_TOP))]
    pub(crate) top: u32,
}

/// `kronika_get_context` takes no arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GetContextInput;

/// Aggregation granularity for a relation `find_*` tool: one row per
/// object, or rolled up to a coarser identity that drops the columns
/// beneath it.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GroupInput {
    /// One row per table/index.
    Object,
    /// One row per database, summed across its tables/indexes.
    Database,
    /// One row per schema, summed across its tables/indexes.
    Schema,
    /// One row per tablespace, summed across its tables/indexes.
    Tablespace,
}

impl From<GroupInput> for RelationGroup {
    fn from(value: GroupInput) -> Self {
        match value {
            GroupInput::Object => Self::Object,
            GroupInput::Database => Self::Database,
            GroupInput::Schema => Self::Schema,
            GroupInput::Tablespace => Self::Tablespace,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DirectionInput {
    Asc,
    Desc,
}

impl From<DirectionInput> for Order {
    fn from(value: DirectionInput) -> Self {
        match value {
            DirectionInput::Asc => Self::Asc,
            DirectionInput::Desc => Self::Desc,
        }
    }
}

/// Field and direction to rank rows by before truncating to `limit`.
/// Omitting `sort` entirely leaves rows in their identity order.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SortInput {
    pub(crate) field: String,
    pub(crate) direction: DirectionInput,
}

/// Input for `kronika_find_postgresql_tables`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TablesInput {
    pub(crate) group: GroupInput,
    /// Flat AND-only list of typed predicates over `pg_stat_user_tables`
    /// fields, e.g. `table_name` contains "orders", or `size` greater than
    /// a byte count. Empty or omitted matches every table.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`seq_scan`", "`n_live_tup`", "`dead_pct`",
    /// "`buffer_hit_pct`". Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_indexes`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct IndexesInput {
    pub(crate) group: GroupInput,
    /// Flat AND-only list of typed predicates over `pg_stat_user_indexes`
    /// fields, e.g. `index_name` contains "pkey", or `size` greater than a
    /// byte count. Empty or omitted matches every index.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`idx_scan`", "`idx_tup_read`",
    /// "`buffer_hit_pct`". Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_activity`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ActivityInput {
    /// Flat AND-only list of typed predicates over `pg_stat_activity`
    /// fields: `pid`, `database`, `role`, `application`, `client_addr`,
    /// `backend_type`, `state`, `wait_event_type`, `wait_event`,
    /// `query_id`, `backend_xid_age`, `backend_xmin_age`, or `text` (a
    /// combined search over the query, application name, client address,
    /// database and role). Empty or omitted matches every backend.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`backend_xid_age`", "`backend_xmin_age`".
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_locks`. Every returned row also
/// carries `blocked_by`, the list of pids blocking it — not filterable as a
/// predicate here (it is a list, not a scalar), but always present in the
/// row.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LocksInput {
    /// Flat AND-only list of typed predicates over `pg_locks` fields:
    /// `pid`, `database`, `role`, `state`, `lock_type`, `lock_mode`,
    /// `table_name`, or `text` (a combined search over the query, database,
    /// role and locked table name). Empty or omitted matches every backend
    /// in the wait graph.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by. Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_vacuum`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct VacuumInput {
    /// Flat AND-only list of typed predicates over `pg_stat_progress_vacuum`
    /// fields: `pid`, `database`, `schema`, `table_name`, `phase`,
    /// `is_autovacuum`, `heap_blks_total`, `heap_blks_scanned`,
    /// `heap_blks_vacuumed`, or `text` (a combined search over database,
    /// table, schema and phase). Empty or omitted matches every backend
    /// currently running `VACUUM`.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`heap_blks_scanned`", "`heap_blks_vacuumed`".
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_databases`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DatabasesInput {
    /// Flat AND-only list of typed predicates over `pg_stat_database`
    /// fields: `datid`, `database`, `numbackends`, `xact_commit`,
    /// `xact_rollback`, `deadlocks`, `temp_bytes`, or `text` (a database
    /// name search). Empty or omitted matches every database.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`numbackends`", "`deadlocks`", "`xact_commit`".
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_statements`. Rows carry seven
/// `derived_*` fields alongside the raw `pg_stat_statements` columns —
/// `derived_mean_exec_ms_per_call`, `derived_rows_per_call`,
/// `derived_blocks_per_call`, `derived_hit_pct`, `derived_wal_per_call`,
/// `derived_plan_time_pct`, `derived_cv` — computed from the same row's
/// already-rate-converted fields, never from a filter or a second lookup.
/// `derived_hit_pct` and `derived_plan_time_pct` are a `0.0`-`1.0`
/// fraction, not a percentage. Any `derived_*` field is `null` when its
/// reading has no predecessor snapshot yet (a rate needs two samples) or
/// when the underlying column does not exist on this extension version
/// (e.g. `derived_wal_per_call` before extension 1.8, which predates WAL
/// tracking).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct StatementsInput {
    /// Flat AND-only list of typed predicates over `pg_stat_statements`
    /// fields: `query_id`, `database`, `role`, `call_rate`,
    /// `exec_time_rate`, `mean_exec`, `row_rate`, `rows_per_call`, or
    /// `text` (a combined search over the query text, database and role).
    /// Empty or omitted matches every statement.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`calls`", "`total_exec_time`", or a
    /// `derived_*` field name. Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_plans`. Rows carry the same seven
/// `derived_*` fields `kronika_find_postgresql_statements` does, computed
/// the same way from `pg_store_plans`'s own already-rate-converted
/// fields — `derived_wal_per_call` is always `null` here, since no
/// `pg_store_plans` physical layout carries a WAL byte count;
/// `derived_plan_time_pct` is `null` except on the one layout that tracks
/// planning time separately from execution time.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PlansInput {
    /// Flat AND-only list of typed predicates over `pg_store_plans`
    /// fields: `query_id`, `plan_id`, `database`, `role`, `calls`,
    /// `call_rate`, `exec_time_rate`, `mean_exec`, `row_rate`,
    /// `rows_per_call`, or `text` (a combined search over the plan text,
    /// database and role). Empty or omitted matches every plan.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`calls`", "`total_time`", or a `derived_*`
    /// field name. Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_processes`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ProcessesInput {
    /// Flat AND-only list of typed predicates over `os_process` fields,
    /// e.g. `pid` equals a number, or `command` contains "postgres". Empty
    /// or omitted matches every process.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Field to rank by, e.g. "`rmem_kb`", "`vmem_kb`", "`num_threads`".
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_get_row_detail`. Every locator field accepts a JSON
/// number or a decimal string: `segment_id`, `at` and `row_ordinal` are
/// `i64`/`u64` values that can exceed JSON's safe-integer range (2^53), and
/// `type_id` is accepted the same way for a different reason — a
/// `kronika_find_*` row renders all four locator fields as decimal strings,
/// so any of them can be copied straight from there into this tool's
/// arguments without reformatting.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RowDetailInput {
    /// Recorded logical section the row belongs to, e.g. "`os_process`",
    /// "`pg_stat_statements`", "`pg_store_plans`". Must be a plain,
    /// single-section, timestamped section — not "`pg_stat_user_tables`" or
    /// "`pg_stat_user_indexes`", which this tool does not address (use
    /// `kronika_find_postgresql_tables`/`_indexes` for those).
    pub(crate) section: String,
    /// The segment this row was recorded in, as a JSON number or a decimal
    /// string.
    pub(crate) segment_id: serde_json::Value,
    /// The row's own timestamp, Unix microseconds, exactly as recorded, as
    /// a JSON number or a decimal string.
    pub(crate) at: serde_json::Value,
    /// The physical layout id for `section` in this segment, as a JSON
    /// number or a decimal string: pins the locator to one exact schema
    /// version, since a logical section's physical layout can change over
    /// time.
    pub(crate) type_id: serde_json::Value,
    /// The row's physical position within `section`'s data in this
    /// segment, as a JSON number or a decimal string.
    pub(crate) row_ordinal: serde_json::Value,
}

/// Input for `kronika_find_events`. Narrows by which recorded event
/// sections to read and by an explicit time window only — no field-level
/// predicates in v1; a caller that needs more filters the returned rows
/// itself.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EventsInput {
    /// Which recorded event sections to read: any of "`pg_log_errors`",
    /// "`pg_log_checkpoints`", "`pg_log_autovacuum`",
    /// "`pg_log_slow_queries`", "`pg_log_lock_waits`",
    /// "`pg_log_lifecycle`", "`pgbouncer_events`". Omit to read all seven.
    #[serde(default)]
    pub(crate) sources: Option<Vec<String>>,
    /// Inclusive start of the window, Unix microseconds.
    pub(crate) from: i64,
    /// Inclusive end of the window, Unix microseconds. `to` minus `from`
    /// cannot exceed one hour.
    pub(crate) to: i64,
    /// Maximum rows to return in total, across every requested source.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

pub(crate) fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            OVERVIEW_TOOL,
            "Ranks activity over a time window by a chosen field, across \
             whatever identities were active in a recorded section \
             (processes, PostgreSQL tables, statements, ...). Call this \
             first: it shows where activity concentrated in the window \
             before a kronika_find_* tool narrows in on one identity. \
             Returns ranked identities plus the combined total for \
             everything else that did not make the top list.",
            schema_object::<OverviewInput>(),
        ),
        Tool::new(
            GET_CONTEXT_TOOL,
            "Reports what this host actually recorded: which logical \
             sections exist, so kronika_overview's `section` argument is \
             never a guess.",
            schema_object::<GetContextInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_TABLES_TOOL,
            "Reads the current pg_stat_user_tables snapshot: one row per \
             table (or rolled up to database/schema/tablespace), with \
             optional typed filters and a sort field. Returns up to \
             `limit` rows plus `has_more` when more rows matched than were \
             returned.",
            schema_object::<TablesInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_INDEXES_TOOL,
            "Reads the current pg_stat_user_indexes snapshot: one row per \
             index (or rolled up to database/schema/tablespace), with \
             optional typed filters and a sort field. Returns up to \
             `limit` rows plus `has_more` when more rows matched than were \
             returned.",
            schema_object::<IndexesInput>(),
        ),
    ]
    .into_iter()
    .chain(postgresql_plain_tools())
    .chain([
        Tool::new(
            FIND_PROCESSES_TOOL,
            "Reads the current os_process snapshot: one row per running \
             process, with optional typed filters and a sort field. \
             Returns up to `limit` rows plus `has_more` when more rows \
             matched than were returned. Each row carries its own \
             segment_id/type_id/row_ordinal/at as decimal strings — pass \
             them straight into kronika_get_row_detail to re-fetch that \
             exact row later.",
            schema_object::<ProcessesInput>(),
        ),
        Tool::new(
            GET_ROW_DETAIL_TOOL,
            "Fetches one exact row by its physical locator — `section`, \
             `segment_id`, `at`, `type_id`, `row_ordinal` — rather than a \
             ranked or filtered search. Unlike kronika_find_*, which only \
             reads the current live snapshot, this can read a row from \
             any recorded segment once its locator is already known. \
             Covers plain, single-section, timestamped sections only \
             (os_process, pg_stat_statements, pg_store_plans, and \
             similar); it does not address \
             kronika_find_postgresql_tables/_indexes rows, which are \
             grouped by (datid, relid) rather than a single physical row \
             ordinal.",
            schema_object::<RowDetailInput>(),
        ),
        Tool::new(
            FIND_EVENTS_TOOL,
            "Reads recorded event rows from one or more of the seven \
             event-shaped log sections (PostgreSQL log errors, \
             checkpoints, autovacuum, slow queries, lock waits, lifecycle \
             events, and PgBouncer log events), narrowed by source and by \
             an explicit time window of at most one hour. No field-level \
             filtering in v1: a caller that needs to narrow further does \
             so over the returned rows itself. Merges every requested \
             source into one list ordered by timestamp, truncated to \
             `limit`, with `has_more` set when more rows matched than \
             were returned. Each row carries its own source plus \
             segment_id/type_id/row_ordinal/at as decimal strings — pass \
             them straight into kronika_get_row_detail to re-fetch that \
             exact row later. The numeric codes these sections record \
             (pg_log_errors severity/category, pg_log_checkpoints phase, \
             pg_log_autovacuum/pg_log_lock_waits/pg_log_lifecycle kind, \
             pgbouncer_events level) are Kronika-recorded values, not a \
             severity ordering — each such field comes with a \
             `<field>_label` sibling carrying its human-readable string, \
             e.g. `severity: 0, severity_label: \"error\"`, alongside the \
             unchanged numeric field. kronika_get_row_detail carries the \
             same label siblings when reading one of these rows back.",
            schema_object::<EventsInput>(),
        ),
    ])
    .collect()
}

/// The six `find_*` tools over plain (non-relation-grouped) `PostgreSQL`
/// sections: split out of `tools()` to keep that function under Clippy's
/// line-count lint, not because these six are architecturally distinct
/// from the tools around them.
fn postgresql_plain_tools() -> [Tool; 6] {
    [
        Tool::new(
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            "Reads the current pg_stat_activity snapshot: one row per \
             backend connection, with optional typed filters and a sort \
             field. Returns up to `limit` rows plus `has_more` when more \
             rows matched than were returned. Each row carries its own \
             segment_id/type_id/row_ordinal/at as decimal strings — pass \
             them straight into kronika_get_row_detail to re-fetch that \
             exact row later.",
            schema_object::<ActivityInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_LOCKS_TOOL,
            "Reads the current pg_locks snapshot: one row per backend \
             involved in the lock wait graph (root or waiter), with \
             optional typed filters and a sort field. Returns up to \
             `limit` rows plus `has_more` when more rows matched than were \
             returned. Each row carries its own \
             segment_id/type_id/row_ordinal/at as decimal strings — pass \
             them straight into kronika_get_row_detail to re-fetch that \
             exact row later.",
            schema_object::<LocksInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_VACUUM_TOOL,
            "Reads the current pg_stat_progress_vacuum snapshot: one row \
             per backend actively running VACUUM, with optional typed \
             filters and a sort field. Returns up to `limit` rows plus \
             `has_more` when more rows matched than were returned. Each \
             row carries its own segment_id/type_id/row_ordinal/at as \
             decimal strings — pass them straight into \
             kronika_get_row_detail to re-fetch that exact row later.",
            schema_object::<VacuumInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_DATABASES_TOOL,
            "Reads the current pg_stat_database snapshot: one row per \
             database, with optional typed filters and a sort field. \
             Returns up to `limit` rows plus `has_more` when more rows \
             matched than were returned. Each row carries its own \
             segment_id/type_id/row_ordinal/at as decimal strings — pass \
             them straight into kronika_get_row_detail to re-fetch that \
             exact row later.",
            schema_object::<DatabasesInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_STATEMENTS_TOOL,
            "Reads the current pg_stat_statements snapshot: one row per \
             tracked statement, with optional typed filters and a sort \
             field. Each row carries derived_mean_exec_ms_per_call, \
             derived_rows_per_call, derived_blocks_per_call, \
             derived_hit_pct, derived_wal_per_call, \
             derived_plan_time_pct and derived_cv alongside the raw \
             columns; derived_hit_pct and derived_plan_time_pct are a \
             0.0-1.0 fraction, not a percentage. Any derived_* field is \
             null when its reading has no prior snapshot to compute a \
             rate from, or when the column is absent on this extension \
             version. Returns up to limit rows plus has_more when more \
             rows matched than were returned. Each row carries its own \
             segment_id/type_id/row_ordinal/at as decimal strings — pass \
             them straight into kronika_get_row_detail to re-fetch that \
             exact row later.",
            schema_object::<StatementsInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_PLANS_TOOL,
            "Reads the current pg_store_plans snapshot: one row per \
             tracked plan, with optional typed filters and a sort field. \
             Rows carry the same seven derived_* fields \
             kronika_find_postgresql_statements does, computed the same \
             way; derived_wal_per_call is always null here (no \
             pg_store_plans layout tracks WAL bytes), and \
             derived_plan_time_pct is null except where the installed \
             extension tracks planning time separately from execution \
             time. Returns up to limit rows plus has_more when more rows \
             matched than were returned. Each row carries its own \
             segment_id/type_id/row_ordinal/at as decimal strings — pass \
             them straight into kronika_get_row_detail to re-fetch that \
             exact row later.",
            schema_object::<PlansInput>(),
        ),
    ]
}

/// Bridge from a `schemars`-derived schema to the `Arc<JsonObject>` shape
/// `rmcp::model::Tool` wants. Goes through `serde_json::Value` rather than
/// `schemars`'s own schema types, so this does not depend on which
/// `schemars` major version Cargo resolved.
fn schema_object<T: JsonSchema>() -> Arc<JsonObject> {
    let schema = schemars::schema_for!(T);
    let value = serde_json::to_value(schema).expect("schema serializes to JSON");
    let object = value.as_object().expect("schema is a JSON object").clone();
    Arc::new(object)
}

#[cfg(test)]
mod tests;
