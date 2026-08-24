mod discovery;
mod pagination;
mod tree;

use std::ops::Bound::Included;

use kronika_reader::{Reader, SegmentRef};
use serde_json::{Map, Value, json};

use super::{Failure, Payload};
use crate::api::{self, ApiError, ValueCollection, ValueLimits, ValueStopReason};
use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::mcp::State;
use crate::route::{
    Filter, HeatmapRequest, Order, ProcessLens, ProcessSurfaceRequest, Route, SnapshotRequest,
    Window,
};

const HOUR_US: i64 = 3_600_000_000;
const MAX_SEGMENTS: usize = 64;
const MAX_ROWS: u64 = 1_000_000;
const MAX_TREE_ROWS: usize = 500;

pub(super) fn execute(
    state: &State,
    name: &str,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    match name {
        "kronika_get_context" => discovery::payload(state, cancelled),
        "kronika_list_hours" => hours(state, args, budget, cancelled),
        "kronika_rank_heatmap" => heatmap(state, args, budget, cancelled),
        "kronika_list_findings" => findings(state, args, budget, cancelled),
        "kronika_get_timeline" => timeline(state, args, budget, cancelled),
        "kronika_get_host_context" => host(state, args, budget, cancelled),
        "kronika_find_processes" => processes(state, args, budget, cancelled),
        _ => Err(Failure::bounded(
            "not_wired",
            "This historical surface is not wired yet.",
        )),
    }
}

fn hours(
    state: &State,
    args: &Map<String, Value>,
    _budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    let from = optional_i64(args, "from_us")?;
    let to = optional_i64(args, "to_us")?;
    validate_optional_window(from, to)?;
    let limit = usize_arg(args, "limit", 100, 500)?;
    let paged = pagination::hours(
        &state.data_root,
        Window { from, to },
        optional_string(args, "cursor")?,
        limit,
        cancelled,
    )?;
    let page_value = page(
        paged.page.returned,
        paged.page.truncated,
        paged.page.next_cursor.as_deref(),
        paged.page.stop_reason,
    );
    let returned = paged.hours.len();
    Ok(Payload {
        anchor: anchor(None, None, None, None),
        data: json!({"hours": paged.hours, "sources": source_values(state.sources)}),
        page: page_value,
        warnings: Vec::new(),
        summary: format!("Returned {returned} UTC hour(s)."),
    })
}

fn heatmap(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    let from = required_i64(args, "from_us")?;
    let to = required_i64(args, "to_us")?;
    validate_window(from, to)?;
    let surface = required_string(args, "surface")?;
    let cut = required_string(args, "cut")?;
    let spec = discovery::heatmap_cut(surface, cut)
        .ok_or_else(|| Failure::input("cut", "cut is not an accepted metric for this surface."))?;
    admit_window(state, from, to, Some(spec.section))?;
    let columns = usize_arg(args, "columns", 12, 1_440)?;
    let top = usize_arg(args, "top", 25, 500)?;
    let group = match optional_string(args, "group")? {
        None | Some("identity") => Vec::new(),
        Some("command") if surface == "processes" => vec!["comm".to_owned()],
        Some("database") => vec!["datname".to_owned()],
        Some("schema") => vec!["datname".to_owned(), "schemaname".to_owned()],
        Some("tablespace") => vec!["tablespace".to_owned()],
        Some(_other) => {
            return Err(Failure::input(
                "group",
                "group is not valid for this surface.",
            ));
        }
    };
    let collected = run_route(
        state,
        Route::Heatmap(HeatmapRequest {
            from,
            to,
            section: spec.section.to_owned(),
            fields: spec
                .fields
                .iter()
                .map(|field| (*field).to_owned())
                .collect(),
            columns,
            top,
            labels: spec
                .labels
                .iter()
                .map(|field| (*field).to_owned())
                .collect(),
            group,
            type_id: None,
        }),
        budget,
        top.saturating_add(3),
        cancelled,
    )?;
    let header = collected
        .records
        .iter()
        .find(|value| value["record"] == "heatmap");
    let rows = records_named(&collected.records, "heatmap_row");
    let totals = collected
        .records
        .iter()
        .find(|value| value["record"] == "heatmap_band" && value["band"] == "totals")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let others = collected
        .records
        .iter()
        .find(|value| value["record"] == "heatmap_band" && value["band"] == "others")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let stop = collected.stop_reason.code();
    Ok(Payload {
        anchor: anchor(None, Some(from), None, None),
        data: json!({
            "intervals": header.and_then(|value| value.get("intervals")).cloned().unwrap_or_else(|| json!([])),
            "rows": rows,
            "totals": totals,
            "others": others,
            "semantics": [spec.semantic()],
        }),
        page: page(
            rows.len(),
            collected.stop_reason != ValueStopReason::Complete,
            None,
            stop,
        ),
        warnings: warnings(&collected.records),
        summary: format!("Returned {} ranked Heatmap row(s).", rows.len()),
    })
}

