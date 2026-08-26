//! `kronika_get_context`: physical section layouts found in recorded segments.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::route::Window;

use super::semantics::{DecimalI64, mcp_error, mcp_structured};

pub(crate) fn call(
    config: &Config,
    _arguments: Map<String, Value>,
    _cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
    let range = prepared.recorded_range();
    let summary = format!("{} physical section layouts recorded", sections.len());
    mcp_structured(
        json!({
            "recorded_from": range.map(|(from, _to)| DecimalI64(from)),
            "recorded_to": range.map(|(_from, to)| DecimalI64(to)),
            "sections": sections,
        }),
        summary,
    )
}
