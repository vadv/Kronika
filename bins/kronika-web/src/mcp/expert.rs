//! Bounded expert MCP surfaces over the typed web API readers.

use kronika_registry::{ColumnType, Semantics, contract, logical_section_name, registry};
use serde_json::{Map, Value, json};

use super::State;
use crate::api::{self, ApiError, ValueCollection, ValueLimits, ValueStopReason};
use crate::product_semantics::SemanticPolicy;
use crate::route::{
    DataRequest, Filter, HourRequest, Order, Route, RowsRequest, SegmentRequest, SeriesRequest,
    SnapshotRequest, Window,
};

const HOUR_US: i64 = 3_600_000_000;
const MAX_SEGMENTS: usize = 64;
const MAX_FIELDS: usize = 32;
const MAX_FILTERS: usize = 16;
const MAX_IDENTITIES: usize = 16;
const MAX_HISTORY_SAMPLES: usize = 10_000;
const MAX_PAGE_ROWS: usize = 500;
const DEFAULT_PAGE_ROWS: usize = 100;
const DEFAULT_HISTORY_SAMPLES: usize = 2_000;
const DEFAULT_DATA_BYTES: usize = 32 * 1_024;
const MAX_DATA_BYTES: usize = 96 * 1_024;

const EVENT_SECTIONS: &[&str] = &[
    "pg_log_errors",
    "pg_log_checkpoints",
    "pg_log_autovacuum",
    "pg_log_slow_queries",
    "pg_log_lock_waits",
    "pg_log_lifecycle",
    "pgbouncer_events",
];

const EVENT_LONG_TEXT_FIELDS: &[&str] = &[
    "pattern",
    "sample",
    "detail",
    "hint",
    "context",
    "statement",
    "message",
    "query_detail",
    "text",
];

#[derive(Debug)]
pub(super) struct ExpertPayload {
    pub(super) anchor: Value,
    pub(super) data: Value,
    pub(super) page: Value,
    pub(super) warnings: Vec<Value>,
    pub(super) summary: String,
}

#[derive(Debug)]
pub(super) struct ExpertFailure {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) parameter: Option<String>,
    pub(super) retryable: bool,
}

pub(super) fn execute(
    state: &State,
    name: &str,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    match name {
        "kronika_find_events" => events(state, args, cancelled),
        "kronika_get_metric_history" => history(state, args, cancelled),
        "kronika_get_snapshot" => snapshot(state, args, cancelled),
        "kronika_get_row_detail" => row_detail(state, args, cancelled),
        _ => Err(failure(
            "unsupported_tool",
            format!("unsupported expert tool {name}"),
            Some("name"),
            false,
        )),
    }
}

