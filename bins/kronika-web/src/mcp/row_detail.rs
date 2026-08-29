//! `kronika_get_row_detail`: one recorded row addressed by its physical locator.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::api::Prepared;
use crate::api::snapshot;
use crate::config::Config;
use crate::route::{Order, SnapshotRequest};

use super::catalog::RowDetailInput;
use super::semantics::{mcp_error, mcp_structured};
use crate::api::events::label_event_fields;

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: RowDetailInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                super::catalog::GET_ROW_DETAIL_TOOL,
                "section, segment_id, at, type_id, and row_ordinal are required; row_key comes from the find row",
                error,
            );
        }
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
        Err(error) => return super::semantics::storage_error(&error),
    };
    let Prepared::Snapshot(prepared) = prepared else {
        return mcp_error(
            "internal error: snapshot preparation returned an unexpected response type",
        );
    };
    let row = match prepared.fetch_exact_row(&|| cancelled()) {
        Ok(row) => row,
        Err(error) => return super::semantics::storage_error(&error),
    };
    let Some(mut row) = row else {
        return mcp_error(format!(
            "no recorded row matched segment_id={segment_id}, section={:?}, type_id={type_id}, at={at}, row_ordinal={row_ordinal}",
            input.section,
        ));
    };
    if let Some(column) = crate::api::row_key::discriminator(&input.section) {
        let Value::Object(fields) = &row else {
            return mcp_error("internal error: the fetched row is not an object");
        };
        let actual = fields.get(column).cloned().unwrap_or(Value::Null);
        if let Err(error) =
            crate::api::row_key::verify(&input.section, column, input.row_key.as_ref(), &actual)
        {
            return mcp_error(error);
        }
    }
    // Keep event-code labels identical between list and exact-row reads.
    if let Value::Object(fields) = &mut row {
        label_event_fields(&input.section, fields);
    }
    mcp_structured(
        row,
        format!(
            "Returned one {} row at the requested locator.",
            input.section
        ),
    )
}

/// Accepts a JSON integer or decimal string.
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

/// Accepts a non-negative JSON integer or decimal string.
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

/// Accepts `type_id` in the number-or-decimal-string form emitted by find
/// tools.
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
