//! `CallToolResult` helpers and decimal-string serialization for 64-bit integers.

use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;
use serde_json::Value;

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
    let mut body = serde_json::Map::new();
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

/// Rejects arguments a parameterless tool's closed-object schema forbids:
/// the schema says `additionalProperties: false`, so the runtime must not
/// quietly accept `{"unexpected": true}` either.
pub(crate) fn parameterless<T: serde::de::DeserializeOwned>(
    name: &str,
    arguments: serde_json::Map<String, Value>,
) -> Result<(), CallToolResult> {
    serde_json::from_value::<T>(Value::Object(arguments))
        .map(|_input| ())
        .map_err(|error| mcp_error(format!("invalid arguments for {name}: {error}")))
}

/// The largest encoded `structuredContent` a tool returns. A legal
/// 5,000-row request over wide text columns can otherwise encode hundreds
/// of megabytes; past this ceiling the caller gets an error naming the way
/// out instead of a response no MCP client will digest.
const RESPONSE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// An `io::Write` that only counts, failing once the budget is spent — so
/// oversized results abort during measurement instead of allocating fully.
struct ByteBudget {
    remaining: usize,
}

impl std::io::Write for ByteBudget {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(std::io::Error::other("over budget"));
        }
        self.remaining -= buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Places data in `structuredContent` with the summary as the only text
/// content, built without `CallToolResult::structured`'s eager JSON mirror.
/// A result whose encoding exceeds [`RESPONSE_MAX_BYTES`] comes back as an
/// error instead.
pub(crate) fn mcp_structured(value: Value, summary: impl Into<String>) -> CallToolResult {
    structured_within_budget(value, summary, None)
}

/// [`mcp_structured`] for row-listing tools: the over-budget error names
/// the requested knob value and the halved retry.
pub(crate) fn mcp_structured_bounded(
    value: Value,
    summary: impl Into<String>,
    knob: &str,
    requested: usize,
) -> CallToolResult {
    structured_within_budget(value, summary, Some((knob, requested)))
}

fn structured_within_budget(
    value: Value,
    summary: impl Into<String>,
    knob: Option<(&str, usize)>,
) -> CallToolResult {
    let mut budget = ByteBudget {
        remaining: RESPONSE_MAX_BYTES,
    };
    if serde_json::to_writer(&mut budget, &value).is_err() {
        return mcp_error(over_budget_message(knob));
    }
    let mut result = CallToolResult::success(vec![ContentBlock::text(summary.into())]);
    result.structured_content = Some(value);
    result
}

/// Names the way out of an oversized result; with a known knob, names
/// the halved value to retry with.
fn over_budget_message(knob: Option<(&str, usize)>) -> String {
    match knob {
        Some((name, requested)) if requested > 1 => format!(
            "result exceeds {RESPONSE_MAX_BYTES} encoded bytes at {name}={requested}: \
             retry with {name}={} or add filters",
            requested / 2
        ),
        _ => format!(
            "result exceeds {RESPONSE_MAX_BYTES} encoded bytes: lower `limit`/`top` or add \
             filters"
        ),
    }
}

#[cfg(test)]
mod tests;
