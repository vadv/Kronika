//! Direct `PostgreSQL` MCP surfaces over the typed web API readers.

mod locks;
mod vacuum;

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use super::State;
use crate::api::{self, ApiError, ValueLimits, ValueStopReason};
use crate::route::{
    ActivityVisibility, Order, PostgresqlSurface, PostgresqlSurfaceRequest, RelationGroup, Route,
    SnapshotRequest, Window,
};

const HOUR_US: i64 = 3_600_000_000;
const MAX_ROWS: usize = 500;
const MAX_FIELDS: usize = 32;
const MAX_SECTIONS: usize = 16;
const MAX_SEGMENTS: usize = 64;
const RETAINED_RECORDS: usize = MAX_ROWS + MAX_SECTIONS + MAX_SEGMENTS + 8;

const OVERVIEW_SECTIONS: &[&str] = &[
    "pg_settings",
    "pg_stat_database",
    "pg_stat_wal",
    "pg_stat_checkpointer",
    "pg_stat_bgwriter",
    "pg_stat_archiver",
    "pg_stat_io",
    "pg_wal_storage",
    "pg_prepared_xacts",
];

#[derive(Debug)]
pub(super) struct PostgresqlPayload {
    pub(super) anchor: Value,
    pub(super) data: Value,
    pub(super) page: Value,
    pub(super) warnings: Vec<Value>,
    pub(super) summary: String,
}

#[derive(Debug)]
pub(super) struct PostgresqlFailure {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) parameter: Option<String>,
    pub(super) retryable: bool,
}

#[derive(Clone)]
struct Anchor {
    segment_id: i64,
    active_wal_position: Option<u64>,
    warnings: Vec<Value>,
}

#[derive(Clone, Copy)]
struct DirectSpec {
    section: &'static str,
    key: &'static str,
    search: bool,
    relation: bool,
    whole_set: bool,
}

pub(super) fn execute(
    state: &State,
    name: &str,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    match name {
        "kronika_get_postgresql_overview" => overview(state, args, cancelled),
        "kronika_find_postgresql_activity" => direct(
            state,
            args,
            DirectSpec {
                section: "pg_stat_activity",
                key: "activity",
                search: false,
                relation: false,
                whole_set: false,
            },
            budget,
            cancelled,
        ),
        "kronika_find_postgresql_locks" => locks::execute(state, args, cancelled),
        "kronika_find_postgresql_vacuum" => vacuum::execute(state, args, cancelled),
        "kronika_find_postgresql_statements" => direct(
            state,
            args,
            DirectSpec {
                section: "pg_stat_statements",
                key: "statements",
                search: true,
                relation: false,
                whole_set: false,
            },
            budget,
            cancelled,
        ),
        "kronika_find_postgresql_plans" => direct(
            state,
            args,
            DirectSpec {
                section: "pg_store_plans",
                key: "plans",
                search: true,
                relation: false,
                whole_set: false,
            },
            budget,
            cancelled,
        ),
        "kronika_find_postgresql_databases" => direct(
            state,
            args,
            DirectSpec {
                section: "pg_stat_database",
                key: "databases",
                search: false,
                relation: false,
                whole_set: false,
            },
            budget,
            cancelled,
        ),
        "kronika_find_postgresql_tables" => direct(
            state,
            args,
            DirectSpec {
                section: "pg_stat_user_tables",
                key: "tables",
                search: true,
                relation: true,
                whole_set: false,
            },
            budget,
            cancelled,
        ),
        "kronika_find_postgresql_indexes" => direct(
            state,
            args,
            DirectSpec {
                section: "pg_stat_user_indexes",
                key: "indexes",
                search: true,
                relation: true,
                whole_set: false,
            },
            budget,
            cancelled,
        ),
        _ => Err(failure(
            "unsupported_tool",
            format!("unsupported PostgreSQL tool {name}"),
            Some("name"),
        )),
    }
}

