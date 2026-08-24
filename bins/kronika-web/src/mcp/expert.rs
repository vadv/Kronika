//! Bounded expert MCP surfaces over the typed web API readers.

mod events;

#[cfg(test)]
mod tests;

use kronika_registry::{
    ColumnType, Semantics, TypeContract, contract, logical_section_name, registry,
};
use serde_json::{Map, Value, json};

use super::State;
use crate::api::{self, ApiError, ValueCollection, ValueLimits, ValueStopReason};
use crate::product_semantics::SemanticPolicy;
use crate::route::{Filter, HourRequest, Order, Route, SeriesRequest, SnapshotRequest, Window};

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

struct RowDetailQuery {
    timestamp: i64,
    request: api::RowDetailRequest,
}

pub(super) fn execute(
    state: &State,
    name: &str,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    match name {
        "kronika_find_events" => events::execute(state, args, cancelled),
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

#[expect(
    clippy::too_many_lines,
    reason = "the bounded history handler keeps validation, shared-reader execution, and response assembly together"
)]
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
    let source = catalog_segments(&catalog.records);
    let active_segment = active_segment(&source);
    let anchor = anchor_for_window(from, to, &catalog.records);
    let mut series = Vec::new();
    let mut warnings = catalog_warnings(&catalog.records);
    let bounded_warning = json!({
        "code": "continuation_unavailable",
        "message": "the shared native-history reader has no query-bound cursor yet"
    });
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
        series.push(json!({
            "identity": filters_value(&filters),
            "records": [],
        }));
        let mut bounded_warnings = warnings.clone();
        bounded_warnings.push(bounded_warning.clone());
        let fixed = history_envelope_len(
            &anchor,
            &series,
            sample_limit,
            ValueStopReason::RecordLimit,
            &bounded_warnings,
        );
        let _empty_series = series.pop();
        if fixed >= budget {
            stop = ValueStopReason::ByteLimit;
            break;
        }
        let route = history_route(
            from,
            to,
            active_segment,
            section,
            fields.clone(),
            filters.clone(),
        );
        let collected = collect(
            state,
            route,
            ValueLimits {
                records: remaining.saturating_add(MAX_SEGMENTS * 3 + 1),
                ndjson_bytes: budget.saturating_sub(fixed),
            },
            cancelled,
        )?;
        let rows = records_named(&collected.records, "row");
        if rows.is_empty() && collected.stop_reason != ValueStopReason::Complete {
            stop = collected.stop_reason;
            break;
        }
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
        return Err(history_prefix_too_large());
    }
    if stop != ValueStopReason::Complete {
        warnings.push(bounded_warning);
    }
    ensure_history_source_unchanged(
        state,
        Window {
            from: Some(from),
            to: Some(to),
        },
        &source,
        cancelled,
    )?;
    let payload = ExpertPayload {
        anchor,
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
    };
    if super::tools::structured_envelope_len(
        &payload.anchor,
        &payload.data,
        &payload.page,
        &payload.warnings,
    ) > budget
    {
        return Err(history_prefix_too_large());
    }
    Ok(payload)
}

fn history_route(
    from: i64,
    to: i64,
    active_segment: Option<(i64, u64)>,
    section: &str,
    fields: Vec<String>,
    filters: Vec<Filter>,
) -> Route {
    Route::Hour(HourRequest {
        window: Window {
            from: Some(from),
            to: Some(to),
        },
        active_segment,
        series: Some(SeriesRequest {
            section: section.to_owned(),
            fields,
            filters,
            type_id: None,
            group: None,
        }),
    })
}