fn findings(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    let (from, to) = required_window(args)?;
    admit_window(state, from, to, None)?;
    let limit = usize_arg(args, "limit", 100, 500)?;
    let surface = optional_string(args, "surface")?;
    let kind = optional_string(args, "kind")?;
    let paged = pagination::findings(
        state,
        Window {
            from: Some(from),
            to: Some(to),
        },
        surface,
        kind,
        optional_string(args, "cursor")?,
        limit,
        budget,
        cancelled,
    )?;
    let page_value = page(
        paged.page.returned,
        paged.page.truncated,
        paged.page.next_cursor.as_deref(),
        paged.page.stop_reason,
    );
    let returned = paged.findings.len();
    Ok(Payload {
        anchor: anchor(None, Some(from), None, None),
        data: json!({"findings": paged.findings, "semantics": paged.semantics}),
        page: page_value,
        warnings: paged.warnings,
        summary: format!("Returned {returned} sparse finding(s)."),
    })
}

fn timeline(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    let (from, to) = required_window(args)?;
    admit_window(state, from, to, None)?;
    let limit = usize_arg(args, "limit", 200, 1_000)?;
    let wanted = string_array(args, "lanes")?;
    let paged = pagination::timeline(
        state,
        Window {
            from: Some(from),
            to: Some(to),
        },
        &wanted,
        optional_string(args, "cursor")?,
        limit,
        budget,
        cancelled,
    )?;
    let page_value = page(
        paged.page.returned,
        paged.page.truncated,
        paged.page.next_cursor.as_deref(),
        paged.page.stop_reason,
    );
    let returned = paged.lanes.len().saturating_add(paged.markers.len());
    Ok(Payload {
        anchor: anchor(None, Some(from), None, None),
        data: json!({"lanes": paged.lanes, "markers": paged.markers, "semantics": paged.semantics}),
        page: page_value,
        warnings: paged.warnings,
        summary: format!("Returned {returned} native Timeline record(s)."),
    })
}

fn host(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    let at = required_i64(args, "at_us")?;
    let lens = required_string(args, "lens")?;
    let section = match lens {
        "identity" => "instance_metadata",
        "cpu" => "os_cpu",
        "memory" => "os_meminfo",
        "storage" => "os_diskstats",
        "filesystem" => "os_mountinfo",
        "network" => "os_netdev",
        "kernel" => "os_vmstat",
        "cgroup" => "os_cgroup_context",
        _ => return Err(Failure::input("lens", "unknown Host lens.")),
    };
    let segment = select_segment_at(state, at)?;
    let route = snapshot_route(
        args,
        segment.id(),
        segment.active_position(),
        at,
        section,
        None,
    )?;
    let collected = run_route(state, route, budget, 520, cancelled)?;
    let page_value = snapshot_page(&collected.records, collected.stop_reason);
    let active_position = snapshot_active_position(&collected.records)?;
    Ok(Payload {
        anchor: anchor(
            Some(segment.id()),
            Some(at),
            selected_at(&collected.records),
            active_position,
        ),
        data: json!({
            "rows": collected.records,
            "health": {},
            "semantics": crate::mcp::semantics::health(),
        }),
        page: page_value,
        warnings: warnings(&collected.records),
        summary: format!("Returned Host {lens} context."),
    })
}