fn history(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    let from = decimal_i64(args, "from_us")?;
    let to = decimal_i64(args, "to_us")?;
    bounded_window(from, to)?;
    let section = string(args, "section")?;
    known_section(section)?;
    let fields = strings(args, "fields", MAX_FIELDS)?;
    if fields.is_empty() {
        return Err(failure(
            "missing_parameter",
            "metric history requires at least one field",
            Some("fields"),
            false,
        ));
    }
    let identities = identity_filters(args.get("identities"))?;
    let sample_limit = usize_arg(
        args,
        "sample_limit",
        DEFAULT_HISTORY_SAMPLES,
        1,
        MAX_HISTORY_SAMPLES,
    )?;
    if args.contains_key("cursor") {
        return Err(failure(
            "unsupported_cursor",
            "native history continuation is not available on this reader path",
            Some("cursor"),
            false,
        ));
    }
    let budget = data_budget(args)?;
    let catalog = catalog(
        state,
        Window {
            from: Some(from),
            to: Some(to),
        },
        cancelled,
    )?;
    ensure_segment_budget(&catalog.records)?;
    let mut series = Vec::new();
    let mut warnings = catalog_warnings(&catalog.records);
    let mut stop = ValueStopReason::Complete;
    let mut remaining = sample_limit;
    for filters in identities {
        if remaining == 0 || cancelled() {
            stop = if cancelled() {
                ValueStopReason::Cancelled
            } else {
                ValueStopReason::RecordLimit
            };
            break;
        }
        let route = Route::Hour(HourRequest {
            window: Window {
                from: Some(from),
                to: Some(to),
            },
            series: Some(SeriesRequest {
                section: section.to_owned(),
                fields: fields.clone(),
                filters: filters.clone(),
                type_id: None,
                group: None,
            }),
        });
        let collected = collect(
            state,
            route,
            ValueLimits {
                records: remaining.saturating_add(MAX_SEGMENTS * 3 + 1),
                ndjson_bytes: budget,
            },
            cancelled,
        )?;
        let rows = records_named(&collected.records, "row");
        remaining = remaining.saturating_sub(rows.len());
        series.push(json!({
            "identity": filters_value(&filters),
            "records": collected.records,
        }));
        if collected.stop_reason != ValueStopReason::Complete {
            stop = collected.stop_reason;
            break;
        }
    }
    let returned = sample_limit.saturating_sub(remaining);
    if stop == ValueStopReason::Cancelled {
        return Err(failure(
            "cancelled",
            "history read was cancelled",
            None,
            true,
        ));
    }
    if stop == ValueStopReason::ByteLimit && returned == 0 {
        return Err(first_row_too_large());
    }
    if stop != ValueStopReason::Complete {
        warnings.push(json!({
            "code": "continuation_unavailable",
            "message": "the shared native-history reader has no query-bound cursor yet"
        }));
    }
    Ok(ExpertPayload {
        anchor: anchor_for_window(from, to, &catalog.records),
        data: json!({
            "series": series,
            "semantics": [],
        }),
        page: page(
            returned,
            stop != ValueStopReason::Complete,
            None,
            stop.code(),
        ),
        warnings,
        summary: format!("Returned {returned} native-cadence history samples."),
    })
}

fn snapshot(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    let section = string(args, "section")?;
    snapshot_section(section)?;
    let at = decimal_i64(args, "at_us")?;
    let fields = selected_fields(args, section, None)?;
    let filters = object_filters(args.get("filters"))?;
    let page_size = usize_arg(args, "page_size", DEFAULT_PAGE_ROWS, 1, MAX_PAGE_ROWS)?;
    let catalog = catalog(
        state,
        Window {
            from: None,
            to: Some(at),
        },
        cancelled,
    )?;
    ensure_segment_budget(&catalog.records)?;
    let Some(segment) = latest_segment_with_section(&catalog.records, section, at) else {
        return Ok(empty_snapshot(
            at,
            section,
            catalog_warnings(&catalog.records),
        ));
    };
    let request = SnapshotRequest {
        segment_id: segment.id,
        at,
        sections: vec![section.to_owned()],
        fields,
        by: optional_string(args, "order").into_iter().collect(),
        direction: order(args)?,
        group: None,
        page_size: Some(page_size),
        cursor: optional_string(args, "cursor"),
        search: None,
        first_match: false,
        text: None,
        filters,
        type_id: None,
        row_ordinal: None,
    };
    let collected = collect(
        state,
        Route::Snapshot(Box::new(request)),
        ValueLimits {
            records: page_size.saturating_add(64),
            ndjson_bytes: data_budget(args)?,
        },
        cancelled,
    )?;
    snapshot_payload(at, segment, collected, catalog_warnings(&catalog.records))
}