fn history_envelope_len(
    anchor: &Value,
    series: &[Value],
    returned: usize,
    stop: ValueStopReason,
    warnings: &[Value],
) -> usize {
    let data = json!({
        "series": series,
        "semantics": [],
    });
    let page = page(
        returned,
        stop != ValueStopReason::Complete,
        None,
        stop.code(),
    );
    super::tools::structured_envelope_len(anchor, &data, &page, warnings)
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
    let request = SnapshotRequest {
        segment_id: 0,
        active_position: None,
        at,
        sections: vec![section.to_owned()],
        fields,
        by: optional_string(args, "order").into_iter().collect(),
        direction: order(args)?,
        group: None,
        postgresql: None,
        process: None,
        page_size: Some(page_size),
        cursor: optional_string(args, "cursor"),
        search: None,
        first_match: false,
        text: None,
        filters,
        activity_visibility: None,
        type_id: None,
        row_ordinal: None,
    };
    let prepared = match api::prepare_snapshot_for_mcp(&state.data_root, request, cancelled) {
        Ok(prepared) => prepared,
        Err(_error) if cancelled() => {
            return Err(failure(
                "cancelled",
                "snapshot read was cancelled",
                None,
                true,
            ));
        }
        Err(error) => return Err(api_failure(error)),
    };
    let warnings = prepared
        .warnings
        .iter()
        .map(api::catalog_warning_value)
        .collect::<Vec<_>>();
    let Some(source) = prepared.prepared else {
        return Ok(empty_snapshot(at, section, warnings));
    };
    let segment = SegmentInfo {
        id: prepared.segment_id.ok_or_else(|| {
            failure(
                "unreadable",
                "selected snapshot has no segment identity",
                None,
                false,
            )
        })?,
        min_ts: 0,
        max_ts: 0,
        active_position: prepared.active_position,
        sections: vec![section.to_owned()],
    };
    let collected = source
        .collect_values(
            ValueLimits {
                records: page_size.saturating_add(64),
                ndjson_bytes: data_budget(args)?,
            },
            cancelled,
        )
        .map_err(api_failure)?;
    snapshot_payload(at, segment, &collected, warnings)
}

