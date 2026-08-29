//! Thin MCP adapter for the shared recorded-events product query.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::api::events::{EventsRepresentation, EventsResult, MAX_EVENTS_WINDOW_MICROS};
use crate::config::Config;
use crate::route::MAX_QUERY_BYTES;

use super::catalog::{EventsInput, FIND_EVENTS_TOOL};
use super::semantics::{invalid_arguments, mcp_error, mcp_error_with, mcp_structured};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    if let Err(error) = check_request_budget(&arguments) {
        return mcp_error(error);
    }
    let input: EventsInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return invalid_arguments(
                FIND_EVENTS_TOOL,
                "from, to, and limit are required; representation defaults to groups and sources narrows the answer",
                error,
            );
        }
    };
    let range = match super::time::resolve_bounded_range(
        &input.from,
        &input.to,
        MAX_EVENTS_WINDOW_MICROS,
    ) {
        Ok(range) => range,
        Err(error) => return mcp_error(error.to_string()),
    };
    let query = match crate::api::events::EventsQuery::normalize(
        range,
        input.sources,
        input.representation,
        input.limit as usize,
    ) {
        Ok(query) => query,
        Err(error) => return mcp_error_with(error.to_string(), error.valid_options()),
    };
    let result = match crate::api::events::prepare(&config.data_root, query)
        .and_then(|prepared| prepared.execute(&|| cancelled()))
    {
        Ok(result) => result,
        Err(error) => return mcp_error(error.to_string()),
    };
    let summary = summary(&result);
    match serde_json::to_value(result) {
        Ok(value) => mcp_structured(value, summary),
        Err(error) => mcp_error(format!("encode events result: {error}")),
    }
}

fn check_request_budget(arguments: &Map<String, Value>) -> Result<(), String> {
    let encoded = serde_json::to_vec(&Value::Object(arguments.clone()))
        .map_err(|error| format!("could not measure events arguments: {error}"))?;
    if encoded.len() <= MAX_QUERY_BYTES {
        return Ok(());
    }
    Err(format!(
        "events arguments exceed {MAX_QUERY_BYTES} encoded bytes; narrow sources or time input"
    ))
}

fn summary(result: &EventsResult) -> String {
    let item = match result.representation() {
        EventsRepresentation::Groups => "group",
        EventsRepresentation::Occurrences => "occurrence",
    };
    format!(
        "Returned {} event {item}{}{}.",
        result.len(),
        if result.len() == 1 { "" } else { "s" },
        if result.truncated() {
            "; result truncated to limit"
        } else {
            ""
        },
    )
}