fn direct(
    state: &State,
    args: &Map<String, Value>,
    spec: DirectSpec,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    let at = timestamp(args, "at_us")?;
    if !spec.search && args.contains_key("find") {
        return Err(input(
            "find",
            "find is not supported by the shared Rust field registry for this surface",
        ));
    }
    if spec.whole_set && args.contains_key("cursor") {
        return Err(input(
            "cursor",
            "lock graph reads do not accept a partial-set cursor",
        ));
    }
    let group = if spec.relation {
        Some(group(args)?)
    } else {
        None
    };
    let surface = surface(args, &spec)?;
    let anchor = resolve_anchor(state, at, &[spec.section], cancelled)?;
    let fields = fields(args, &[])?;
    let page_size = if spec.whole_set {
        MAX_ROWS
    } else {
        page_size(args)?
    };
    let direction = direction(args)?;
    let order = args
        .get("order")
        .map(|value| string(value, "order").map(str::to_owned))
        .transpose()?;
    if let Some(requested_order) = order.as_deref()
        && api::postgresql_order_tokens(surface, requested_order, group).is_none()
    {
        return Err(input(
            "order",
            format!(
                "order {requested_order:?} is not accepted for {}",
                spec.section
            ),
        ));
    }
    let request = SnapshotRequest {
        segment_id: anchor.segment_id,
        active_position: anchor.active_wal_position,
        at,
        sections: vec![spec.section.to_owned()],
        fields,
        by: Vec::new(),
        direction,
        group,
        postgresql: Some(PostgresqlSurfaceRequest { surface, order }),
        page_size: Some(page_size),
        cursor: args
            .get("cursor")
            .map(|value| string(value, "cursor").map(str::to_owned))
            .transpose()?,
        search: if spec.search {
            args.get("find")
                .map(|value| string(value, "find").map(str::to_owned))
                .transpose()?
        } else {
            None
        },
        first_match: false,
        text: None,
        filters: Vec::new(),
        activity_visibility: (spec.section == "pg_stat_activity")
            .then(|| activity_visibility(args))
            .transpose()?,
        type_id: None,
        row_ordinal: None,
    };
    fit_direct_page(state, at, &anchor, spec, request, budget, cancelled)
}

fn fit_direct_page(
    state: &State,
    at: i64,
    anchor: &Anchor,
    spec: DirectSpec,
    mut request: SnapshotRequest,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    let page_size = request.page_size.unwrap_or(1);
    let first = direct_page(state, at, anchor, spec, request.clone(), cancelled);
    match first {
        Ok(payload) if payload_len(&payload) <= budget => return Ok(payload),
        Err(error) if error.code != "result_bound_exceeded" => return Err(error),
        Ok(_) | Err(_) if page_size == 1 => return Err(first_row_too_large()),
        Ok(_) | Err(_) => {}
    }

    let mut smallest = 1_usize;
    let mut largest = page_size - 1;
    let mut fitted = None;
    while smallest <= largest {
        if cancelled() {
            return Err(failure(
                "cancelled",
                "the PostgreSQL read was cancelled",
                None,
            ));
        }
        let candidate = smallest + (largest - smallest) / 2;
        request.page_size = Some(candidate);
        match direct_page(state, at, anchor, spec, request.clone(), cancelled) {
            Ok(payload) if payload_len(&payload) <= budget => {
                fitted = Some(payload);
                smallest = candidate + 1;
            }
            Ok(_)
            | Err(PostgresqlFailure {
                code: "result_bound_exceeded",
                ..
            }) => largest = candidate.saturating_sub(1),
            Err(error) => return Err(error),
        }
    }
    fitted.ok_or_else(first_row_too_large)
}

fn direct_page(
    state: &State,
    at: i64,
    anchor: &Anchor,
    spec: DirectSpec,
    request: SnapshotRequest,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    let collected = collect(state, Route::Snapshot(Box::new(request)), cancelled)?;
    let mut response_anchor = anchor.clone();
    response_anchor.active_wal_position = snapshot_active_position(&collected.records)?;
    let page = page(&collected.records, collected.stop_reason);
    if spec.whole_set && page.get("truncated").and_then(Value::as_bool) == Some(true) {
        return Err(failure(
            "whole_set_bound_exceeded",
            "the lock set exceeds the 500-row bound",
            Some("page_size"),
        ));
    }
    let selected = selected_at(&collected.records);
    let records = content_records(collected.records);
    let returned = record_rows(&records);
    let mut data = Map::new();
    data.insert(spec.key.to_owned(), Value::Array(records));
    if spec.key == "locks" {
        data.insert("components".to_owned(), Value::Array(Vec::new()));
    }
    data.insert("semantics".to_owned(), Value::Array(Vec::new()));
    Ok(PostgresqlPayload {
        anchor: anchor_value(at, selected, Some(&response_anchor)),
        data: Value::Object(data),
        page,
        warnings: anchor.warnings.clone(),
        summary: format!("Returned {returned} PostgreSQL {} rows.", spec.key),
    })
}

