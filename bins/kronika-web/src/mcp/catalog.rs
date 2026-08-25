//! The fixed MCP tool catalog: names, input schemas, descriptions.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::route::{Order, RelationGroup};

use super::filter::FilterInput;

pub(crate) const OVERVIEW_TOOL: &str = "kronika_overview";
pub(crate) const GET_CONTEXT_TOOL: &str = "kronika_get_context";
pub(crate) const FIND_POSTGRESQL_TABLES_TOOL: &str = "kronika_find_postgresql_tables";
pub(crate) const FIND_POSTGRESQL_INDEXES_TOOL: &str = "kronika_find_postgresql_indexes";
pub(crate) const FIND_PROCESSES_TOOL: &str = "kronika_find_processes";
pub(crate) const GET_ROW_DETAIL_TOOL: &str = "kronika_get_row_detail";

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
