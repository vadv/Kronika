//! `kronika_get_row_detail`: one exact row by its physical locator, through
//! `PreparedSnapshot::fetch_exact_row`.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::api::Prepared;
use crate::api::snapshot;
use crate::config::Config;
use crate::route::{Order, SnapshotRequest};

use super::catalog::RowDetailInput;
use super::semantics::{mcp_error, mcp_structured};

pub(crate) fn call(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: RowDetailInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    let segment_id = match decimal_i64("segment_id", &input.segment_id) {
        Ok(segment_id) => segment_id,
        Err(error) => return mcp_error(error),
    };
    let row_ordinal = match decimal_u64("row_ordinal", &input.row_ordinal) {
        Ok(row_ordinal) => row_ordinal,
        Err(error) => return mcp_error(error),
    };

    let request = SnapshotRequest {
        segment_id,
        at: input.at,
        sections: vec![input.section.clone()],
        fields: Vec::new(),
        by: Vec::new(),
        direction: Order::Asc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: Some(input.type_id),
        row_ordinal: Some(row_ordinal),
    };
    let prepared = match snapshot::prepare(&config.data_root, request, None) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let Prepared::Snapshot(prepared) = prepared else {
        return mcp_error("row locator did not prepare a snapshot");
    };
    let row = match prepared.fetch_exact_row(&|| false) {
        Ok(row) => row,
        Err(error) => return mcp_error(error.to_string()),
    };
    let Some(row) = row else {
        return mcp_error(format!(
            "no row at segment {segment_id}, section {:?}, at {}, ordinal {row_ordinal}",
            input.section, input.at
        ));
    };
    mcp_structured(row, format!("row from {}", input.section))
}

/// Accepts a JSON number or a decimal string, same convention
/// `mcp::filter::identifier_value` already uses for a large `i64` input.
fn decimal_i64(field: &str, value: &Value) -> Result<i64, String> {
    match value {
        Value::String(text) => text
            .parse()
            .map_err(|error| format!("{field} is not a valid integer: {text:?} ({error})")),
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| format!("{field} does not fit in a 64-bit signed integer")),
        other => Err(format!(
            "{field} must be a JSON integer or a decimal string, got {other}"
        )),
    }
}

/// Same acceptance as [`decimal_i64`], for the unsigned `row_ordinal`.
fn decimal_u64(field: &str, value: &Value) -> Result<u64, String> {
    match value {
        Value::String(text) => text.parse().map_err(|error| {
            format!("{field} is not a valid non-negative integer: {text:?} ({error})")
        }),
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| format!("{field} does not fit in a 64-bit unsigned integer")),
        other => Err(format!(
            "{field} must be a JSON integer or a decimal string, got {other}"
        )),
    }
}

#[cfg(test)]
mod tests;
