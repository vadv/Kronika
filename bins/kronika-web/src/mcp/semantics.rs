//! Response-envelope pieces shared by every MCP tool handler: numbers that
//! may exceed JSON's safe-integer range, and the one error shape the whole
//! catalog uses.

use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;
use serde_json::Value;

/// An `i64` serialized as a decimal string. JSON numbers lose precision
/// above 2^53; Kronika's microsecond timestamps and segment ids exceed it
/// routinely, so every value in that range crosses the wire as a string.
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

/// The one error shape every MCP tool handler in this catalog uses: a
/// factual message, no structured content, marked as an error result.
pub(crate) fn mcp_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// Rejects a `limit`/`top` argument outside `1..=cap`, the same
/// reject-not-clamp contract `bounded()` (`route.rs`) already enforces on
/// the HTTP side for the identical numeric ceiling. Called once per tool
/// at the point where every one of its call paths converges on the raw
/// argument, before it can size a `Vec` or a query.
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

/// The one success shape every MCP tool handler in this catalog uses:
/// `structuredContent` carries the data, `content` carries one short
/// factual sentence. `CallToolResult::structured` alone puts the whole JSON
/// value into `content` as text, duplicating `structuredContent` — this
/// replaces that text with `summary` instead.
pub(crate) fn mcp_structured(value: Value, summary: impl Into<String>) -> CallToolResult {
    let mut result = CallToolResult::structured(value);
    result.content = vec![ContentBlock::text(summary.into())];
    result
}

#[cfg(test)]
mod tests;