fn row_detail(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    if args.contains_key("cursor") {
        return Err(failure(
            "unsupported_cursor",
            "row detail has no continuation without a text chunk",
            Some("cursor"),
            false,
        ));
    }
    if args.contains_key("text_field") {
        return Err(failure(
            "text_chunk_unsupported",
            "the shared reader cannot byte-chunk a dictionary value without resolving it in full",
            Some("text_field"),
            false,
        ));
    }
    let segment_id = decimal_i64(args, "segment_id")?;
    let type_id = u32_arg(args, "type_id")?;
    let ordinal = decimal_u64(args, "row_ordinal")?;
    let timestamp = decimal_i64(args, "timestamp_us")?;
    let contract = contract(type_id).ok_or_else(|| {
        failure(
            "unknown_type_id",
            format!("unknown physical type id {type_id}"),
            Some("type_id"),
            false,
        )
    })?;
    let section = logical_section_name(type_id).ok_or_else(|| {
        failure(
            "unknown_type_id",
            format!("type id {type_id} has no logical section"),
            Some("type_id"),
            false,
        )
    })?;
    let fields = selected_fields(args, section, Some(type_id))?;
    for field in &fields {
        if contract
            .column(field)
            .is_some_and(|column| column.ty == ColumnType::StrId)
        {
            return Err(failure(
                "text_field_requires_chunk",
                format!("field {field:?} is a dictionary text value"),
                Some("fields"),
                false,
            ));
        }
    }
    let request = SnapshotRequest {
        segment_id,
        at: timestamp,
        sections: vec![section.to_owned()],
        fields,
        by: Vec::new(),
        direction: Order::Asc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: Some(type_id),
        row_ordinal: Some(ordinal),
    };
    let collected = collect(
        state,
        Route::Snapshot(Box::new(request)),
        ValueLimits {
            records: 8,
            ndjson_bytes: data_budget(args)?,
        },
        cancelled,
    )?;
    if collected.stop_reason == ValueStopReason::Cancelled {
        return Err(failure(
            "cancelled",
            "row detail read was cancelled",
            None,
            true,
        ));
    }
    if collected.stop_reason == ValueStopReason::ByteLimit {
        return Err(first_row_too_large());
    }
    let row = records_named(&collected.records, "row")
        .into_iter()
        .next()
        .ok_or_else(|| {
            failure(
                "locator_mismatch",
                "the exact row locator no longer identifies a recorded row",
                None,
                false,
            )
        })?;
    let layout = first_layout(&collected.records).unwrap_or_else(empty_object);
    Ok(ExpertPayload {
        anchor: json!({
            "hour_start_us": Value::Null,
            "requested_at_us": timestamp.to_string(),
            "selected_at_us": timestamp.to_string(),
            "segment_id": segment_id.to_string(),
            "active_wal_position": Value::Null,
        }),
        data: json!({
            "row": row,
            "text_chunk": {},
            "semantics": [layout],
        }),
        page: page(1, false, None, "complete"),
        warnings: Vec::new(),
        summary: "Returned one exact projected recorded row.".to_owned(),
    })
}

