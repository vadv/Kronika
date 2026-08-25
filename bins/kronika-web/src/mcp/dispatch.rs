//! Tool name -> handler routing. Placeholder until a later change adds the
//! real `kronika_get_context` and `kronika_overview` handlers.

use rmcp::model::{CallToolRequestParams, CallToolResult};

use super::semantics::mcp_error;
use crate::config::Config;

pub(crate) fn dispatch(_config: &Config, request: &CallToolRequestParams) -> CallToolResult {
    mcp_error(format!("not yet implemented: {}", request.name))
}
