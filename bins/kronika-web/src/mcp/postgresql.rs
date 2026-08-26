//! `kronika_find_postgresql_tables` and `kronika_find_postgresql_indexes`:
//! bounded top-N reads of the same aggregate/filter/sort pipeline HTTP's
//! paged relation snapshot streams, through `compute_relation_rows`.
//!
//! `kronika_find_postgresql_activity`, `_locks`, `_vacuum` and `_databases`
//! read the same pipeline's plain (non-grouped) side, through
//! `compute_plain_rows` — one row per backend/lock/vacuum/database, no
//! `group` argument, since none of the four has a relation identity to roll
//! up.
//!
//! `call_statements`/`call_plans` read the same plain path for
//! `pg_stat_statements`/`pg_store_plans`, then layer `derived_*` per-row
//! ratio fields (mean execution time per call, rows per call, buffer hit
//! fraction, ...) on top of `row_record`'s already-rated cumulative
//! columns — plain arithmetic on two fields of the same row, not a new
//! predecessor lookup. Their input types live in `catalog.rs` alongside
//! every other tool's, since the schema descriptions there are what
//! documents the `derived_*` scale to a calling model.

use std::collections::BTreeMap;

use kronika_reader::Reader;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::snapshot;
use crate::api::snapshot::PlainRowOut;
use crate::api::snapshot::relation::{RelationKind, RelationRow};
use crate::api::{ApiError, Prepared};
use crate::config::Config;
use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Order, RelationGroup, SnapshotRequest};

use super::catalog::{
    ActivityInput, DatabasesInput, IndexesInput, LocksInput, PlansInput, SortInput,
    StatementsInput, TablesInput, VacuumInput,
};
use super::filter::{FilterInput, build_search};
use super::semantics::{bounded_limit, mcp_error, mcp_structured};

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
    let limit = match bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE) {
        Ok(limit) => limit,
        Err(error) => return error,
    };
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
    let (rows, has_more) = match prepared.compute_relation_rows(limit, &|| false) {
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
    let (rows, has_more) = match plain_rows(logical_name, config, filters, sort, limit) {
        Ok(result) => result,
        Err(error) => return error,
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

/// Filter/sort/limit -> `SnapshotRequest` -> `compute_plain_rows`: the
/// physical read shared by every plain (non-relation-grouped) `PostgreSQL`
/// tool. Errors come back pre-rendered as a `CallToolResult` so every
/// caller's `match` collapses to one line, the same shape `mcp_error`
/// already produces for its other callers.
fn plain_rows(
    logical_name: &str,
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
) -> Result<(Vec<PlainRowOut>, bool), CallToolResult> {
    let limit = bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE)?;
    let search = build_search(logical_name, filters).map_err(mcp_error)?;
    let (segment_id, at) =
        current_segment(&config.data_root).map_err(|error| mcp_error(error.to_string()))?;
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
    let prepared = snapshot::prepare(&config.data_root, request, None)
        .map_err(|error| mcp_error(error.to_string()))?;
    let Prepared::Snapshot(prepared) = prepared else {
        return Err(mcp_error("snapshot request did not prepare a snapshot"));
    };
    let prepared = prepared
        .with_search(search)
        .map_err(|error| mcp_error(error.to_string()))?;
    prepared
        .compute_plain_rows(limit, &|| false)
        .map_err(|error| mcp_error(error.to_string()))
}

pub(crate) fn call_statements(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: StatementsInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_ratio(
        "pg_stat_statements",
        config,
        &input.filters,
        input.sort,
        input.limit,
    )
}

pub(crate) fn call_plans(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
    let input: PlansInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_ratio(
        "pg_store_plans",
        config,
        &input.filters,
        input.sort,
        input.limit,
    )
}

/// Shared body for `call_statements`/`call_plans`: the same plain read as
/// `call_plain`, plus `derived_*` ratio fields computed from each row's
/// own already-rated fields.
fn call_ratio(
    logical_name: &str,
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
) -> CallToolResult {
    let (rows, has_more) = match plain_rows(logical_name, config, filters, sort, limit) {
        Ok(result) => result,
        Err(error) => return error,
    };
    let row_count = rows.len();
    let rows: Vec<Value> = rows.into_iter().map(ratio_row_to_json).collect();
    let summary = format!(
        "{row_count} {logical_name} row{}{}",
        if row_count == 1 { "" } else { "s" },
        if has_more { ", more available" } else { "" },
    );
    mcp_structured(json!({ "rows": rows, "has_more": has_more }), summary)
}

/// `plain_row_to_json`, plus the `derived_*` ratio fields computed from the
/// same row's fields.
fn ratio_row_to_json(row: PlainRowOut) -> Value {
    let derived = derived_ratio_fields(&row.fields);
    let mut value = plain_row_to_json(row);
    if let Value::Object(object) = &mut value {
        object.extend(derived);
    }
    value
}

/// Reads one JS-safe-rendered numeric field back out: `row_record` renders
/// `i64`/`u64`/`Ts` as decimal strings (outside JSON's safe-integer range)
/// and every rate/`f64` gauge as a JSON number, so both forms need parsing
/// here.
fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// Reads one already-rendered field, trying each candidate name in turn
/// and stopping at the first the row's own physical layout carries — a
/// present-but-null value (no predecessor snapshot, or the rate could not
/// be computed) is authoritative and does not fall through to the next
/// candidate. This is how a legacy `pg_stat_statements` extension version
/// (`total_time` instead of `total_exec_time`) or a `pg_store_plans` row
/// (whose own "calls" field is the raw counter, not a rate — see
/// `calls_per_second`) resolves to the right column without the caller
/// naming a section.
fn ratio_field(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| fields.get(*name))
        .and_then(value_as_f64)
}

