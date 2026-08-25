//! `kronika_find_processes`: bounded top-N reads of the current `os_process`
//! snapshot, through `compute_process_rows`.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::Prepared;
use crate::api::snapshot;
use crate::api::snapshot::ProcessRowOut;
use crate::config::Config;
use crate::route::{Order, SnapshotRequest};

use super::catalog::{ProcessesInput, SortInput};
use super::filter::{FilterInput, build_search};
use super::postgresql::current_segment;
use super::semantics::{mcp_error, mcp_structured};

const LOGICAL_NAME: &str = "os_process";

pub(crate) fn call(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: ProcessesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_with(config, &input.filters, input.sort, input.limit)
}

fn call_with(
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
) -> CallToolResult {
    let search = match build_search(LOGICAL_NAME, filters) {
        Ok(search) => search,
        Err(error) => return mcp_error(error),
    };
    let (segment_id, at) = match current_segment(&config.data_root) {
        Ok(segment) => segment,
        Err(error) => return mcp_error(error.to_string()),
    };
    let by = sort
        .as_ref()
        .map_or_else(Vec::new, |sort| vec![sort.field.clone()]);
    let direction = sort.map_or(Order::Asc, |sort| sort.direction.into());

    let request = SnapshotRequest {
        segment_id,
        at,
        sections: vec![LOGICAL_NAME.to_owned()],
        fields: Vec::new(),
        by,
        direction,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
    };
    let prepared = match snapshot::prepare(&config.data_root, request, None) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let Prepared::Snapshot(prepared) = prepared else {
        return mcp_error("snapshot request did not prepare a process snapshot");
    };
    let prepared = match prepared.with_search(search) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let (rows, has_more) = match prepared.compute_process_rows(limit as usize, &|| false) {
        Ok(result) => result,
        Err(error) => return mcp_error(error.to_string()),
    };

    let row_count = rows.len();
    let rows: Vec<Value> = rows.into_iter().map(row_to_json).collect();
    let summary = format!(
        "{row_count} process row{}{}",
        if row_count == 1 { "" } else { "s" },
        if has_more { ", more available" } else { "" },
    );
    mcp_structured(json!({ "rows": rows, "has_more": has_more }), summary)
}

/// Flattens one process row into a single keyed JSON object: `fields`
/// first (already keyed and JSON-rendered by `row_record`, and already
/// carrying `pid`/`ppid` under their own names), then `pid`/`ppid` written
/// again from `ProcessRowOut`'s own typed copies so they stay present
/// even if a future field-list change ever narrowed `fields` — same
/// "identity wins on collision" flattening `postgresql.rs`'s `row_to_json`
/// uses for `RelationRow`. `segment_id`/`type_id`/`row_ordinal`/`at` are
/// written out as decimal strings, the same convention
/// `kronika_get_row_detail` (`mcp/row_detail.rs`) uses for these same four
/// fields, so a caller can copy them straight into that tool's arguments.
fn row_to_json(row: ProcessRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    object.insert("pid".to_owned(), json!(row.pid));
    object.insert(
        "ppid".to_owned(),
        row.ppid.map_or(Value::Null, |ppid| json!(ppid)),
    );
    object.insert("segment_id".to_owned(), json!(row.segment_id.to_string()));
    object.insert("type_id".to_owned(), json!(row.type_id.to_string()));
    object.insert("row_ordinal".to_owned(), json!(row.row_ordinal.to_string()));
    object.insert("at".to_owned(), json!(row.at.to_string()));
    Value::Object(object)
}