fn events(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    if args.contains_key("find") {
        return Err(failure(
            "unsupported_find",
            "Events has no shared Rust public field registry; use sources and fields",
            Some("find"),
            false,
        ));
    }
    if args.contains_key("cursor") {
        return Err(failure(
            "unsupported_cursor",
            "cross-segment Event continuation is not available on the shared rows path",
            Some("cursor"),
            false,
        ));
    }
    let from = decimal_i64(args, "from_us")?;
    let to = decimal_i64(args, "to_us")?;
    bounded_window(from, to)?;
    let requested_sources = strings(args, "sources", EVENT_SECTIONS.len())?;
    let sources: Vec<&str> = if requested_sources.is_empty() {
        EVENT_SECTIONS.to_vec()
    } else {
        requested_sources
            .iter()
            .map(String::as_str)
            .map(|source| {
                EVENT_SECTIONS
                    .contains(&source)
                    .then_some(source)
                    .ok_or_else(|| {
                        failure(
                            "unsupported_source",
                            format!("unsupported Event source {source:?}"),
                            Some("sources"),
                            false,
                        )
                    })
            })
            .collect::<Result<_, _>>()?
    };
    let requested_fields = strings(args, "fields", MAX_FIELDS)?;
    if requested_fields
        .iter()
        .any(|field| EVENT_LONG_TEXT_FIELDS.contains(&field.as_str()))
    {
        return Err(failure(
            "text_field_requires_detail",
            "Event rows do not expose unbounded message, query, or statement text",
            Some("fields"),
            false,
        ));
    }
    let page_size = usize_arg(args, "page_size", DEFAULT_PAGE_ROWS, 1, MAX_PAGE_ROWS)?;
    let catalog = catalog(
        state,
        Window {
            from: Some(from),
            to: Some(to),
        },
        cancelled,
    )?;
    ensure_segment_budget(&catalog.records)?;
    let mut events = Vec::new();
    let mut stop = ValueStopReason::Complete;
    'segments: for segment in catalog_segments(&catalog.records) {
        for source in &sources {
            if events.len() >= page_size {
                stop = ValueStopReason::RecordLimit;
                break 'segments;
            }
            if !segment.sections.iter().any(|name| name == source) {
                continue;
            }
            let fields = event_fields(source, &requested_fields)?;
            let mut projected = vec!["ts".to_owned()];
            projected.extend(fields.iter().cloned());
            projected.sort();
            projected.dedup();
            let route = Route::Rows(RowsRequest {
                data: DataRequest {
                    segment: SegmentRequest {
                        segment_id: segment.id,
                        section: (*source).to_owned(),
                    },
                    fields: projected.clone(),
                    filters: Vec::new(),
                    type_id: None,
                    after: None,
                },
                order: order(args)?,
                page_size: page_size.saturating_sub(events.len()),
                cursor: None,
            });
            let rows = collect(
                state,
                route,
                ValueLimits {
                    records: page_size.saturating_sub(events.len()).saturating_add(8),
                    ndjson_bytes: data_budget(args)?,
                },
                cancelled,
            )?;
            let layout = first_layout(&rows.records);
            for row in records_named(&rows.records, "row") {
                let Some(timestamp) = row_value(&row, &layout, "ts").and_then(Value::as_str) else {
                    continue;
                };
                let Ok(timestamp_number) = timestamp.parse::<i64>() else {
                    continue;
                };
                if timestamp_number < from || timestamp_number > to {
                    continue;
                }
                let type_id = row.get("type_id").cloned().unwrap_or(Value::Null);
                let tier = event_tier(source, &row, &layout)?;
                events.push(json!({
                    "section": source,
                    "tier": tier,
                    "semantic_id": format!("event.{source}.tier"),
                    "segment_id": segment.id.to_string(),
                    "type_id": type_id,
                    "row_ordinal": row.get("ordinal").cloned().unwrap_or(Value::Null),
                    "timestamp_us": timestamp,
                    "fields": row_fields(&row, &layout, &fields),
                }));
                if events.len() >= page_size {
                    stop = ValueStopReason::RecordLimit;
                    break 'segments;
                }
            }
            if rows.stop_reason != ValueStopReason::Complete {
                stop = rows.stop_reason;
                break 'segments;
            }
        }
    }
    if stop == ValueStopReason::Cancelled {
        return Err(failure("cancelled", "Event read was cancelled", None, true));
    }
    let semantics = event_semantics()?;
    let returned = events.len();
    let mut warnings = catalog_warnings(&catalog.records);
    if stop != ValueStopReason::Complete {
        warnings.push(json!({
            "code": "continuation_unavailable",
            "message": "the shared Event row reader has no cross-segment query cursor yet"
        }));
    }
    Ok(ExpertPayload {
        anchor: anchor_for_window(from, to, &catalog.records),
        data: json!({
            "groups": [],
            "events": events,
            "semantics": semantics,
        }),
        page: page(
            returned,
            stop != ValueStopReason::Complete,
            None,
            stop.code(),
        ),
        warnings,
        summary: format!("Returned {returned} recorded Event rows."),
    })
}

#[derive(Clone)]
struct SegmentInfo {
    id: i64,
    min_ts: i64,
    max_ts: i64,
    active_position: Option<u64>,
    sections: Vec<String>,
}

