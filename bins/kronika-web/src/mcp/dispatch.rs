//! Tool name -> handler routing.

use rmcp::model::{CallToolRequestParams, CallToolResult};

use crate::config::Config;

use super::catalog::{
    FIND_EVENTS_TOOL, FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL,
    FIND_POSTGRESQL_INDEXES_TOOL, FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL,
    FIND_POSTGRESQL_STATEMENTS_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
    FIND_PROCESSES_TOOL, GET_CONTEXT_TOOL, GET_ROW_DETAIL_TOOL, OVERVIEW_TOOL,
};
use super::semantics::mcp_error;
use super::{context, events, overview, postgresql, processes, row_detail};

pub(crate) fn dispatch(config: &Config, request: CallToolRequestParams) -> CallToolResult {
    let arguments = request.arguments.unwrap_or_default();
    match request.name.as_ref() {
        GET_CONTEXT_TOOL => context::call(config, arguments),
        OVERVIEW_TOOL => overview::call(config, arguments),
        FIND_POSTGRESQL_TABLES_TOOL => postgresql::call_tables(config, arguments),
        FIND_POSTGRESQL_INDEXES_TOOL => postgresql::call_indexes(config, arguments),
        FIND_POSTGRESQL_ACTIVITY_TOOL => postgresql::call_activity(config, arguments),
        FIND_POSTGRESQL_LOCKS_TOOL => postgresql::call_locks(config, arguments),
        FIND_POSTGRESQL_VACUUM_TOOL => postgresql::call_vacuum(config, arguments),
        FIND_POSTGRESQL_DATABASES_TOOL => postgresql::call_databases(config, arguments),
        FIND_POSTGRESQL_STATEMENTS_TOOL => postgresql::call_statements(config, arguments),
        FIND_POSTGRESQL_PLANS_TOOL => postgresql::call_plans(config, arguments),
        FIND_PROCESSES_TOOL => processes::call(config, arguments),
        GET_ROW_DETAIL_TOOL => row_detail::call(config, arguments),
        FIND_EVENTS_TOOL => events::call(config, arguments),
        other => mcp_error(format!("unknown tool: {other}")),
    }
}