fn row_detail(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<ExpertPayload, ExpertFailure> {
    let query = row_detail_query(args)?;
    if cancelled() {
        return Err(failure(
            "cancelled",
            "row detail read was cancelled",
            None,
            true,
        ));
    }
    let segment_id = query.request.segment_id;
    let timestamp = query.timestamp;
    let detail = api::read_row_detail(&state.data_root, &query.request).map_err(api_failure)?;
    if cancelled() {
        return Err(failure(
            "cancelled",
            "row detail read was cancelled",
            None,
            true,
        ));
    }
    let has_more = detail.next_cursor.is_some();
    let bounded = has_more || detail.source_truncated;
    let stop_reason = if has_more {
        "text_chunk_limit"
    } else if detail.source_truncated {
        "source_truncated"
    } else {
        "complete"
    };
    Ok(ExpertPayload {
        anchor: json!({
            "hour_start_us": Value::Null,
            "requested_at_us": timestamp.to_string(),
            "selected_at_us": timestamp.to_string(),
            "segment_id": segment_id.to_string(),
            "active_wal_position": detail.active_position.map(|value| value.to_string()),
        }),
        data: json!({
            "row": detail.row,
            "text_chunk": detail.text_chunk.unwrap_or_else(empty_object),
            "semantics": [detail.layout],
        }),
        page: page(1, bounded, detail.next_cursor, stop_reason),
        warnings: Vec::new(),
        summary: if has_more {
            "Returned one projected row and a bounded text chunk; another chunk is available."
                .to_owned()
        } else {
            "Returned one projected row and its requested text detail.".to_owned()
        },
    })
}

fn row_detail_query(args: &Map<String, Value>) -> Result<RowDetailQuery, ExpertFailure> {
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
    let fields = detail_fields(args, section, type_id, contract)?;
    let text_field = detail_text_field(args, type_id, contract)?;
    let byte_offset = optional_usize_arg(
        args,
        "byte_offset",
        0,
        usize::try_from(u32::MAX).unwrap_or(usize::MAX),
    )?
    .map(|value| u64::try_from(value).unwrap_or(u64::MAX));
    let byte_limit = usize_arg(args, "byte_limit", 16 * 1_024, 1, 32 * 1_024)?;
    let cursor = args
        .get("cursor")
        .map(|_value| string(args, "cursor").map(ToOwned::to_owned))
        .transpose()?;
    if text_field.is_none() && cursor.is_some() {
        return Err(failure(
            "invalid_cursor",
            "a row-detail continuation cursor requires text_field",
            Some("cursor"),
            false,
        ));
    }
    if text_field.is_none() && byte_offset.is_some() {
        return Err(failure(
            "invalid_parameter",
            "byte_offset requires text_field",
            Some("byte_offset"),
            false,
        ));
    }
    Ok(RowDetailQuery {
        timestamp,
        request: api::RowDetailRequest {
            segment_id,
            type_id,
            row_ordinal: ordinal,
            timestamp_us: timestamp,
            fields,
            text_field,
            byte_offset,
            byte_limit,
            cursor,
        },
    })
}

fn detail_fields(
    args: &Map<String, Value>,
    section: &str,
    type_id: u32,
    contract: &'static TypeContract,
) -> Result<Vec<String>, ExpertFailure> {
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
    Ok(fields)
}

fn detail_text_field(
    args: &Map<String, Value>,
    type_id: u32,
    contract: &'static TypeContract,
) -> Result<Option<String>, ExpertFailure> {
    let text_field = args
        .get("text_field")
        .map(|_value| string(args, "text_field").map(ToOwned::to_owned))
        .transpose()?;
    if let Some(field) = text_field.as_deref() {
        let Some(column) = contract.column(field) else {
            return Err(failure(
                "no_such_column",
                format!("physical type id {type_id} has no field {field:?}"),
                Some("text_field"),
                false,
            ));
        };
        if column.ty != ColumnType::StrId {
            return Err(failure(
                "text_field_not_text",
                format!("field {field:?} is not a text or blob value"),
                Some("text_field"),
                false,
            ));
        }
    }
    Ok(text_field)
}

#[derive(Clone, PartialEq, Eq)]
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
    let message = match error {
        _ if retryable => "source changed during the read; retry the request".to_owned(),
        ApiError::Unreadable(_) => "data could not be read".to_owned(),
        error => error.to_string(),
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

fn history_prefix_too_large() -> ExpertFailure {
    failure(
        "result_too_large",
        "the fixed history metadata and first selected sample exceed data_budget_bytes",
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

fn optional_usize_arg(
    args: &Map<String, Value>,
    name: &str,
    minimum: usize,
    maximum: usize,
) -> Result<Option<usize>, ExpertFailure> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
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
    Ok(Some(value))
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

fn active_segment(segments: &[SegmentInfo]) -> Option<(i64, u64)> {
    segments.iter().find_map(|segment| {
        segment
            .active_position
            .map(|position| (segment.id, position))
    })
}

fn ensure_history_source_unchanged(
    state: &State,
    window: Window,
    expected: &[SegmentInfo],
    cancelled: &impl Fn() -> bool,
) -> Result<(), ExpertFailure> {
    let current = catalog(state, window, cancelled)?;
    if catalog_segments(&current.records) != expected {
        return Err(failure(
            "source_changed",
            "the metric-history source changed during the read; retry the request",
            None,
            true,
        ));
    }
    Ok(())
}

fn catalog_warnings(records: &[Value]) -> Vec<Value> {
    records_named(records, "warning")
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
    let next_cursor = next_cursor.map_or(Value::Null, Value::String);
    json!({
        "returned": returned,
        "truncated": truncated,
        "next_cursor": next_cursor,
        "stop_reason": reason,
    })
}

fn empty_snapshot(at: i64, section: &str, mut warnings: Vec<Value>) -> ExpertPayload {
    warnings.push(json!({
        "code": "section_unavailable",
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
        summary: "No sample exists at or before the requested time.".to_owned(),
    }
}

fn snapshot_payload(
    at: i64,
    mut segment: SegmentInfo,
    collected: &ValueCollection,
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
    segment.active_position = collected
        .records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("snapshot"))
        .and_then(|record| record.pointer("/segment/active_wal_position"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse().ok());
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
            "Returned {returned} rows from the latest sample at or before the requested time."
        ),
    })
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