fn collect(
    state: &State,
    route: Route,
    limits: ValueLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<ValueCollection, ExpertFailure> {
    api::prepare_for_mcp(&state.data_root, state.sources, state.synthetic_demo, route)
        .and_then(|prepared| prepared.collect_values(limits, cancelled))
        .map_err(api_failure)
}

fn catalog(
    state: &State,
    window: Window,
    cancelled: &impl Fn() -> bool,
) -> Result<ValueCollection, ExpertFailure> {
    let collected = collect(
        state,
        Route::Catalog(window),
        ValueLimits {
            records: MAX_SEGMENTS.saturating_add(4),
            ndjson_bytes: MAX_DATA_BYTES,
        },
        cancelled,
    )?;
    match collected.stop_reason {
        ValueStopReason::Complete => Ok(collected),
        ValueStopReason::Cancelled => Err(failure(
            "cancelled",
            "catalog read was cancelled",
            None,
            true,
        )),
        ValueStopReason::RecordLimit | ValueStopReason::ByteLimit => Err(failure(
            "scan_budget_exceeded",
            "catalog exceeds the 64-segment MCP scan bound",
            None,
            false,
        )),
    }
}

fn api_failure(error: ApiError) -> ExpertFailure {
    let retryable = error.source_changed_during_read();
    let code = if retryable {
        "source_changed"
    } else {
        error.code()
    };
    let parameter = error.parameter().map(ToOwned::to_owned);
    let message = if matches!(error, ApiError::Unreadable(_)) {
        "recorded data could not be read".to_owned()
    } else {
        error.to_string()
    };
    failure(code, message, parameter.as_deref(), retryable)
}

fn failure(
    code: &'static str,
    message: impl Into<String>,
    parameter: Option<&str>,
    retryable: bool,
) -> ExpertFailure {
    ExpertFailure {
        code,
        message: message.into(),
        parameter: parameter.map(ToOwned::to_owned),
        retryable,
    }
}

fn first_row_too_large() -> ExpertFailure {
    failure(
        "result_too_large",
        "the first selected row exceeds the structured-data budget",
        Some("data_budget_bytes"),
        false,
    )
}

fn string<'a>(args: &'a Map<String, Value>, name: &str) -> Result<&'a str, ExpertFailure> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            failure(
                "invalid_parameter",
                format!("{name} must be a non-empty string"),
                Some(name),
                false,
            )
        })
}

fn optional_string(args: &Map<String, Value>, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn decimal_i64(args: &Map<String, Value>, name: &str) -> Result<i64, ExpertFailure> {
    string(args, name)?.parse().map_err(|_error| {
        failure(
            "invalid_parameter",
            format!("{name} must be a decimal i64 string"),
            Some(name),
            false,
        )
    })
}

fn decimal_u64(args: &Map<String, Value>, name: &str) -> Result<u64, ExpertFailure> {
    string(args, name)?.parse().map_err(|_error| {
        failure(
            "invalid_parameter",
            format!("{name} must be a decimal u64 string"),
            Some(name),
            false,
        )
    })
}

fn u32_arg(args: &Map<String, Value>, name: &str) -> Result<u32, ExpertFailure> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            failure(
                "invalid_parameter",
                format!("{name} must be a u32"),
                Some(name),
                false,
            )
        })
}

fn usize_arg(
    args: &Map<String, Value>,
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, ExpertFailure> {
    let value = match args.get(name) {
        None => return Ok(default),
        Some(value) => value.as_u64().and_then(|value| usize::try_from(value).ok()),
    }
    .ok_or_else(|| {
        failure(
            "invalid_parameter",
            format!("{name} must be an integer"),
            Some(name),
            false,
        )
    })?;
    if !(minimum..=maximum).contains(&value) {
        return Err(failure(
            "invalid_parameter",
            format!("{name} must be between {minimum} and {maximum}"),
            Some(name),
            false,
        ));
    }
    Ok(value)
}

fn data_budget(args: &Map<String, Value>) -> Result<usize, ExpertFailure> {
    usize_arg(
        args,
        "data_budget_bytes",
        DEFAULT_DATA_BYTES,
        1_024,
        MAX_DATA_BYTES,
    )
}

fn strings(
    args: &Map<String, Value>,
    name: &str,
    maximum: usize,
) -> Result<Vec<String>, ExpertFailure> {
    let Some(value) = args.get(name) else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| {
        failure(
            "invalid_parameter",
            format!("{name} must be an array"),
            Some(name),
            false,
        )
    })?;
    if values.len() > maximum {
        return Err(failure(
            "invalid_parameter",
            format!("{name} accepts at most {maximum} values"),
            Some(name),
            false,
        ));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    failure(
                        "invalid_parameter",
                        format!("{name} values must be non-empty strings"),
                        Some(name),
                        false,
                    )
                })
        })
        .collect()
}

fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn object_filters(value: Option<&Value>) -> Result<Vec<Filter>, ExpertFailure> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let object = value.as_object().ok_or_else(|| {
        failure(
            "invalid_parameter",
            "filters must be an object",
            Some("filters"),
            false,
        )
    })?;
    if object.len() > MAX_FILTERS {
        return Err(failure(
            "invalid_parameter",
            format!("filters accepts at most {MAX_FILTERS} predicates"),
            Some("filters"),
            false,
        ));
    }
    object
        .iter()
        .map(|(column, value)| {
            value_text(value)
                .map(|value| Filter {
                    column: column.clone(),
                    value,
                })
                .ok_or_else(|| {
                    failure(
                        "invalid_parameter",
                        "filter values must be strings, integers, or booleans",
                        Some("filters"),
                        false,
                    )
                })
        })
        .collect()
}

