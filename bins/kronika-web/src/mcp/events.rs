//! `kronika_find_events`: bounded reads from recorded `PostgreSQL` and `PgBouncer`
//! event sections.

use std::path::Path;

use kronika_reader::{Reader, SegmentRef};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::ApiError;
use crate::api::history::{EventRowOut, fetch_bounded_events};
use crate::config::Config;
use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Window};

use super::catalog::EventsInput;
use super::event_labels::label_event_fields;
use super::semantics::{bounded_limit, mcp_error, mcp_structured};

/// Event sections accepted by this tool.
const SOURCES: [&str; 8] = [
    "pg_log_errors",
    "pg_log_checkpoints",
    "pg_log_autovacuum",
    "pg_log_slow_queries",
    "pg_log_lock_waits",
    "pg_log_temp_files",
    "pg_log_lifecycle",
    "pgbouncer_events",
];

/// Maximum `to - from` for an inclusive window: one hour exactly, so the
/// covered span is one hour plus one microsecond.
const MAX_WINDOW_MICROS: i64 = 3_600_000_000;

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: EventsInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => return mcp_error(format!("invalid arguments: {error}")),
    };
    let limit = match bounded_limit("limit", input.limit, MAX_SNAPSHOT_PAGE_SIZE) {
        Ok(limit) => limit,
        Err(error) => return error,
    };
    let sources = match resolve_sources(input.sources) {
        Ok(sources) => sources,
        Err(error) => return mcp_error(error),
    };
    if let Err(error) = check_window(input.from, input.to) {
        return mcp_error(error);
    }
    let (reader, segments) = match windowed_segments(&config.data_root, input.from, input.to) {
        Ok(result) => result,
        Err(error) => return mcp_error(error.to_string()),
    };

    let window = Window {
        from: Some(input.from),
        to: Some(input.to),
    };
    let by_source =
        match fetch_bounded_events(&reader, &segments, &sources, window, limit, &|| cancelled()) {
            Ok(result) => result,
            Err(error) => return mcp_error(error.to_string()),
        };
    let mut has_more = false;
    let mut rows: Vec<(&'static str, EventRowOut)> = Vec::new();
    for section in by_source {
        has_more = has_more || section.has_more;
        rows.extend(section.rows.into_iter().map(|row| (section.section, row)));
    }
    rows.sort_by_key(|(_, row)| row.at);
    has_more = has_more || rows.len() > limit;
    rows.truncate(limit);

    let row_count = rows.len();
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|(source, row)| row_to_json(source, row))
        .collect();
    let summary = format!(
        "Returned {row_count} recorded event row{}{}.",
        if row_count == 1 { "" } else { "s" },
        if has_more {
            "; result truncated to limit"
        } else {
            ""
        },
    );
    mcp_structured(json!({ "rows": rows, "has_more": has_more }), summary)
}

fn resolve_sources(requested: Option<Vec<String>>) -> Result<Vec<&'static str>, String> {
    let Some(requested) = requested else {
        return Ok(SOURCES.to_vec());
    };
    requested
        .iter()
        .map(|name| {
            SOURCES
                .iter()
                .copied()
                .find(|&source| source == name.as_str())
                .ok_or_else(|| format!("unknown source {name:?}: must be one of {SOURCES:?}"))
        })
        .collect()
}

/// Validates the inclusive `[from, to]` window before opening storage.
fn check_window(from: i64, to: i64) -> Result<(), String> {
    let span = to
        .checked_sub(from)
        .ok_or_else(|| format!("window is invalid: to ({to}) minus from ({from}) overflows"))?;
    if span < 0 {
        return Err(format!("to ({to}) must not be before from ({from})"));
    }
    if span > MAX_WINDOW_MICROS {
        return Err(format!(
            "window too wide: to - from is {span} microseconds, the limit is \
             {MAX_WINDOW_MICROS} microseconds"
        ));
    }
    Ok(())
}

/// Opens segments overlapping inclusive `[from, to]`, sorted by `min_ts`
/// so a full collector can skip every later segment.
fn windowed_segments(
    root: &Path,
    from: i64,
    to: i64,
) -> Result<(Reader, Vec<SegmentRef>), ApiError> {
    let reader = Reader::open(root)?;
    let stored = reader.catalog_segments(from..=to)?;
    let mut segments = stored.segments;
    segments.sort_by_key(SegmentRef::min_ts);
    Ok((reader, segments))
}

/// Adds labels, `source`, and `row_key`, then appends the decimal-string
/// locator fields accepted by `kronika_get_row_detail`.
fn row_to_json(source: &str, row: EventRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    label_event_fields(source, &mut object);
    super::row_key::attach(source, &mut object);
    object.insert("source".to_owned(), json!(source));
    object.insert("segment_id".to_owned(), json!(row.segment_id.to_string()));
    object.insert("type_id".to_owned(), json!(row.type_id.to_string()));
    object.insert("row_ordinal".to_owned(), json!(row.row_ordinal.to_string()));
    object.insert("at".to_owned(), json!(row.at.to_string()));
    Value::Object(object)
}