fn processes(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    let at = required_i64(args, "at_us")?;
    let lens = optional_string(args, "lens")?.unwrap_or("tree");
    let process_lens = ProcessLens::parse(Some(lens))
        .ok_or_else(|| Failure::input("lens", "unknown Process lens."))?;
    let segment = select_segment_at(state, at)?;
    let route = process_route(
        args,
        segment.id(),
        segment.active_position(),
        at,
        process_lens,
    )?;
    let collected = if lens == "tree" {
        let Route::Snapshot(request) = route else {
            return Err(Failure::bounded(
                "internal_error",
                "The Process tree did not receive a snapshot request.",
            ));
        };
        let request = tree::prepare(*request)?;
        let collected = run_route(
            state,
            Route::Snapshot(Box::new(request.complete)),
            super::super::STRUCTURED_CONTENT_BYTES,
            MAX_TREE_ROWS.saturating_add(20),
            cancelled,
        )?;
        let row_count = records_named(&collected.records, "row").len();
        if row_count > MAX_TREE_ROWS || collected.stop_reason != ValueStopReason::Complete {
            return Err(Failure::bounded(
                "tree_bound_exceeded",
                "The Process snapshot exceeds the tree admission limit.",
            ));
        }
        let matched = request
            .matched
            .map(|request| {
                run_route(
                    state,
                    Route::Snapshot(Box::new(request)),
                    super::super::STRUCTURED_CONTENT_BYTES,
                    MAX_TREE_ROWS.saturating_add(20),
                    cancelled,
                )
            })
            .transpose()?;
        if matched
            .as_ref()
            .is_some_and(|matched| matched.stop_reason != ValueStopReason::Complete)
        {
            return Err(Failure::bounded(
                "tree_bound_exceeded",
                "The filtered Process snapshot exceeds the tree admission limit.",
            ));
        }
        let transformed = tree::transform(
            collected.records,
            matched.as_ref().map(|matched| matched.records.as_slice()),
        )?;
        ValueCollection {
            records: transformed.records,
            ndjson_bytes: collected.ndjson_bytes,
            stop_reason: ValueStopReason::Complete,
        }
    } else {
        run_route(
            state,
            route,
            budget,
            MAX_TREE_ROWS.saturating_add(20),
            cancelled,
        )?
    };
    let row_count = collected
        .records
        .iter()
        .filter(|row| row["record"] == "row")
        .count();
    let page_value = snapshot_page(&collected.records, collected.stop_reason);
    let active_position = snapshot_active_position(&collected.records)?;
    Ok(Payload {
        anchor: anchor(
            Some(segment.id()),
            Some(at),
            selected_at(&collected.records),
            active_position,
        ),
        data: json!({"processes": collected.records, "semantics": process_semantics()}),
        page: page_value,
        warnings: warnings(&collected.records),
        summary: format!("Returned {row_count} Process row(s) for the {lens} lens."),
    })
}