fn identity_filters(value: Option<&Value>) -> Result<Vec<Vec<Filter>>, ExpertFailure> {
    let Some(value) = value else {
        return Ok(vec![Vec::new()]);
    };
    let identities = value.as_array().ok_or_else(|| {
        failure(
            "invalid_parameter",
            "identities must be an array",
            Some("identities"),
            false,
        )
    })?;
    if identities.len() > MAX_IDENTITIES {
        return Err(failure(
            "invalid_parameter",
            format!("identities accepts at most {MAX_IDENTITIES} entries"),
            Some("identities"),
            false,
        ));
    }
    if identities.is_empty() {
        return Ok(vec![Vec::new()]);
    }
    identities
        .iter()
        .map(|identity| object_filters(Some(identity)))
        .collect()
}

fn filters_value(filters: &[Filter]) -> Value {
    Value::Object(
        filters
            .iter()
            .map(|filter| (filter.column.clone(), Value::String(filter.value.clone())))
            .collect(),
    )
}

fn bounded_window(from: i64, to: i64) -> Result<(), ExpertFailure> {
    if to < from || to.saturating_sub(from) > HOUR_US {
        return Err(failure(
            "invalid_window",
            "the inclusive window must be ordered and no longer than one hour",
            Some("to_us"),
            false,
        ));
    }
    Ok(())
}

fn known_section(section: &str) -> Result<(), ExpertFailure> {
    if registry()
        .iter()
        .any(|item| logical_section_name(item.type_id.get()).is_some_and(|name| name == section))
    {
        Ok(())
    } else {
        Err(failure(
            "unsupported_section",
            format!("unsupported logical section {section:?}"),
            Some("section"),
            false,
        ))
    }
}

fn snapshot_section(section: &str) -> Result<(), ExpertFailure> {
    known_section(section)?;
    if registry().iter().any(|item| {
        logical_section_name(item.type_id.get()).is_some_and(|name| name == section)
            && item.semantics != Semantics::EventStream
    }) {
        Ok(())
    } else {
        Err(failure(
            "unsupported_section",
            "event streams use kronika_find_events",
            Some("section"),
            false,
        ))
    }
}

fn selected_fields(
    args: &Map<String, Value>,
    section: &str,
    type_id: Option<u32>,
) -> Result<Vec<String>, ExpertFailure> {
    let requested = strings(args, "fields", MAX_FIELDS)?;
    let layouts = registry().iter().filter(|item| {
        type_id.is_none_or(|wanted| item.type_id.get() == wanted)
            && logical_section_name(item.type_id.get()).is_some_and(|name| name == section)
    });
    let layouts = layouts.collect::<Vec<_>>();
    for field in &requested {
        if !layouts.iter().any(|layout| layout.column(field).is_some()) {
            return Err(failure(
                "no_such_column",
                format!("logical section {section:?} has no field {field:?}"),
                Some("fields"),
                false,
            ));
        }
    }
    if !requested.is_empty() {
        return Ok(requested);
    }
    let mut fields = Vec::new();
    for layout in layouts {
        for column in layout.columns {
            if column.ty != ColumnType::StrId && !fields.iter().any(|name| name == column.name) {
                fields.push(column.name.to_owned());
                if fields.len() == MAX_FIELDS {
                    return Ok(fields);
                }
            }
        }
    }
    Ok(fields)
}

