//! `kronika_get_row_detail`: one recorded row addressed by an opaque reference.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::Prepared;
use crate::api::snapshot;
use crate::config::Config;
use crate::route::{Order, SnapshotRequest};

use super::catalog::RowDetailInput;
use super::semantics::{mcp_error, mcp_structured};
use crate::api::events::label_event_fields;
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

    let request = SnapshotRequest {
        segment_id: locator.segment_id,
        at: locator.at,
        sections: vec![locator.section.clone()],
        fields: Vec::new(),
        by: Vec::new(),
        direction: Order::Asc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: Some(locator.type_id),
        row_ordinal: None,
    };
    let prepared = match snapshot::prepare(&config.data_root, request, None) {
        Ok(prepared) => prepared,
        Err(_error) => return mcp_error("detail_ref does not identify one recorded row"),
    };
    let Prepared::Snapshot(prepared) = prepared else {
        return mcp_error(
            "internal error: snapshot preparation returned an unexpected response type",
        );
    };
    let row = match prepared
        .fetch_identity_row(locator.row_ordinal, &locator.identity, &|| cancelled())
    {
        Ok(row) => row,
        Err(_error) => return mcp_error("detail_ref does not identify one recorded row"),
    };
    let Some(mut row) = row else {
        return mcp_error("detail_ref does not identify one recorded row");
    };
    // Keep event-code labels identical between list and exact-row reads.
    if let Value::Object(fields) = &mut row {
        fields.remove("segment_id");
        fields.remove("type_id");
        fields.remove("row_ordinal");
        fields.remove("row_key");
        fields.remove("identity");
        fields.remove("detail_locator");
        label_event_fields(&locator.section, fields);
        if let Err(error) = normalize_detail_text(&locator.section, fields) {
            return mcp_error(error);
        }
    }
    mcp_structured(
        row,
        format!(
            "Returned one {} row for the requested detail reference.",
            locator.section
        ),
    )
}

fn normalize_detail_text(section: &str, fields: &mut Map<String, Value>) -> Result<(), String> {
    for (field, value) in fields {
        if crate::api::row_key::is_detail_text(section, field) && !value.is_null() {
            *value = stable_text(std::mem::take(value)).map_err(|error| {
                format!("internal error: {section}.{field} is not stored text: {error}")
            })?;
        }
    }
    Ok(())
}

fn stable_text(value: Value) -> Result<Value, &'static str> {
    match value {
        Value::String(stored_text) => Ok(json!({
            "full_len": stored_text.len().to_string(),
            "sha256": null,
            "stored_text": stored_text,
            "truncated": false,
        })),
        Value::Object(object) if object.get("representation") == Some(&json!("text")) => {
            let stored_text = object.get("stored_text").ok_or("missing stored_text")?;
            let full_len = object.get("full_len").ok_or("missing full_len")?;
            let truncated = object.get("truncated").ok_or("missing truncated")?;
            let sha256 = object.get("sha256").ok_or("missing sha256")?;
            Ok(json!({
                "full_len": full_len,
                "sha256": sha256,
                "stored_text": stored_text,
                "truncated": truncated,
            }))
        }
        _ => Err("expected a UTF-8 string"),
    }
}

#[cfg(test)]
mod tests;