fn payload_len(payload: &PostgresqlPayload) -> usize {
    super::tools::structured_envelope_len(
        &payload.anchor,
        &payload.data,
        &payload.page,
        &payload.warnings,
    )
}

fn first_row_too_large() -> PostgresqlFailure {
    failure(
        "result_too_large",
        "the fixed PostgreSQL metadata and first selected row exceed data_budget_bytes",
        Some("data_budget_bytes"),
    )
}

fn surface(
    args: &Map<String, Value>,
    spec: &DirectSpec,
) -> Result<PostgresqlSurface, PostgresqlFailure> {
    let requested = args
        .get("lens")
        .map(|value| string(value, "lens"))
        .transpose()?;
    PostgresqlSurface::parse(spec.section, requested)
        .ok_or_else(|| input("lens", PostgresqlSurface::lens_error(spec.section)))
}

fn activity_visibility(args: &Map<String, Value>) -> Result<ActivityVisibility, PostgresqlFailure> {
    Ok(ActivityVisibility {
        include_idle: boolean(args, "include_idle", false)?,
        include_system: boolean(args, "include_system", false)?,
    })
}

fn boolean(
    args: &Map<String, Value>,
    name: &'static str,
    default: bool,
) -> Result<bool, PostgresqlFailure> {
    args.get(name).map_or(Ok(default), |value| {
        value
            .as_bool()
            .ok_or_else(|| input(name, format!("{name} must be a boolean")))
    })
}

fn overview(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    let at = timestamp(args, "at_us")?;
    let fields = fields(args, &[])?;
    let mut anchor = resolve_anchor(state, at, OVERVIEW_SECTIONS, cancelled)?;
    let request = SnapshotRequest {
        segment_id: anchor.segment_id,
        active_position: anchor.active_wal_position,
        at,
        sections: OVERVIEW_SECTIONS
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        fields,
        by: Vec::new(),
        direction: Order::Desc,
        group: None,
        postgresql: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        activity_visibility: None,
        type_id: None,
        row_ordinal: None,
    };
    let collected = collect(state, Route::Snapshot(Box::new(request)), cancelled)?;
    anchor.active_wal_position = snapshot_active_position(&collected.records)?;
    let selected = selected_at(&collected.records);
    let layouts = collected
        .records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some("layout"))
        .cloned()
        .collect::<Vec<_>>();
    let records = content_records(collected.records);
    let returned = record_rows(&records);
    Ok(PostgresqlPayload {
        anchor: anchor_value(at, selected, Some(&anchor)),
        data: json!({
            "overview": {"records": records},
            "layouts": layouts,
            "health": {},
            "semantics": crate::mcp::semantics::health(),
        }),
        page: json!({"returned": returned, "truncated": false, "next_cursor": null, "stop_reason": collected.stop_reason.code()}),
        warnings: anchor.warnings,
        summary: format!("Returned {returned} PostgreSQL overview rows."),
    })
}

