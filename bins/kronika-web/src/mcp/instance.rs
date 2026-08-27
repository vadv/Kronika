//! `kronika_get_instance`: the newest recorded host facts and `PostgreSQL`
//! server settings.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::snapshot::PlainRowOut;
use crate::config::Config;
use crate::route::MAX_SNAPSHOT_PAGE_SIZE;

use super::postgresql::{plain_row_to_json, plain_rows};
use super::semantics::{DecimalI64, mcp_structured};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    if let Err(error) = super::semantics::parameterless::<super::catalog::GetInstanceInput>(
        "kronika_get_instance",
        arguments,
    ) {
        return error;
    }
    let host = match newest_rows(config, "instance_metadata", cancelled) {
        Ok(part) => part,
        Err(error) => return error,
    };
    let settings = match newest_rows(config, "pg_settings", cancelled) {
        Ok(part) => part,
        Err(error) => return error,
    };

    // The newest recorded host row: layouts can each contribute their latest
    // observation, so pick the most recent by the row's own `at`.
    let host_row = host
        .rows
        .into_iter()
        .max_by_key(|row| row.at)
        .map(plain_row_to_json);
    let row_count = settings.rows.len();
    let summary = format!(
        "Returned {} and {row_count} recorded pg_settings row{}{}.",
        if host_row.is_some() {
            "recorded host facts"
        } else {
            "no recorded host facts"
        },
        if row_count == 1 { "" } else { "s" },
        if settings.has_more {
            "; settings truncated to the row cap"
        } else {
            ""
        },
    );
    mcp_structured(
        json!({
            "host": host_row,
            "host_as_of": host.as_of.map(DecimalI64),
            "postgresql_settings": settings
                .rows
                .into_iter()
                .map(plain_row_to_json)
                .collect::<Vec<Value>>(),
            "settings_as_of": settings.as_of.map(DecimalI64),
            "settings_has_more": settings.has_more,
        }),
        summary,
    )
}

struct Part {
    rows: Vec<PlainRowOut>,
    has_more: bool,
    as_of: Option<i64>,
}

/// One section's newest recorded rows through the shared plain pipeline; a
/// section recorded by no segment yields an empty part, not an error.
fn newest_rows(
    config: &Config,
    logical_name: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Part, CallToolResult> {
    let limit = u32::try_from(MAX_SNAPSHOT_PAGE_SIZE).unwrap_or(u32::MAX);
    let Some((rows, has_more, at)) = plain_rows(logical_name, config, &[], None, limit, cancelled)?
    else {
        return Ok(Part {
            rows: Vec::new(),
            has_more: false,
            as_of: None,
        });
    };
    Ok(Part {
        rows,
        has_more,
        as_of: Some(at),
    })
}
