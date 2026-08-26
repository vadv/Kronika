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

/// Returns an in-band tool execution error with text content only.
pub(crate) fn mcp_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
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

/// Places data in `structuredContent` with the summary as the only text
/// content, built without `CallToolResult::structured`'s eager JSON mirror.
pub(crate) fn mcp_structured(value: Value, summary: impl Into<String>) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(summary.into())]);
    result.structured_content = Some(value);
    result
}

#[cfg(test)]
mod tests;