fn collect(
    state: &State,
    route: Route,
    cancelled: &impl Fn() -> bool,
) -> Result<api::ValueCollection, PostgresqlFailure> {
    let prepared =
        api::prepare_for_mcp(&state.data_root, state.sources, state.synthetic_demo, route)
            .map_err(|error| api_failure(&error))?;
    let collected = prepared
        .collect_values(
            ValueLimits {
                records: RETAINED_RECORDS,
                ndjson_bytes: super::STRUCTURED_CONTENT_BYTES,
            },
            cancelled,
        )
        .map_err(|error| api_failure(&error))?;
    match collected.stop_reason {
        ValueStopReason::Complete => Ok(collected),
        ValueStopReason::Cancelled => Err(failure(
            "cancelled",
            "the PostgreSQL read was cancelled",
            None,
        )),
        ValueStopReason::RecordLimit | ValueStopReason::ByteLimit => Err(failure(
            "result_bound_exceeded",
            "the typed PostgreSQL record stream exceeds the retained result bound",
            Some("page_size"),
        )),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "anchor selection keeps its bounded catalog validation and deterministic fallback together"
)]
fn resolve_anchor(
    state: &State,
    at: i64,
    sections: &[&str],
    cancelled: &impl Fn() -> bool,
) -> Result<Anchor, PostgresqlFailure> {
    if sections.len() > MAX_SECTIONS {
        return Err(failure(
            "section_bound_exceeded",
            "more than 16 logical sections were requested",
            Some("section"),
        ));
    }
    let hour_start = at.div_euclid(HOUR_US).saturating_mul(HOUR_US);
    let prepared = api::prepare_for_mcp(
        &state.data_root,
        state.sources,
        state.synthetic_demo,
        Route::Catalog(Window {
            from: Some(hour_start),
            to: Some(at),
        }),
    )
    .map_err(|error| api_failure(&error))?;
    let catalog = prepared
        .collect_values(
            ValueLimits {
                records: MAX_SEGMENTS + 16,
                ndjson_bytes: super::STRUCTURED_CONTENT_BYTES,
            },
            cancelled,
        )
        .map_err(|error| api_failure(&error))?;
    if catalog.stop_reason != ValueStopReason::Complete {
        return Err(failure(
            "segment_bound_exceeded",
            "the selected hour exceeds the 64-segment catalog bound",
            None,
        ));
    }
    let wanted = sections.iter().copied().collect::<HashSet<_>>();
    let mut any = Vec::new();
    let mut matching = Vec::new();
    let mut warnings = Vec::new();
    for record in catalog.records {
        match record.get("record").and_then(Value::as_str) {
            Some("finished_segment" | "active_segment") => {
                let Some(id) = record.get("id").and_then(decimal_i64) else {
                    continue;
                };
                let wal = record
                    .pointer("/cursor/wal_position")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok());
                let matches_section = record
                    .get("sections")
                    .and_then(Value::as_array)
                    .is_some_and(|stored| {
                        stored.iter().any(|section| {
                            section
                                .get("logical_name")
                                .and_then(Value::as_str)
                                .is_some_and(|name| wanted.contains(name))
                        })
                    });
                any.push((id, wal));
                if matches_section {
                    matching.push((id, wal));
                }
            }
            Some("warning") => warnings.push(record),
            _ => {}
        }
    }
    if any.len() > MAX_SEGMENTS {
        return Err(failure(
            "segment_bound_exceeded",
            "the selected hour exceeds the 64-segment catalog bound",
            None,
        ));
    }
    let selected = matching
        .into_iter()
        .max_by_key(|(id, _)| *id)
        .or_else(|| any.into_iter().max_by_key(|(id, _)| *id))
        .ok_or_else(|| {
            failure(
                "no_data_at_time",
                "no segment exists at the requested time",
                Some("at_us"),
            )
        })?;
    Ok(Anchor {
        segment_id: selected.0,
        active_wal_position: selected.1,
        warnings,
    })
}

fn fields(args: &Map<String, Value>, defaults: &[&str]) -> Result<Vec<String>, PostgresqlFailure> {
    let Some(value) = args.get("fields") else {
        return Ok(defaults.iter().map(|value| (*value).to_owned()).collect());
    };
    let values = value
        .as_array()
        .ok_or_else(|| input("fields", "fields must be an array"))?;
    if values.len() > MAX_FIELDS {
        return Err(input("fields", "fields may contain at most 32 names"));
    }
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| {
            let field = string(value, "fields")?;
            if field.is_empty() || !seen.insert(field) {
                return Err(input("fields", "field names must be nonempty and unique"));
            }
            Ok(field.to_owned())
        })
        .collect()
}

fn timestamp(args: &Map<String, Value>, name: &'static str) -> Result<i64, PostgresqlFailure> {
    args.get(name).and_then(decimal_i64).ok_or_else(|| {
        input(
            name,
            format!("{name} must be a nonnegative decimal timestamp within i64"),
        )
    })
}

fn decimal_i64(value: &Value) -> Option<i64> {
    value
        .as_str()?
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
}

fn page_size(args: &Map<String, Value>) -> Result<usize, PostgresqlFailure> {
    let Some(value) = args.get("page_size") else {
        return Ok(100);
    };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| input("page_size", "page_size must be an integer"))?;
    if !(1..=MAX_ROWS).contains(&value) {
        return Err(input("page_size", "page_size must be between 1 and 500"));
    }
    Ok(value)
}

fn direction(args: &Map<String, Value>) -> Result<Order, PostgresqlFailure> {
    match args
        .get("direction")
        .map(|value| string(value, "direction"))
        .transpose()?
    {
        None | Some("desc") => Ok(Order::Desc),
        Some("asc") => Ok(Order::Asc),
        Some(_) => Err(input("direction", "direction must be asc or desc")),
    }
}

