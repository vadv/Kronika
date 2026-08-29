//! `kronika_find_processes`: bounded sorting and filtering of recorded
//! `os_process` rows relative to the greatest-ID segment.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::Prepared;
use crate::api::snapshot;
use crate::api::snapshot::ProcessRowOut;
use crate::config::Config;
use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Order, SnapshotRequest};

use super::catalog::{ProcessesInput, SortInput};
use super::filter::{FilterInput, build_search};
use super::postgresql::current_segment;
use super::semantics::{DecimalI64, bounded_limit, mcp_error, mcp_structured_bounded};

const LOGICAL_NAME: &str = "os_process";

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: ProcessesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                super::catalog::FIND_PROCESSES_TOOL,
                "limit is required; filters and sort are optional",
                error,
            );
        }
    };
    call_with(config, &input.filters, input.sort, input.limit, cancelled)
}

fn call_with(
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let limit = match bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE) {
        Ok(limit) => limit,
        Err(error) => return error,
    };
    let search = match build_search(LOGICAL_NAME, filters) {
        Ok(search) => search,
        Err(refusal) => return refusal.into_error(),
    };
    let by = match &sort {
        Some(sort) => match super::postgresql::plain_sort_token(LOGICAL_NAME, &sort.field) {
            Ok(token) => vec![token],
            Err(error) => return mcp_error(error),
        },
        None => Vec::new(),
    };
    let (segment_id, at) = match current_segment(&config.data_root, LOGICAL_NAME) {
        Ok(Some(segment)) => segment,
        Ok(None) => return super::postgresql::no_recorded_rows(LOGICAL_NAME),
        Err(error) => return mcp_error(error.to_string()),
    };
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
        return mcp_error(
            "internal error: snapshot preparation returned an unexpected response type",
        );
    };
    let prepared = match prepared.with_search(search) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let (rows, has_more) = match prepared.compute_process_rows(limit, &|| cancelled()) {
        Ok(result) => result,
        Err(error) => return mcp_error(error.to_string()),
    };

    let row_count = rows.len();
    let rows: Vec<Value> = rows.into_iter().map(row_to_json).collect();
    let summary = format!(
        "Returned {row_count} recorded process row{}{}.",
        if row_count == 1 { "" } else { "s" },
        if has_more {
            "; result truncated to limit"
        } else {
            ""
        },
    );
    mcp_structured_bounded(
        json!({ "rows": rows, "has_more": has_more, "as_of": DecimalI64(at) }),
        summary,
        "limit",
        limit,
    )
}

/// Flattens projected fields, overwrites `pid`/`ppid` from the typed
/// identity, and appends `row_key` plus the decimal-string locator fields
/// accepted by `kronika_get_row_detail`.
fn row_to_json(row: ProcessRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    object.insert("pid".to_owned(), json!(row.pid));
    object.insert(
        "ppid".to_owned(),
        row.ppid.map_or(Value::Null, |ppid| json!(ppid)),
    );
    crate::api::row_key::attach("os_process", &mut object);
    object.insert("segment_id".to_owned(), json!(row.segment_id.to_string()));
    object.insert("type_id".to_owned(), json!(row.type_id.to_string()));
    object.insert("row_ordinal".to_owned(), json!(row.row_ordinal.to_string()));
    object.insert("at".to_owned(), json!(row.at.to_string()));
    Value::Object(object)
}