fn event_fields(section: &str, requested: &[String]) -> Result<Vec<String>, ExpertFailure> {
    let layouts = registry()
        .iter()
        .filter(|item| logical_section_name(item.type_id.get()).is_some_and(|name| name == section))
        .collect::<Vec<_>>();
    if requested.is_empty() {
        return Ok(layouts
            .iter()
            .flat_map(|layout| layout.columns)
            .filter(|column| column.name != "ts" && column.ty != ColumnType::StrId)
            .map(|column| column.name.to_owned())
            .take(MAX_FIELDS)
            .collect());
    }
    for field in requested {
        if !registry()
            .iter()
            .any(|layout| layout.column(field).is_some())
        {
            return Err(failure(
                "no_such_column",
                format!("no recorded Event layout has field {field:?}"),
                Some("fields"),
                false,
            ));
        }
    }
    Ok(requested
        .iter()
        .filter(|field| layouts.iter().any(|layout| layout.column(field).is_some()))
        .cloned()
        .collect())
}

fn order(args: &Map<String, Value>) -> Result<Order, ExpertFailure> {
    match args.get("direction").and_then(Value::as_str) {
        None | Some("asc") => Ok(Order::Asc),
        Some("desc") => Ok(Order::Desc),
        Some(_) => Err(failure(
            "invalid_parameter",
            "direction must be asc or desc",
            Some("direction"),
            false,
        )),
    }
}

fn records_named(records: &[Value], name: &str) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some(name))
        .cloned()
        .collect()
}

fn first_layout(records: &[Value]) -> Option<Value> {
    records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("layout"))
        .and_then(|record| record.get("layout"))
        .cloned()
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn catalog_segments(records: &[Value]) -> Vec<SegmentInfo> {
    records
        .iter()
        .filter_map(|record| {
            matches!(
                record.get("record").and_then(Value::as_str),
                Some("finished_segment" | "active_segment")
            )
            .then_some(())?;
            let id = record.get("id")?.as_str()?.parse().ok()?;
            let min_ts = record.get("min_ts")?.as_str()?.parse().ok()?;
            let max_ts = record.get("max_ts")?.as_str()?.parse().ok()?;
            let active_position = record
                .get("cursor")
                .and_then(|cursor| cursor.get("wal_position"))
                .and_then(Value::as_str)
                .and_then(|position| position.parse().ok());
            let sections = record
                .get("sections")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|section| section.get("logical_name").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect();
            Some(SegmentInfo {
                id,
                min_ts,
                max_ts,
                active_position,
                sections,
            })
        })
        .collect()
}

fn ensure_segment_budget(records: &[Value]) -> Result<(), ExpertFailure> {
    if catalog_segments(records).len() > MAX_SEGMENTS {
        Err(failure(
            "scan_budget_exceeded",
            "request intersects more than 64 segments",
            None,
            false,
        ))
    } else {
        Ok(())
    }
}

fn catalog_warnings(records: &[Value]) -> Vec<Value> {
    records_named(records, "warning")
}

fn latest_segment_with_section(records: &[Value], section: &str, at: i64) -> Option<SegmentInfo> {
    catalog_segments(records)
        .into_iter()
        .filter(|segment| {
            segment.min_ts <= at && segment.sections.iter().any(|name| name == section)
        })
        .max_by_key(|segment| (segment.max_ts.min(at), segment.id))
}

fn anchor_for_window(from: i64, _to: i64, records: &[Value]) -> Value {
    let segments = catalog_segments(records);
    let active = segments
        .iter()
        .find(|segment| segment.active_position.is_some());
    json!({
        "hour_start_us": from.div_euclid(HOUR_US).saturating_mul(HOUR_US).to_string(),
        "requested_at_us": Value::Null,
        "selected_at_us": Value::Null,
        "segment_id": Value::Null,
        "active_wal_position": active.and_then(|segment| segment.active_position).map(|value| value.to_string()),
    })
}

fn page(returned: usize, truncated: bool, next_cursor: Option<String>, reason: &str) -> Value {
    json!({
        "returned": returned,
        "truncated": truncated,
        "next_cursor": next_cursor,
        "stop_reason": reason,
    })
}

fn empty_snapshot(at: i64, section: &str, mut warnings: Vec<Value>) -> ExpertPayload {
    warnings.push(json!({
        "code": "section_not_recorded",
        "section": section,
    }));
    ExpertPayload {
        anchor: json!({
            "hour_start_us": Value::Null,
            "requested_at_us": at.to_string(),
            "selected_at_us": Value::Null,
            "segment_id": Value::Null,
            "active_wal_position": Value::Null,
        }),
        data: json!({
            "rows": [],
            "layout": {},
            "semantics": [],
        }),
        page: page(0, false, None, "complete"),
        warnings,
        summary: "No recorded sample exists at or before the requested time.".to_owned(),
    }
}

