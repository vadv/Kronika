//! `kronika_get_row_detail`: one exact row by its physical locator, through
//! `PreparedSnapshot::fetch_exact_row`.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::api::Prepared;
use crate::api::snapshot;
use crate::config::Config;
use crate::route::{Order, SnapshotRequest};

use super::catalog::RowDetailInput;
use super::event_labels::label_event_fields;
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
    let at = match decimal_i64("at", &input.at) {
        Ok(at) => at,
        Err(error) => return mcp_error(error),
    };
    let type_id = match decimal_u32("type_id", &input.type_id) {
        Ok(type_id) => type_id,
        Err(error) => return mcp_error(error),
    };

    let request = SnapshotRequest {
        segment_id,
        at,
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
        type_id: Some(type_id),
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
    let Some(mut row) = row else {
        return mcp_error(format!(
            "no row at segment {segment_id}, section {:?}, at {at}, ordinal {row_ordinal}",
            input.section
        ));
    };
    // Same numeric log-event codes `kronika_find_events` labels
    // (`mcp/event_labels.rs`), so a row fetched by exact locator carries
    // the same `<field>_label` siblings a listing row already has.
    if let Value::Object(fields) = &mut row {
        label_event_fields(&input.section, fields);
    }
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

/// Same acceptance as [`decimal_i64`], for the unsigned `type_id`. `type_id`
/// fits comfortably inside JSON's safe-integer range on its own, but a
/// `kronika_find_*` row renders it as a decimal string alongside
/// `segment_id`/`row_ordinal`/`at`, so it takes the same string-or-number
/// input here too — otherwise a caller copying a row's locator fields
/// straight into this tool's arguments would hit a type mismatch on this
/// one field alone.
fn decimal_u32(field: &str, value: &Value) -> Result<u32, String> {
    match value {
        Value::String(text) => text.parse().map_err(|error| {
            format!("{field} is not a valid non-negative integer: {text:?} ({error})")
        }),
        Value::Number(number) => number
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| format!("{field} does not fit in a 32-bit unsigned integer")),
        other => Err(format!(
            "{field} must be a JSON integer or a decimal string, got {other}"
        )),
    }
}

#[cfg(test)]
mod tests;
