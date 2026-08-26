//! MCP adapters over recorded `PostgreSQL` relation and snapshot rows selected
//! relative to the greatest-ID segment.
//! Statement and plan rows also expose ratios computed from fields in the same
//! rendered row.

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
use super::semantics::{DecimalI64, bounded_limit, mcp_error, mcp_structured};

pub(crate) fn call_tables(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
        cancelled,
    )
}

pub(crate) fn call_indexes(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
        cancelled,
    )
}

fn call(
    kind: RelationKind,
    config: &Config,
    group: RelationGroup,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let limit = match bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE) {
        Ok(limit) => limit,
        Err(error) => return error,
    };
    let search = match build_search(kind.logical_name(), filters) {
        Ok(search) => search,
        Err(error) => return mcp_error(error),
    };
    if let Some(sort) = &sort
        && !kind.sort_field_known(group, &sort.field)
    {
        return mcp_error(format!(
            "no such sort field for {}: {}",
            kind.logical_name(),
            sort.field
        ));
    }
    let (segment_id, at) = match current_segment(&config.data_root, kind.logical_name()) {
        Ok(Some(segment)) => segment,
        Ok(None) => return no_recorded_rows(kind.logical_name()),
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
        return mcp_error(
            "internal error: snapshot preparation returned an unexpected response type",
        );
    };
    let prepared = match prepared.with_search(search) {
        Ok(prepared) => prepared,
        Err(error) => return mcp_error(error.to_string()),
    };
    let (rows, has_more) = match prepared.compute_relation_rows(limit, &|| cancelled()) {
        Ok(result) => result,
        Err(error) => return mcp_error(error.to_string()),
    };

    let row_count = rows.len();
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|row| row_to_json(row, kind, group))
        .collect();
    let summary = format!(
        "Returned {row_count} recorded {} row{}{}.",
        kind.logical_name(),
        if row_count == 1 { "" } else { "s" },
        if has_more {
            "; result truncated to limit"
        } else {
            ""
        },
    );
    mcp_structured(
        json!({ "rows": rows, "has_more": has_more, "as_of": DecimalI64(at) }),
        summary,
    )
}

/// Returns the highest-ID segment carrying `logical_name` and its maximum
/// timestamp, or `None` when nothing recorded the section — resolved per
/// section because a sparsely recorded one is regularly absent from the
/// newest segment. `find_*` tools use this pair as their snapshot anchor;
/// they do not query live sources.
pub(crate) fn current_segment(
    root: &std::path::Path,
    logical_name: &str,
) -> Result<Option<(i64, i64)>, ApiError> {
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments(..)?;
    let segment = listing
        .segments
        .iter()
        .filter(|segment| {
            segment.sections().iter().any(|section| {
                kronika_registry::logical_section_name(section.type_id) == Some(logical_name)
            })
        })
        .max_by_key(|segment| segment.id());
    Ok(segment.map(|segment| (segment.id(), segment.max_ts())))
}

/// Public `derived_*` sort names mapped to the dotted internal tokens
/// `derived_page_order` (`api/snapshot.rs`) resolves. Sorting runs inside
/// the snapshot pipeline before this module attaches the derived fields to
/// rows, so the public names must translate before the request is built.
/// `derived_hit_fraction`/`derived_plan_time_fraction` rank by the internal
/// 0-100 renderings of the same ratios — identical order, different scale.
const DERIVED_SORT_TOKENS: [(&str, &str); 7] = [
    (
        "derived_mean_exec_ms_per_call",
        "derived.mean_exec_ms_per_call",
    ),
    ("derived_rows_per_call", "derived.rows_per_call"),
    ("derived_blocks_per_call", "derived.blocks_per_call"),
    ("derived_hit_fraction", "derived.hit_pct"),
    ("derived_wal_per_call", "derived.wal_per_call"),
    ("derived_plan_time_fraction", "derived.plan_time_pct"),
    ("derived_cv", "derived.cv"),
];

/// Resolves a plain tool's public sort name to the token the snapshot
/// pipeline ranks by, or errors on a name the pipeline would silently
/// ignore — `page_order` (`api/snapshot.rs`) falls back to identity order
/// on an unknown token.
pub(super) fn plain_sort_token(logical_name: &str, field: &str) -> Result<String, String> {
    if matches!(logical_name, "pg_stat_statements" | "pg_store_plans")
        && let Some((_, internal)) = DERIVED_SORT_TOKENS
            .iter()
            .find(|(public, _)| *public == field)
    {
        return Ok((*internal).to_owned());
    }
    let known = kronika_registry::registry()
        .iter()
        .filter(|contract| {
            kronika_registry::logical_section_name(contract.type_id.get()) == Some(logical_name)
        })
        .any(|contract| contract.column(field).is_some());
    if known {
        Ok(field.to_owned())
    } else {
        Err(format!("no such sort field for {logical_name}: {field}"))
    }
}

/// The empty result a `find_*` tool returns when [`current_segment`] finds
/// no segment carrying the section: an answer ("nothing recorded"), not an
/// error — on a healthy host this is `pg_stat_progress_vacuum`'s normal
/// state whenever no vacuum has run.
pub(super) fn no_recorded_rows(logical_name: &str) -> CallToolResult {
    mcp_structured(
        json!({ "rows": [], "has_more": false, "as_of": Value::Null }),
        format!("{logical_name} has no recorded rows in any segment"),
    )
}

