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
