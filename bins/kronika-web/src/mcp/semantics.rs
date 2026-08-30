//! `CallToolResult` helpers and decimal-string serialization for 64-bit integers.

use std::collections::BTreeMap;

use rmcp::model::{CallToolResult, ContentBlock};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::budget::ByteBudget;
use crate::route::MAX_QUERY_BYTES;

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct FinderOutput {
    #[schemars(with = "Vec<BTreeMap<String, Value>>")]
    rows: Vec<Value>,
    truncated: bool,
}

pub(crate) fn finder_output(rows: Vec<Value>, truncated: bool) -> Result<Value, CallToolResult> {
    serde_json::to_value(FinderOutput { rows, truncated })
        .map_err(|error| mcp_error(format!("finder result encoding failed: {error}")))
}

pub(crate) fn finder_summary(noun: &str, row_count: usize, truncated: bool) -> String {
    let suffix = if truncated { "; truncated" } else { "" };
    format!(
        "Returned {row_count} recorded {noun} row{}{suffix}.",
        if row_count == 1 { "" } else { "s" }
    )
}

/// Serializes every `i64` as decimal text so JSON clients retain exact 64-bit
/// values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecimalI64(pub(crate) i64);

impl Serialize for DecimalI64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// Returns an in-band tool execution error. The text is mirrored into
/// `structuredContent` as `{"record":"error","message":…}`, so a client
/// reading only the structure sees the error instead of an empty result.
pub(crate) fn mcp_error(message: impl Into<String>) -> CallToolResult {
    mcp_error_with(message, Vec::new())
}

/// An error whose structured mirror also carries `valid_options` — the
/// choices the refused input can be replaced with, ready to pick without
/// parsing the text.
pub(crate) fn mcp_error_with(
    message: impl Into<String>,
    valid_options: Vec<String>,
) -> CallToolResult {
    let message = message.into();
    let mut body = Map::new();
    body.insert("record".to_owned(), Value::String("error".to_owned()));
    body.insert("message".to_owned(), Value::String(message.clone()));
    if !valid_options.is_empty() {
        body.insert(
            "valid_options".to_owned(),
            Value::Array(valid_options.into_iter().map(Value::String).collect()),
        );
    }
    let mut result = CallToolResult::error(vec![ContentBlock::text(message)]);
    result.structured_content = Some(Value::Object(body));
    result
}

/// A batch refusal naming the zero-based ranking whose validation, resource,
/// or reservation failed.
pub(crate) fn mcp_error_indexed(
    message: impl Into<String>,
    ranking_index: usize,
) -> CallToolResult {
    mcp_error_indexed_with(message, ranking_index, Vec::new())
}

/// A batch refusal with both its zero-based ranking and known replacements.
pub(crate) fn mcp_error_indexed_with(
    message: impl Into<String>,
    ranking_index: usize,
    valid_options: Vec<String>,
) -> CallToolResult {
    let message = message.into();
    let mut result = mcp_error_with(message, valid_options);
    if let Some(Value::Object(body)) = result.structured_content.as_mut() {
        body.insert("ranking_index".to_owned(), serde_json::json!(ranking_index));
    }
    result
}

/// A storage error as a tool answer; an unknown section or column also
/// names the tool that lists the valid ones.
pub(crate) fn storage_error(error: &crate::api::ApiError) -> CallToolResult {
    let hinted = matches!(
        error,
        crate::api::ApiError::NoSuchSection | crate::api::ApiError::NoSuchColumn(_)
    );
    let mut message = error.to_string();
    if hinted {
        message.push_str("; kronika_get_context lists recorded sections and their fields");
    }
    mcp_error(message)
}

pub(crate) fn finder_storage_error(
    logical_name: &str,
    error: &crate::api::ApiError,
) -> CallToolResult {
    if let crate::api::ApiError::NoSuchColumn(field) = error {
        return mcp_error(format!(
            "no such sort field for {logical_name}: {field}; kronika_get_context lists the section's fields"
        ));
    }
    storage_error(error)
}

/// Rejected tool arguments with the tool's one-line usage appended: the
/// serde text alone names a field, not the shape of a working call.
pub(crate) fn invalid_arguments(
    tool: &str,
    usage: &str,
    error: impl std::fmt::Display,
) -> CallToolResult {
    mcp_error(format!(
        "invalid arguments for {tool}: {error}. Usage: {usage}"
    ))
}

/// Accepts the inclusive range `1..=cap` and never clamps.
pub(crate) fn bounded_limit(name: &str, value: u32, cap: usize) -> Result<usize, CallToolResult> {
    let value = value as usize;
    if (1..=cap).contains(&value) {
        Ok(value)
    } else {
        Err(mcp_error(format!(
            "{name} must be between 1 and {cap}, got {value}"
        )))
    }
}

pub(crate) fn arguments_within_budget(arguments: &Map<String, Value>) -> bool {
    let mut budget = ByteBudget::new(MAX_QUERY_BYTES);
    serde_json::to_writer(&mut budget, arguments).is_ok()
}

/// Places data in `structuredContent` with the summary as the only text
/// content, built without `CallToolResult::structured`'s eager JSON mirror.
pub(crate) fn mcp_structured(value: Value, summary: impl Into<String>) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(summary.into())]);
    result.structured_content = Some(value);
    result
}

#[cfg(test)]
mod tests;