/// Flattens metrics and group identity; identity fields win name collisions.
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

pub(crate) fn call_activity(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
        cancelled,
    )
}

pub(crate) fn call_locks(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: LocksInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    call_plain(
        "pg_locks",
        config,
        &input.filters,
        input.sort,
        input.limit,
        cancelled,
    )
}

pub(crate) fn call_vacuum(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
        cancelled,
    )
}

pub(crate) fn call_databases(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
        cancelled,
    )
}

fn call_plain(
    logical_name: &str,
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let (rows, has_more, at) =
        match plain_rows(logical_name, config, filters, sort, limit, cancelled) {
            Ok(Some(result)) => result,
            Ok(None) => return no_recorded_rows(logical_name),
            Err(error) => return error,
        };
    let row_count = rows.len();
    let rows: Vec<Value> = rows.into_iter().map(plain_row_to_json).collect();
    let summary = format!(
        "Returned {row_count} recorded {logical_name} row{}{}.",
        if row_count == 1 { "" } else { "s" },
        if has_more {
            "; result truncated to limit"
        } else {
            ""
        },
    );
    mcp_structured(
        json!({ "rows": rows, "has_more": has_more, "as_of": DecimalI64(at) }),
        summary,
    )
}

pub(super) fn plain_rows(
    logical_name: &str,
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<(Vec<PlainRowOut>, bool, i64)>, CallToolResult> {
    let limit = bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE)?;
    let search = build_search(logical_name, filters).map_err(mcp_error)?;
    let by = match &sort {
        Some(sort) => vec![plain_sort_token(logical_name, &sort.field).map_err(mcp_error)?],
        None => Vec::new(),
    };
    let Some((segment_id, at)) = current_segment(&config.data_root, logical_name)
        .map_err(|error| mcp_error(error.to_string()))?
    else {
        return Ok(None);
    };
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
        return Err(mcp_error(
            "internal error: snapshot preparation returned an unexpected response type",
        ));
    };
    let prepared = prepared
        .with_search(search)
        .map_err(|error| mcp_error(error.to_string()))?;
    let (rows, has_more) = prepared
        .compute_plain_rows(limit, &|| cancelled())
        .map_err(|error| mcp_error(error.to_string()))?;
    Ok(Some((rows, has_more, at)))
}

pub(crate) fn call_statements(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
        cancelled,
    )
}

pub(crate) fn call_plans(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
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
        cancelled,
    )
}

fn call_ratio(
    logical_name: &str,
    config: &Config,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let (rows, has_more, at) =
        match plain_rows(logical_name, config, filters, sort, limit, cancelled) {
            Ok(Some(result)) => result,
            Ok(None) => return no_recorded_rows(logical_name),
            Err(error) => return error,
        };
    let row_count = rows.len();
    let rows: Vec<Value> = rows.into_iter().map(ratio_row_to_json).collect();
    let summary = format!(
        "Returned {row_count} recorded {logical_name} row{}{}.",
        if row_count == 1 { "" } else { "s" },
        if has_more {
            "; result truncated to limit"
        } else {
            ""
        },
    );
    mcp_structured(
        json!({ "rows": rows, "has_more": has_more, "as_of": DecimalI64(at) }),
        summary,
    )
}

fn ratio_row_to_json(row: PlainRowOut) -> Value {
    let derived = derived_ratio_fields(&row.fields);
    let mut value = plain_row_to_json(row);
    if let Value::Object(object) = &mut value {
        object.extend(derived);
    }
    value
}

/// Accepts `row_record` numeric output in decimal-string or JSON-number form.
fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// Returns the first candidate present in the physical layout. A present null
/// stops fallback; candidate order handles legacy field names.
fn ratio_field(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| fields.get(*name))
        .and_then(value_as_f64)
}

/// Sums rendered fields; a missing or null operand makes the result null.
fn ratio_sum(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    let mut total = 0.0;
    for name in names {
        total += ratio_field(fields, &[name])?;
    }
    Some(total)
}

/// Returns `numerator / denominator`, or null for missing operands, zero
/// denominator, or a non-finite result.
fn ratio_value(numerator: Option<f64>, denominator: Option<f64>) -> Value {
    match numerator.zip(denominator).map(|(n, d)| n / d) {
        Some(value) if value.is_finite() => json!(value),
        _ => Value::Null,
    }
}

/// Computes ratio fields from values rendered for one row.
///
/// `derived_hit_fraction` and `derived_plan_time_fraction` use a `0..1` scale.
/// Hit fraction uses shared hit/read only; blocks per call includes shared and
/// local hit/read. Ratios of cumulative rates cancel their shared interval.
/// Plan calls use `calls_per_second` because `calls` is rendered as the raw
/// counter. Missing operands yield null: WAL is absent from all plan layouts
/// and legacy statement layouts; planning time is absent from legacy statements
/// and OSSC/Datasentinel plans.
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

/// Flattens projected fields and appends decimal-string locator fields accepted
/// by `kronika_get_row_detail`.
pub(super) fn plain_row_to_json(row: PlainRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    object.insert("segment_id".to_owned(), json!(row.segment_id.to_string()));
    object.insert("type_id".to_owned(), json!(row.type_id.to_string()));
    object.insert("row_ordinal".to_owned(), json!(row.row_ordinal.to_string()));
    object.insert("at".to_owned(), json!(row.at.to_string()));
    Value::Object(object)
}
