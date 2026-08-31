//! `kronika_get_row_detail`: one recorded row addressed by an opaque reference.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::config::Config;

use super::catalog::RowDetailInput;
use super::semantics::{detail_ref_error, mcp_error, mcp_structured};
#[cfg(test)]
use crate::api::row_detail::normalize_detail_text;
use crate::api::row_key::DetailLocator;

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: RowDetailInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(_error) => {
            return mcp_error("invalid arguments: pass only one copied detail_ref string");
        }
    };
    let locator = match DetailLocator::from_detail_ref(&input.detail_ref) {
        Ok(locator) => locator,
        Err(_error) => return mcp_error("invalid detail_ref"),
    };

    let prepared = match crate::api::row_detail::prepare_locator(&config.data_root, locator) {
        Ok(prepared) => prepared,
        Err(error) => return detail_ref_error(&error),
    };
    let row = match prepared.resolve(&|| cancelled()) {
        Ok(row) => row,
        Err(error) => return detail_ref_error(&error),
    };
    let Some(row) = row else {
        return mcp_error("detail_ref does not identify one recorded row");
    };
    mcp_structured(
        Value::Object(row.fields),
        format!(
            "Returned one {} row for the requested detail reference.",
            row.section
        ),
    )
}

#[cfg(test)]
mod tests;