fn snapshot_payload(
    at: i64,
    segment: SegmentInfo,
    collected: ValueCollection,
    warnings: Vec<Value>,
) -> Result<ExpertPayload, ExpertFailure> {
    match collected.stop_reason {
        ValueStopReason::Complete => {}
        ValueStopReason::Cancelled => {
            return Err(failure(
                "cancelled",
                "snapshot read was cancelled",
                None,
                true,
            ));
        }
        ValueStopReason::ByteLimit | ValueStopReason::RecordLimit => {
            return Err(first_row_too_large());
        }
    }
    let rows = records_named(&collected.records, "row");
    let layouts = records_named(&collected.records, "layout")
        .into_iter()
        .filter_map(|record| record.get("layout").cloned())
        .collect::<Vec<_>>();
    let trailer = collected
        .records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("snapshot_page"));
    let next = trailer
        .and_then(|record| record.get("next_cursor"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let has_more = trailer
        .and_then(|record| record.get("has_more"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let selected = trailer
        .and_then(|record| record.get("to"))
        .cloned()
        .unwrap_or(Value::Null);
    let returned = rows.len();
    Ok(ExpertPayload {
        anchor: json!({
            "hour_start_us": Value::Null,
            "requested_at_us": at.to_string(),
            "selected_at_us": selected,
            "segment_id": segment.id.to_string(),
            "active_wal_position": segment.active_position.map(|value| value.to_string()),
        }),
        data: json!({
            "rows": rows,
            "layout": { "layouts": layouts },
            "semantics": [],
        }),
        page: page(
            returned,
            has_more,
            next,
            if has_more { "page_limit" } else { "complete" },
        ),
        warnings,
        summary: format!(
            "Returned {returned} rows from the latest recorded sample at or before the requested time."
        ),
    })
}

fn layout_columns(layout: &Option<Value>) -> Vec<String> {
    layout
        .as_ref()
        .and_then(|layout| layout.get("columns"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|column| column.get("name").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn row_value<'a>(row: &'a Value, layout: &Option<Value>, field: &str) -> Option<&'a Value> {
    let index = layout_columns(layout)
        .iter()
        .position(|name| name == field)?;
    row.get("values")?.as_array()?.get(index)
}

fn row_fields(row: &Value, layout: &Option<Value>, fields: &[String]) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|field| {
                (
                    field.clone(),
                    row_value(row, layout, field)
                        .cloned()
                        .unwrap_or(Value::Null),
                )
            })
            .collect(),
    )
}

fn event_tier(section: &str, row: &Value, layout: &Option<Value>) -> Result<Value, ExpertFailure> {
    let id = format!("event.{section}.tier");
    let definition = crate::product_semantics::get(&id)
        .map_err(|error| failure("semantics_unreadable", error.to_string(), None, false))?
        .ok_or_else(|| {
            failure(
                "semantics_unreadable",
                format!("missing accepted Event tier for {section}"),
                None,
                false,
            )
        })?;
    let SemanticPolicy::EventTier {
        discriminator,
        tiers,
        fallback,
        ..
    } = &definition.policy
    else {
        return Err(failure(
            "semantics_unreadable",
            format!("invalid accepted Event tier for {section}"),
            None,
            false,
        ));
    };
    let selected = discriminator
        .as_deref()
        .and_then(|field| row_value(row, layout, field))
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| tiers.get(index))
        .unwrap_or(fallback);
    serde_json::to_value(selected)
        .map_err(|error| failure("semantics_unreadable", error.to_string(), None, false))
}

fn event_semantics() -> Result<Vec<Value>, ExpertFailure> {
    crate::product_semantics::all()
        .map_err(|error| failure("semantics_unreadable", error.to_string(), None, false))?
        .iter()
        .filter(|definition| {
            matches!(
                &definition.policy,
                SemanticPolicy::EventTier { .. } | SemanticPolicy::EventTierOrder { .. }
            )
        })
        .map(|definition| {
            serde_json::to_value(definition)
                .map_err(|error| failure("semantics_unreadable", error.to_string(), None, false))
        })
        .collect()
}
