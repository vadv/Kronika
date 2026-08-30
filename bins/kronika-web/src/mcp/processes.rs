//! `kronika_find_processes`: bounded sorting and filtering of recorded rows.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::snapshot::ProcessRowOut;
use crate::api::snapshot::selector::{FinderOrder, FinderQuery, FinderSurface, execute_processes};
use crate::api::time::SnapshotPoint;
use crate::config::Config;
use crate::route::MAX_SNAPSHOT_PAGE_SIZE;

use super::catalog::{ProcessesInput, SortInput};
use super::filter::{FilterInput, build_search};
use super::semantics::{
    bounded_limit, finder_output, finder_storage_error, finder_summary, mcp_structured,
};
use super::time::resolve_point;

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
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let point = match resolve_point(input.at.as_ref()) {
        Ok(point) => point,
        Err(error) => {
            return super::semantics::invalid_arguments(
                super::catalog::FIND_PROCESSES_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    call_with(
        config,
        point,
        &input.filters,
        input.sort,
        input.limit,
        cancelled,
    )
}

fn call_with(
    config: &Config,
    point: SnapshotPoint,
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
    let query = FinderQuery {
        surface: FinderSurface::Processes,
        point,
        search,
        order: sort.map(|sort| FinderOrder {
            field: sort.field,
            direction: sort.direction.into(),
        }),
        group: None,
        limit,
    };
    let result = match execute_processes(&config.data_root, query, &|| cancelled()) {
        Ok(result) => result,
        Err(error) => return finder_storage_error(LOGICAL_NAME, &error),
    };

    let row_count = result.rows.len();
    let rows: Vec<Value> = result.rows.into_iter().map(row_to_json).collect();
    let summary = finder_summary("process", row_count, result.truncated);
    let output = match finder_output(rows, result.truncated) {
        Ok(output) => output,
        Err(error) => return error,
    };
    mcp_structured(output, summary)
}

/// Keeps compact fields, overwrites `pid`/`ppid` from the typed identity, and
/// appends the ready row-detail input.
fn row_to_json(row: ProcessRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    object.insert("pid".to_owned(), json!(row.pid));
    object.insert(
        "ppid".to_owned(),
        row.ppid.map_or(Value::Null, |ppid| json!(ppid)),
    );
    let locator = crate::api::row_key::detail_locator(
        LOGICAL_NAME,
        row.segment_id,
        row.at,
        row.type_id,
        row.row_ordinal,
        &object,
    );
    object.remove("cmdline");
    object.insert("detail_locator".to_owned(), json!(locator));
    Value::Object(object)
}