fn group(args: &Map<String, Value>) -> Result<RelationGroup, PostgresqlFailure> {
    match args
        .get("group")
        .map(|value| string(value, "group"))
        .transpose()?
    {
        None | Some("object") => Ok(RelationGroup::Object),
        Some("database") => Ok(RelationGroup::Database),
        Some("schema") => Ok(RelationGroup::Schema),
        Some("tablespace") => Ok(RelationGroup::Tablespace),
        Some(_) => Err(input(
            "group",
            "group must be object, database, schema, or tablespace",
        )),
    }
}

fn string<'a>(value: &'a Value, parameter: &'static str) -> Result<&'a str, PostgresqlFailure> {
    value
        .as_str()
        .ok_or_else(|| input(parameter, format!("{parameter} must be a string")))
}

fn page(records: &[Value], stop: ValueStopReason) -> Value {
    records.iter().find(|record| record.get("record").and_then(Value::as_str) == Some("snapshot_page")).map_or_else(
        || json!({"returned": record_rows(records), "truncated": false, "next_cursor": null, "stop_reason": stop.code()}),
        |record| json!({
            "returned": record.get("returned").and_then(decimal_usize).unwrap_or(0),
            "truncated": record.get("truncated").and_then(Value::as_bool).unwrap_or(false),
            "next_cursor": record.get("next_cursor").cloned().unwrap_or(Value::Null),
            "stop_reason": if record.get("has_more").and_then(Value::as_bool) == Some(true) { "page_limit" } else { stop.code() },
        }),
    )
}

fn decimal_usize(value: &Value) -> Option<usize> {
    value.as_str()?.parse().ok()
}

fn content_records(records: Vec<Value>) -> Vec<Value> {
    records
        .into_iter()
        .filter(|record| {
            !matches!(
                record.get("record").and_then(Value::as_str),
                Some("snapshot" | "snapshot_page" | "hour")
            )
        })
        .collect()
}

fn record_rows(records: &[Value]) -> usize {
    records
        .iter()
        .filter(|record| {
            matches!(
                record.get("record").and_then(Value::as_str),
                Some("row" | "relation")
            )
        })
        .count()
}

fn selected_at(records: &[Value]) -> Option<i64> {
    records
        .iter()
        .filter_map(|record| {
            record
                .get("timestamp")
                .or_else(|| record.get("sample_to"))
                .and_then(decimal_i64)
        })
        .max()
}

fn anchor_value(at: i64, selected: Option<i64>, anchor: Option<&Anchor>) -> Value {
    json!({
        "hour_start_us": at.div_euclid(HOUR_US).saturating_mul(HOUR_US).to_string(),
        "requested_at_us": at.to_string(),
        "selected_at_us": selected.map(|value| value.to_string()),
        "segment_id": anchor.map(|value| value.segment_id.to_string()),
        "active_wal_position": anchor.and_then(|value| value.active_wal_position).map(|value| value.to_string()),
    })
}

fn snapshot_active_position(records: &[Value]) -> Result<Option<u64>, PostgresqlFailure> {
    let value = records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("snapshot"))
        .and_then(|record| record.pointer("/segment/active_wal_position"))
        .ok_or_else(|| {
            failure(
                "snapshot_source_unavailable",
                "the snapshot has no active WAL position",
                None,
            )
        })?;
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .and_then(|position| position.parse().ok())
        .map(Some)
        .ok_or_else(|| {
            failure(
                "snapshot_source_unavailable",
                "the snapshot active WAL position is invalid",
                None,
            )
        })
}

fn api_failure(error: &ApiError) -> PostgresqlFailure {
    let retryable = error.source_changed_during_read();
    let parameter = error.parameter().map(|parameter| {
        if parameter == "search" {
            "find"
        } else {
            parameter
        }
    });
    if retryable {
        return PostgresqlFailure {
            code: "source_changed",
            message: "source changed during the read; retry the request".to_owned(),
            parameter: parameter.map(str::to_owned),
            retryable: true,
        };
    }
    PostgresqlFailure {
        code: error.code(),
        message: error.to_string(),
        parameter: parameter.map(str::to_owned),
        retryable: false,
    }
}

fn input(parameter: &'static str, message: impl Into<String>) -> PostgresqlFailure {
    failure("invalid_input", message, Some(parameter))
}

fn failure(
    code: &'static str,
    message: impl Into<String>,
    parameter: Option<&str>,
) -> PostgresqlFailure {
    PostgresqlFailure {
        code,
        message: message.into(),
        parameter: parameter.map(str::to_owned),
        retryable: false,
    }
}

#[cfg(test)]
#[path = "postgresql/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "postgresql/dispatch_tests.rs"]
mod dispatch_tests;
