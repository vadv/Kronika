//! The fixed MCP tool catalog: names, input schemas, descriptions.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

pub(crate) const OVERVIEW_TOOL: &str = "kronika_overview";
pub(crate) const GET_CONTEXT_TOOL: &str = "kronika_get_context";

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
