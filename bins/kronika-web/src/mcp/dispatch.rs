//! Tool name -> handler routing.

use rmcp::model::{CallToolRequestParams, CallToolResult};

use crate::config::Config;

use super::catalog::{
    FIND_POSTGRESQL_INDEXES_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_PROCESSES_TOOL,
    GET_CONTEXT_TOOL, OVERVIEW_TOOL,
};
use super::semantics::mcp_error;
use super::{context, overview, postgresql, processes};

pub(crate) fn dispatch(config: &Config, request: &CallToolRequestParams) -> CallToolResult {
    let arguments = request.arguments.clone().unwrap_or_default();
    match request.name.as_ref() {
        GET_CONTEXT_TOOL => context::call(config, arguments),
        OVERVIEW_TOOL => overview::call(config, arguments),
        FIND_POSTGRESQL_TABLES_TOOL => postgresql::call_tables(config, arguments),
        FIND_POSTGRESQL_INDEXES_TOOL => postgresql::call_indexes(config, arguments),
        FIND_PROCESSES_TOOL => processes::call(config, arguments),
        other => mcp_error(format!("unknown tool: {other}")),
    }
}
