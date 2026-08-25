//! `kronika_find_postgresql_tables` and `kronika_find_postgresql_indexes`:
//! bounded top-N reads of the same aggregate/filter/sort pipeline HTTP's
//! paged relation snapshot streams, through `compute_relation_rows`.
//!
//! `kronika_find_postgresql_activity`, `_locks`, `_vacuum` and `_databases`
//! read the same pipeline's plain (non-grouped) side, through
//! `compute_plain_rows` — one row per backend/lock/vacuum/database, no
//! `group` argument, since none of the four has a relation identity to roll
//! up.

use kronika_reader::Reader;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::snapshot;
use crate::api::snapshot::PlainRowOut;
use crate::api::snapshot::relation::{RelationKind, RelationRow};
use crate::api::{ApiError, Prepared};
use crate::config::Config;
use crate::route::{Order, RelationGroup, SnapshotRequest};

use super::catalog::{
    ActivityInput, DatabasesInput, IndexesInput, LocksInput, SortInput, TablesInput, VacuumInput,
};
use super::filter::{FilterInput, build_search};
use super::semantics::{mcp_error, mcp_structured};

pub(crate) fn call_tables(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: TablesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call(
        RelationKind::Tables,
        config,
        input.group.into(),
        &input.filters,
        input.sort,
        input.limit,
    )
}

pub(crate) fn call_indexes(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: IndexesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call(
        RelationKind::Indexes,
        config,
        input.group.into(),
        &input.filters,
        input.sort,
        input.limit,
    )
}

fn call(
    kind: RelationKind,
    config: &Config,
    group: RelationGroup,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
) -> CallToolResult {
    let search = match build_search(kind.logical_name(), filters) {
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
        sections: vec![kind.logical_name().to_owned()],
        fields: Vec::new(),
        by,
        direction,
        group: Some(group),
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
        return mcp_error("snapshot request did not prepare a relation snapshot");
    };
    let prepared = match prepared.with_search(search) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let (rows, has_more) = match prepared.compute_relation_rows(limit as usize, &|| false) {
        Ok(result) => result,
        Err(error) => return mcp_error(error.to_string()),
    };

    let row_count = rows.len();
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|row| row_to_json(row, kind, group))
        .collect();
    let summary = format!(
        "{row_count} {} row{}{}",
        kind.logical_name(),
        if row_count == 1 { "" } else { "s" },
        if has_more { ", more available" } else { "" },
    );
    mcp_structured(json!({ "rows": rows, "has_more": has_more }), summary)
}

/// The most recently started segment, and the latest timestamp it carries.
/// A `find_*` tool reads the live state, not an explicit archived segment,
/// so it resolves "current" itself instead of taking a `segment_id`/`at`
/// argument the way the HTTP snapshot endpoint does. Shared with
/// `processes.rs`, which reads the same "current" notion for `os_process` —
/// one resolution of "current segment", not a second copy of it.
pub(crate) fn current_segment(root: &std::path::Path) -> Result<(i64, i64), ApiError> {
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments(..)?;
    let segment = listing
        .segments
        .iter()
        .max_by_key(|segment| segment.id())
        .ok_or(ApiError::NoSuchSegment)?;
    Ok((segment.id(), segment.max_ts()))
}

/// Flattens one relation row into a single keyed JSON object: metric
/// fields first, then the group key's own identity fields spread over
/// them (a key field always wins on a name collision, e.g. `relname`
/// appearing both as an identity field and as a projected column).
fn row_to_json(row: RelationRow, kind: RelationKind, group: RelationGroup) -> Value {
    let mut object = Map::new();
    for (name, metric) in row.metrics {
        object.insert(name, metric.map_or(Value::Null, |metric| metric.json()));
    }
    if let Value::Object(key_fields) = row.key.json(kind, group) {
        object.extend(key_fields);
    }
    Value::Object(object)
}

pub(crate) fn call_activity(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: ActivityInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_plain(
        "pg_stat_activity",
        config,
        &input.filters,
        input.sort,
        input.limit,
    )
}

pub(crate) fn call_locks(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: LocksInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_plain("pg_locks", config, &input.filters, input.sort, input.limit)
}

pub(crate) fn call_vacuum(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: VacuumInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_plain(
        "pg_stat_progress_vacuum",
        config,
        &input.filters,
        input.sort,
        input.limit,
    )
}

pub(crate) fn call_databases(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: DatabasesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_plain(
        "pg_stat_database",
        config,
        &input.filters,
        input.sort,
        input.limit,
    )
}

/// Shared body for `call_activity`/`call_locks`/`call_vacuum`/
/// `call_databases`: the same filter/sort/limit -> `SnapshotRequest` ->
/// `compute_plain_rows` pipeline as `call` above, minus the `group`
/// argument none of the four takes.
fn call_plain(
    logical_name: &str,
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
) -> CallToolResult {
    let search = match build_search(logical_name, filters) {
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
        sections: vec![logical_name.to_owned()],
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
        return mcp_error("snapshot request did not prepare a snapshot");
    };
    let prepared = match prepared.with_search(search) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let (rows, has_more) = match prepared.compute_plain_rows(limit as usize, &|| false) {
        Ok(result) => result,
        Err(error) => return mcp_error(error.to_string()),
    };

    let row_count = rows.len();
    let rows: Vec<Value> = rows.into_iter().map(plain_row_to_json).collect();
    let summary = format!(
        "{row_count} {logical_name} row{}{}",
        if row_count == 1 { "" } else { "s" },
        if has_more { ", more available" } else { "" },
    );
    mcp_structured(json!({ "rows": rows, "has_more": has_more }), summary)
}

/// Flattens one plain gauge/counter row into a single keyed JSON object:
/// `row.fields` already holds every projected field under its own name
/// (`row_record`'s own keying, same as `ProcessRowOut`), so this only adds
/// the `kronika_get_row_detail` locator fields — same convention
/// `processes.rs`'s `row_to_json` uses for `ProcessRowOut`, minus the
/// `pid`/`ppid` identity `PlainRowOut` does not carry.
fn plain_row_to_json(row: PlainRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    object.insert("segment_id".to_owned(), json!(row.segment_id.to_string()));
    object.insert("type_id".to_owned(), json!(row.type_id.to_string()));
    object.insert("row_ordinal".to_owned(), json!(row.row_ordinal.to_string()));
    object.insert("at".to_owned(), json!(row.at.to_string()));
    Value::Object(object)
}