fn run_route(
    state: &State,
    route: Route,
    budget: usize,
    records: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<ValueCollection, Failure> {
    api::prepare_for_mcp(&state.data_root, state.sources, state.synthetic_demo, route)
        .map_err(|error| api_failure(&error))?
        .collect_values(
            ValueLimits {
                records,
                ndjson_bytes: budget.saturating_sub(2_048).max(1_024),
            },
            cancelled,
        )
        .map_err(|error| api_failure(&error))
}

fn snapshot_route(
    args: &Map<String, Value>,
    segment_id: i64,
    active_position: Option<u64>,
    at: i64,
    section: &str,
    order: Option<&str>,
) -> Result<Route, Failure> {
    let direction = match optional_string(args, "direction")? {
        Some("asc") => Order::Asc,
        Some("desc") | None => Order::Desc,
        Some(_other) => {
            return Err(Failure::input(
                "direction",
                "direction must be asc or desc.",
            ));
        }
    };
    Ok(Route::Snapshot(Box::new(SnapshotRequest {
        segment_id,
        active_position,
        at,
        sections: vec![section.to_owned()],
        fields: string_array(args, "fields")?,
        by: order
            .map(|order| vec![order.to_owned()])
            .unwrap_or_default(),
        direction,
        group: None,
        postgresql: None,
        process: None,
        page_size: Some(usize_arg(args, "page_size", 100, 500)?),
        cursor: optional_string(args, "cursor")?.map(str::to_owned),
        search: optional_string(args, "find")?.map(str::to_owned),
        first_match: false,
        text: None,
        filters: filter_values(args)?,
        activity_visibility: None,
        type_id: None,
        row_ordinal: None,
    })))
}

fn process_route(
    args: &Map<String, Value>,
    segment_id: i64,
    active_position: Option<u64>,
    at: i64,
    lens: ProcessLens,
) -> Result<Route, Failure> {
    let direction = optional_string(args, "direction")?
        .map(|direction| match direction {
            "asc" => Ok(Order::Asc),
            "desc" => Ok(Order::Desc),
            _ => Err(Failure::input(
                "direction",
                "direction must be asc or desc.",
            )),
        })
        .transpose()?;
    Ok(Route::Snapshot(Box::new(SnapshotRequest {
        segment_id,
        active_position,
        at,
        sections: vec!["os_process".to_owned()],
        fields: string_array(args, "fields")?,
        by: Vec::new(),
        direction: Order::Desc,
        group: None,
        postgresql: None,
        process: Some(ProcessSurfaceRequest {
            lens,
            order: optional_string(args, "order")?.map(str::to_owned),
            direction,
        }),
        page_size: args
            .contains_key("page_size")
            .then(|| usize_arg(args, "page_size", 200, 500))
            .transpose()?,
        cursor: optional_string(args, "cursor")?.map(str::to_owned),
        search: optional_string(args, "find")?.map(str::to_owned),
        first_match: false,
        text: None,
        filters: Vec::new(),
        activity_visibility: None,
        type_id: None,
        row_ordinal: None,
    })))
}

fn select_segment_at(state: &State, at: i64) -> Result<SegmentRef, Failure> {
    let reader = Reader::open(&state.data_root).map_err(|error| unreadable(error.to_string()))?;
    let listing = reader
        .catalog_segments(..)
        .map_err(|error| unreadable(error.to_string()))?;
    listing
        .segments
        .into_iter()
        .filter(|segment| segment.min_ts() <= at)
        .max_by_key(|segment| (segment.min_ts(), segment.id()))
        .ok_or_else(|| Failure::bounded("no_such_segment", "No segment exists at or before at_us."))
}

fn admit_window(state: &State, from: i64, to: i64, section: Option<&str>) -> Result<(), Failure> {
    let reader = Reader::open(&state.data_root).map_err(|error| unreadable(error.to_string()))?;
    let listing = reader
        .catalog_segments((Included(from), Included(to)))
        .map_err(|error| unreadable(error.to_string()))?;
    if listing.segments.len() > MAX_SEGMENTS {
        return Err(Failure::bounded(
            "segment_limit_exceeded",
            "The interval overlaps more than 64 segments.",
        ));
    }
    let rows = listing
        .segments
        .iter()
        .flat_map(SegmentRef::sections)
        .filter(|stored| {
            section.is_none_or(|name| {
                kronika_registry::logical_section_name(stored.type_id) == Some(name)
            })
        })
        .try_fold(0_u64, |total, stored| total.checked_add(stored.rows))
        .unwrap_or(u64::MAX);
    if rows > MAX_ROWS {
        return Err(Failure::bounded(
            "scan_limit_exceeded",
            "The selected rows exceed the 1,000,000-row admission limit.",
        ));
    }
    Ok(())
}

fn required_window(args: &Map<String, Value>) -> Result<(i64, i64), Failure> {
    let from = required_i64(args, "from_us")?;
    let to = required_i64(args, "to_us")?;
    validate_window(from, to)?;
    Ok((from, to))
}

fn validate_window(from: i64, to: i64) -> Result<(), Failure> {
    if from > to || to.saturating_sub(from) >= HOUR_US {
        return Err(Failure::input(
            "to_us",
            "The inclusive interval must be ordered and no longer than one UTC hour.",
        ));
    }
    Ok(())
}

fn validate_optional_window(from: Option<i64>, to: Option<i64>) -> Result<(), Failure> {
    if from.zip(to).is_some_and(|(from, to)| from > to) {
        return Err(Failure::input("to_us", "to_us must not precede from_us."));
    }
    Ok(())
}

fn required_i64(args: &Map<String, Value>, name: &'static str) -> Result<i64, Failure> {
    optional_i64(args, name)?.ok_or_else(|| Failure::input(name, format!("{name} is required.")))
}

fn optional_i64(args: &Map<String, Value>, name: &'static str) -> Result<Option<i64>, Failure> {
    args.get(name)
        .map(|value| {
            value
                .as_str()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or_else(|| {
                    Failure::input(name, format!("{name} must be a decimal timestamp string."))
                })
        })
        .transpose()
}

fn required_string<'a>(
    args: &'a Map<String, Value>,
    name: &'static str,
) -> Result<&'a str, Failure> {
    optional_string(args, name)?.ok_or_else(|| Failure::input(name, format!("{name} is required.")))
}

fn optional_string<'a>(
    args: &'a Map<String, Value>,
    name: &'static str,
) -> Result<Option<&'a str>, Failure> {
    args.get(name)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| Failure::input(name, format!("{name} must be a string.")))
        })
        .transpose()
}

fn usize_arg(
    args: &Map<String, Value>,
    name: &'static str,
    default: usize,
    max: usize,
) -> Result<usize, Failure> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| Failure::input(name, format!("{name} must be an integer.")))?;
    if value == 0 || value > max {
        return Err(Failure::input(
            name,
            format!("{name} is outside its bounded range."),
        ));
    }
    Ok(value)
}