/// Sums several already-rendered fields; any field the row's layout does
/// not carry nulls the whole sum, matching `derived_page_order`'s
/// `counters`/`neutral_nulls = false` convention for the same tokens.
fn ratio_sum(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    let mut total = 0.0;
    for name in names {
        total += ratio_field(fields, &[name])?;
    }
    Some(total)
}

/// `numerator / denominator`, `null` on a missing operand or a
/// non-finite result (division by zero included) — the same "missing
/// metric stays missing" treatment as every other derived field.
fn ratio_value(numerator: Option<f64>, denominator: Option<f64>) -> Value {
    match numerator.zip(denominator).map(|(n, d)| n / d) {
        Some(value) if value.is_finite() => json!(value),
        _ => Value::Null,
    }
}

/// The seven `derived_*` per-row ratio fields for `pg_stat_statements`/
/// `pg_store_plans`. `derived_hit_fraction` and `derived_plan_time_fraction`
/// carry `derived_page_order`'s 0.0-1.0 fraction scale (not a percentage) —
/// named `_fraction`, not `_pct`, so they cannot be mistaken for the
/// 0-100 `_pct` fields `api/snapshot/relation.rs` ships (`buffer_hit_pct`
/// and friends). `derived_hit_fraction` counts shared-buffer traffic only
/// (`shared_blks_hit`/`shared_blks_read`), not local (temp-table) blocks,
/// unlike `derived_blocks_per_call` which sums both. Every formula reads
/// `row_record`'s already-rated fields directly — cumulative-column rates
/// on both sides of a ratio, so the shared elapsed interval cancels out and
/// the result is a plain per-call or per-total-block figure, not a
/// per-second one. `calls_per_second` is `pg_store_plans`'s alias for its
/// own `calls` column (`section_plans`, `snapshot.rs`): unlike every other
/// cumulative column, a plan row's own `calls` field renders as the raw
/// counter, not a rate, so the ratio math must read the alias instead.
/// `derived_wal_per_call` is null on every `pg_store_plans` row (no
/// `wal_bytes` column in any of its physical layouts) and on legacy
/// `pg_stat_statements` rows (extension 1.5-1.7 predates WAL tracking).
/// `derived_plan_time_fraction` is null wherever `total_plan_time` is
/// absent: legacy `pg_stat_statements` rows, and `pg_store_plans` rows from
/// the ossc/Datasentinel physical layouts (only the vadv layout carries
/// planning time) — not simply "statements only".
fn derived_ratio_fields(fields: &BTreeMap<String, Value>) -> Map<String, Value> {
    let calls = ratio_field(fields, &["calls_per_second", "calls"]);
    let execution = ratio_field(fields, &["total_exec_time", "total_time"]);
    let hit = ratio_field(fields, &["shared_blks_hit"]);
    let read = ratio_field(fields, &["shared_blks_read"]);
    let planning = ratio_field(fields, &["total_plan_time"]);
    let blocks = ratio_sum(
        fields,
        &[
            "shared_blks_hit",
            "shared_blks_read",
            "local_blks_hit",
            "local_blks_read",
        ],
    );

    let mut derived = Map::new();
    derived.insert(
        "derived_mean_exec_ms_per_call".to_owned(),
        ratio_value(execution, calls),
    );
    derived.insert(
        "derived_rows_per_call".to_owned(),
        ratio_value(ratio_field(fields, &["rows"]), calls),
    );
    derived.insert(
        "derived_blocks_per_call".to_owned(),
        ratio_value(blocks, calls),
    );
    derived.insert(
        "derived_hit_fraction".to_owned(),
        ratio_value(hit, hit.zip(read).map(|(hit, read)| hit + read)),
    );
    derived.insert(
        "derived_wal_per_call".to_owned(),
        ratio_value(ratio_field(fields, &["wal_bytes"]), calls),
    );
    derived.insert(
        "derived_plan_time_fraction".to_owned(),
        ratio_value(
            planning,
            planning
                .zip(execution)
                .map(|(planning, execution)| planning + execution),
        ),
    );
    derived.insert(
        "derived_cv".to_owned(),
        ratio_value(
            ratio_field(fields, &["stddev_exec_time", "stddev_time"]),
            ratio_field(fields, &["mean_exec_time", "mean_time"]),
        ),
    );
    derived
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
