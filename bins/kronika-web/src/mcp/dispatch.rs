//! Tool name -> handler routing.

use rmcp::model::{CallToolRequestParams, CallToolResult};

use crate::config::Config;

use super::catalog::{GET_CONTEXT_TOOL, OVERVIEW_TOOL};
use super::semantics::mcp_error;
use super::{context, overview};

pub(crate) fn dispatch(config: &Config, request: &CallToolRequestParams) -> CallToolResult {
    let arguments = request.arguments.clone().unwrap_or_default();
    match request.name.as_ref() {
        GET_CONTEXT_TOOL => context::call(config, arguments),
        OVERVIEW_TOOL => overview::call(config, arguments),
        other => mcp_error(format!("unknown tool: {other}")),
    }
}
