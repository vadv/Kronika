//! Response-envelope pieces shared by every MCP tool handler: numbers that
//! may exceed JSON's safe-integer range, and the one error shape the whole
//! catalog uses.

use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;

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

/// A `u64` serialized as a decimal string — same reasoning as `DecimalI64`,
/// for byte counters and other unsigned quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecimalU64(pub(crate) u64);

impl Serialize for DecimalU64 {
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

#[cfg(test)]
mod tests;