fn string_array(args: &Map<String, Value>, name: &'static str) -> Result<Vec<String>, Failure> {
    let Some(values) = args.get(name) else {
        return Ok(Vec::new());
    };
    values
        .as_array()
        .ok_or_else(|| Failure::input(name, format!("{name} must be an array.")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Failure::input(name, format!("{name} entries must be strings.")))
        })
        .collect()
}

fn filter_values(args: &Map<String, Value>) -> Result<Vec<Filter>, Failure> {
    let Some(filters) = args.get("filters") else {
        return Ok(Vec::new());
    };
    let filters = filters
        .as_object()
        .ok_or_else(|| Failure::input("filters", "filters must be an object."))?;
    filters
        .iter()
        .map(|(column, value)| {
            let value = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            Ok(Filter {
                column: column.clone(),
                value,
            })
        })
        .collect()
}

fn records_named(records: &[Value], name: &str) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record["record"] == name)
        .cloned()
        .collect()
}

fn warnings(records: &[Value]) -> Vec<Value> {
    records_named(records, "warning")
}

fn selected_at(records: &[Value]) -> Option<i64> {
    records
        .iter()
        .filter(|record| record["record"] == "row")
        .filter_map(|record| record["timestamp"].as_str()?.parse().ok())
        .max()
}

fn snapshot_page(records: &[Value], stop: ValueStopReason) -> Value {
    let trailer = records
        .iter()
        .find(|record| record["record"] == "snapshot_page");
    let returned = trailer
        .and_then(|value| value["returned"].as_str())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            records
                .iter()
                .filter(|record| record["record"] == "row")
                .count()
        });
    let truncated = trailer
        .and_then(|value| value["truncated"].as_bool())
        .unwrap_or(stop != ValueStopReason::Complete);
    let cursor = trailer
        .and_then(|value| value.get("next_cursor"))
        .cloned()
        .unwrap_or(Value::Null);
    page(returned, truncated, cursor.as_str(), stop.code())
}

fn page(returned: usize, truncated: bool, next_cursor: Option<&str>, stop_reason: &str) -> Value {
    json!({"returned": returned, "truncated": truncated, "next_cursor": next_cursor, "stop_reason": stop_reason})
}

fn anchor(
    segment_id: Option<i64>,
    requested_at: Option<i64>,
    selected_at: Option<i64>,
    active_position: Option<u64>,
) -> Value {
    json!({
        "hour_start_us": requested_at.map(|value| value.div_euclid(HOUR_US).saturating_mul(HOUR_US).to_string()),
        "requested_at_us": requested_at.map(|value| value.to_string()),
        "selected_at_us": selected_at.map(|value| value.to_string()),
        "segment_id": segment_id.map(|value| value.to_string()),
        "active_wal_position": active_position.map(|value| value.to_string()),
    })
}

fn snapshot_active_position(records: &[Value]) -> Result<Option<u64>, Failure> {
    let value = records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("snapshot"))
        .and_then(|record| record.pointer("/segment/active_wal_position"))
        .ok_or_else(|| {
            Failure::bounded(
                "snapshot_source_unavailable",
                "The snapshot has no active WAL position.",
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
            Failure::bounded(
                "snapshot_source_unavailable",
                "The snapshot active WAL position is invalid.",
            )
        })
}

fn source_values(configured: u32) -> Vec<Value> {
    [("os", SOURCE_OS), ("postgresql", SOURCE_POSTGRESQL)]
        .into_iter()
        .map(|(name, bit)| json!({"name": name, "configured": configured & bit != 0}))
        .collect()
}

fn process_semantics() -> Vec<Value> {
    [
        "value_tone.state",
        "value_tone.cpu_percent",
        "value_tone.rate_zero",
    ]
    .into_iter()
    .filter_map(|id| crate::product_semantics::get(id).ok().flatten())
    .filter_map(|definition| serde_json::to_value(definition).ok())
    .collect()
}

fn api_failure(error: &ApiError) -> Failure {
    if error.source_changed_during_read() {
        return Failure {
            code: "source_changed",
            message: "Source changed during the read; retry the request.".to_owned(),
            parameter: error.parameter().map(str::to_owned),
            retryable: true,
        };
    }
    Failure {
        code: error.code(),
        message: error.to_string(),
        parameter: error.parameter().map(str::to_owned),
        retryable: false,
    }
}

const fn unreadable(message: String) -> Failure {
    Failure {
        code: "unreadable",
        message,
        parameter: None,
        retryable: true,
    }
}

#[cfg(test)]
#[path = "core/tests.rs"]
mod tests;
