//! `kronika_list_recorded_sections`: logical product sections in recorded data.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::route::Window;

use super::semantics::{mcp_error, mcp_structured, storage_error};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    _cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: super::catalog::GetContextInput =
        match serde_json::from_value(Value::Object(arguments)) {
            Ok(input) => input,
            Err(error) => {
                return super::semantics::invalid_arguments(
                    super::catalog::GET_CONTEXT_TOOL,
                    "an optional section name narrows the answer to one section",
                    error,
                );
            }
        };
    let prepared = match crate::api::catalog::prepare(
        &config.data_root,
        Window::default(),
        config.sources,
        config.synthetic_demo,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return storage_error(&error),
    };
    let mut sections = prepared.recorded_sections();
    let range = match exclusive_recorded_range(prepared.recorded_range()) {
        Ok(range) => range,
        Err(error) => return mcp_error(error),
    };
    if let Some(wanted) = &input.section {
        sections.retain(|section| section["logical_name"] == wanted.as_str());
        if sections.is_empty() {
            let recorded: Vec<String> = prepared
                .recorded_sections()
                .iter()
                .filter_map(|section| section["logical_name"].as_str().map(str::to_owned))
                .collect::<std::collections::BTreeSet<String>>()
                .into_iter()
                .collect();
            return super::semantics::mcp_error_with(
                format!(
                    "no recorded section named {wanted:?}; recorded: {}",
                    recorded.join(", ")
                ),
                recorded,
            );
        }
    }
    mcp_structured(json!({
        "recorded_from": range.map(|(from, _to)| from.to_string()),
        "recorded_to": range.map(|(_from, to)| to.to_string()),
        "sections": sections,
    }))
}

fn exclusive_recorded_range(range: Option<(i64, i64)>) -> Result<Option<(i64, i64)>, &'static str> {
    range
        .map(|(from, to)| {
            to.checked_add(1)
                .map(|to_exclusive| (from, to_exclusive))
                .ok_or("last recorded timestamp cannot form an exclusive upper bound")
        })
        .transpose()
}

#[cfg(test)]
mod tests;
