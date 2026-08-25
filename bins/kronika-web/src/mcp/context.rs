//! `kronika_get_context`: what this host actually recorded.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::route::Window;

use super::semantics::{mcp_error, mcp_structured};

pub(crate) fn call(config: &Config, _arguments: Map<String, Value>) -> CallToolResult {
    let prepared = match crate::api::catalog::prepare(
        &config.data_root,
        Window::default(),
        config.sources,
        config.synthetic_demo,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let sections = prepared.recorded_sections();
    let summary = format!("{} recorded sections", sections.len());
    mcp_structured(json!({ "sections": sections }), summary)
}
