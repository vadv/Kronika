//! `kronika_find_events`: bounded merge across the recorded event-shaped
//! log sections (`pg_log_*`, `pgbouncer_events`), through
//! `fetch_bounded_events` called once per requested source. No existing
//! code merges rows across sources — the Events console
//! (`ui/src/events-view.tsx`) does that grouping in the browser, over
//! separate per-section HTTP fetches.

use std::path::Path;

use kronika_reader::{Reader, SegmentRef};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::ApiError;
use crate::api::history::{EventRowOut, fetch_bounded_events};
use crate::config::Config;
use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Window};

use super::catalog::EventsInput;
use super::semantics::{bounded_limit, mcp_error, mcp_structured};

/// The event-shaped logical sections this tool can read: the Events
/// console's own `EVENT_STREAMS` list (`ui/src/events-view.tsx`) minus
/// `pg_settings`, which the console reads only to label a slow-query
/// threshold and carries no event rows of its own.
const SOURCES: [&str; 7] = [
    "pg_log_errors",
    "pg_log_checkpoints",
    "pg_log_autovacuum",
    "pg_log_slow_queries",
    "pg_log_lock_waits",
    "pg_log_lifecycle",
    "pgbouncer_events",
];

/// One hour in microseconds, minus one: the widest window this tool
/// accepts, matching the Events console's own window span
/// (`ui/src/api.ts`: `from=start&to=start + 3_600_000_000 - 1`).
const MAX_WINDOW_MICROS: i64 = 3_600_000_000 - 1;

pub(crate) fn call(config: &Config, arguments: Map<String, Value>) -> CallToolResult {
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
    let mut rows: Vec<(&'static str, EventRowOut)> = Vec::new();
    let mut has_more = false;
    for source in sources {
        let (source_rows, source_has_more) =
            match fetch_bounded_events(&reader, &segments, source, window, limit, &|| false) {
                Ok(result) => result,
                Err(error) => return mcp_error(error.to_string()),
            };
        has_more = has_more || source_has_more;
        rows.extend(source_rows.into_iter().map(|row| (source, row)));
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
        "{row_count} event row{}{}",
        if row_count == 1 { "" } else { "s" },
        if has_more { ", more available" } else { "" },
    );
    mcp_structured(json!({ "rows": rows, "has_more": has_more }), summary)
}

/// Validates `sources` against [`SOURCES`], defaulting to all seven when
/// omitted.
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

/// Rejects a window wider than [`MAX_WINDOW_MICROS`] or with `to` before
/// `from`, before any segment is opened.
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
             {MAX_WINDOW_MICROS} microseconds (one hour)"
        ));
    }
    Ok(())
}

/// Opens the segments overlapping `[from, to]`, sorted by `min_ts` — the
/// same ascending order `fetch_bounded_events` relies on to bound a
/// timestamp-ordered scan without a heap.
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

/// Flattens one event row into a single keyed JSON object: `fields` first,
/// then `source` and the locator (`segment_id`/`type_id`/`row_ordinal`/
/// `at`) written as decimal strings, the same convention
/// `kronika_get_row_detail` (`mcp/row_detail.rs`) uses for these same four
/// fields, so a caller can copy them straight into that tool's arguments.
fn row_to_json(source: &str, row: EventRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    object.insert("source".to_owned(), json!(source));
    object.insert("segment_id".to_owned(), json!(row.segment_id.to_string()));
    object.insert("type_id".to_owned(), json!(row.type_id.to_string()));
    object.insert("row_ordinal".to_owned(), json!(row.row_ordinal.to_string()));
    object.insert("at".to_owned(), json!(row.at.to_string()));
    Value::Object(object)
}
