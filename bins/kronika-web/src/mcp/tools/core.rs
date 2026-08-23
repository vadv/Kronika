mod tree;

use std::ops::Bound::Included;

use kronika_reader::{Reader, SegmentRef};
use serde_json::{Map, Value, json};

use super::{Failure, Payload};
use crate::api::{self, ApiError, ValueCollection, ValueLimits, ValueStopReason};
use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::mcp::State;
use crate::route::{Filter, HeatmapRequest, HourRequest, Order, Route, SnapshotRequest, Window};

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
        "kronika_get_context" => context(state),
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

fn context(state: &State) -> Result<Payload, Failure> {
    let semantics = crate::product_semantics::all()
        .map_err(|error| Failure::bounded("semantics_unreadable", error.to_string()))?;
    Ok(Payload {
        anchor: anchor(None, None, None),
        data: json!({
            "context": {
                "historical_only": true,
                "synthetic_demo": state.synthetic_demo,
                "configured_sources": source_values(state.sources),
                "limits": {
                    "request_body_bytes": super::super::REQUEST_BODY_BYTES,
                    "structured_content_bytes": super::super::STRUCTURED_CONTENT_BYTES,
                    "response_body_bytes": super::super::RESPONSE_BODY_BYTES,
                    "segments": MAX_SEGMENTS,
                    "physical_row_visits": MAX_ROWS,
                    "concurrent_heavy_scans": 2,
                }
            },
            "surfaces": [
                {"tool": "kronika_get_context"},
                {"tool": "kronika_list_hours"},
                {"tool": "kronika_rank_heatmap", "surfaces": ["processes", "statements", "plans", "databases", "tables", "indexes", "cgroups"]},
                {"tool": "kronika_list_findings"},
                {"tool": "kronika_get_timeline"},
                {"tool": "kronika_get_host_context", "lenses": ["identity", "cpu", "memory", "storage", "filesystem", "network", "kernel", "cgroup"]},
                {"tool": "kronika_find_processes", "lenses": ["identity", "cpu", "memory", "disk", "tree"]}
            ],
            "semantics": semantics,
        }),
        page: page(0, false, None, "complete"),
        warnings: Vec::new(),
        summary:
            "Kronika returned its read-only historical surfaces, limits, and accepted semantics."
                .to_owned(),
    })
}

fn hours(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    reject_cursor(args)?;
    let from = optional_i64(args, "from_us")?;
    let to = optional_i64(args, "to_us")?;
    validate_optional_window(from, to)?;
    let limit = usize_arg(args, "limit", 100, 500)?;
    let collected = run_route(
        state,
        Route::Hour(HourRequest {
            window: Window { from, to },
            series: None,
        }),
        budget,
        1,
        cancelled,
    )?;
    let available = collected
        .records
        .first()
        .and_then(|record| record.get("available_hours"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let selected = available
        .into_iter()
        .filter(|hour| {
            hour.as_str()
                .and_then(|hour| hour.parse::<i64>().ok())
                .is_some_and(|hour| {
                    from.is_none_or(|from| hour.saturating_add(HOUR_US - 1) >= from)
                        && to.is_none_or(|to| hour <= to)
                })
        })
        .collect::<Vec<_>>();
    let truncated = selected.len() > limit;
    let hours = selected
        .into_iter()
        .take(limit)
        .filter_map(|start| {
            let start = start.as_str()?.parse::<i64>().ok()?;
            Some(json!({
                "start_us": start.to_string(),
                "end_us": start.saturating_add(HOUR_US - 1).to_string(),
            }))
        })
        .collect::<Vec<_>>();
    Ok(Payload {
        anchor: anchor(None, None, None),
        data: json!({"hours": hours, "sources": source_values(state.sources)}),
        page: page(
            hours.len(),
            truncated,
            None,
            if truncated { "row_limit" } else { "complete" },
        ),
        warnings: warnings(&collected.records),
        summary: format!("Kronika returned {} recorded UTC hour(s).", hours.len()),
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
    let (section, fields, labels, semantic) = heatmap_cut(surface, cut)?;
    admit_window(state, from, to, Some(section))?;
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
            section: section.to_owned(),
            fields: fields.iter().map(|field| (*field).to_owned()).collect(),
            columns,
            top,
            labels: labels.iter().map(|field| (*field).to_owned()).collect(),
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
        anchor: anchor(None, Some(from), None),
        data: json!({
            "intervals": header.and_then(|value| value.get("intervals")).cloned().unwrap_or_else(|| json!([])),
            "rows": rows,
            "totals": totals,
            "others": others,
            "semantics": [semantic],
        }),
        page: page(
            rows.len(),
            collected.stop_reason != ValueStopReason::Complete,
            None,
            stop,
        ),
        warnings: warnings(&collected.records),
        summary: format!(
            "Kronika returned {} ranked Heatmap row(s); ranking is recorded activity, not anomaly or cause.",
            rows.len()
        ),
    })
}

fn findings(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    reject_cursor(args)?;
    let (from, to) = required_window(args)?;
    admit_window(state, from, to, None)?;
    let limit = usize_arg(args, "limit", 100, 500)?;
    let surface = optional_string(args, "surface")?;
    let kind = optional_string(args, "kind")?;
    let collected = collect_hour(
        state,
        from,
        to,
        budget,
        limit.saturating_add(256),
        cancelled,
    )?;
    let mut found = records_named(&collected.records, "finding");
    found.retain(|finding| {
        surface.is_none_or(|surface| finding["logical_name"] == surface)
            && kind.is_none_or(|kind| finding["kind"] == kind)
    });
    let truncated = found.len() > limit || collected.stop_reason != ValueStopReason::Complete;
    found.truncate(limit);
    Ok(Payload {
        anchor: anchor(None, Some(from), None),
        data: json!({"findings": found, "semantics": []}),
        page: page(
            found.len(),
            truncated,
            None,
            if truncated { "row_limit" } else { "complete" },
        ),
        warnings: warnings(&collected.records),
        summary: format!("Kronika returned {} sparse finding(s).", found.len()),
    })
}

fn timeline(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    reject_cursor(args)?;
    let (from, to) = required_window(args)?;
    admit_window(state, from, to, None)?;
    let limit = usize_arg(args, "limit", 200, 1_000)?;
    let wanted = string_array(args, "lanes")?;
    let collected = collect_hour(
        state,
        from,
        to,
        budget,
        limit.saturating_add(256),
        cancelled,
    )?;
    let mut lanes = collected
        .records
        .iter()
        .filter(|record| record["record"] == "lane" || record["record"] == "point")
        .filter(|record| {
            wanted.is_empty()
                || record
                    .get("lane")
                    .or_else(|| record.get("series"))
                    .and_then(Value::as_str)
                    .is_some_and(|lane| wanted.iter().any(|wanted| wanted == lane))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut markers = records_named(&collected.records, "finding");
    let truncated = lanes.len().saturating_add(markers.len()) > limit
        || collected.stop_reason != ValueStopReason::Complete;
    if lanes.len() >= limit {
        lanes.truncate(limit);
        markers.clear();
    } else {
        markers.truncate(limit - lanes.len());
    }
    let returned = lanes.len().saturating_add(markers.len());
    Ok(Payload {
        anchor: anchor(None, Some(from), None),
        data: json!({"lanes": lanes, "markers": markers, "semantics": []}),
        page: page(
            returned,
            truncated,
            None,
            if truncated { "row_limit" } else { "complete" },
        ),
        warnings: warnings(&collected.records),
        summary: format!("Kronika returned {returned} native Timeline record(s)."),
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
    let route = snapshot_route(args, segment.id(), at, section, None)?;
    let collected = run_route(state, route, budget, 520, cancelled)?;
    let page_value = snapshot_page(&collected.records, collected.stop_reason);
    Ok(Payload {
        anchor: anchor(
            Some(segment.id()),
            Some(at),
            selected_at(&collected.records),
        ),
        data: json!({"rows": collected.records, "health": {}, "semantics": []}),
        page: page_value,
        warnings: warnings(&collected.records),
        summary: format!("Kronika returned recorded Host {lens} context."),
    })
}

fn processes(
    state: &State,
    args: &Map<String, Value>,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Payload, Failure> {
    let at = required_i64(args, "at_us")?;
    let lens = optional_string(args, "lens")?.unwrap_or("identity");
    let segment = select_segment_at(state, at)?;
    let order = optional_string(args, "order")?.or_else(|| Some(process_order(lens)));
    let route = snapshot_route(args, segment.id(), at, "os_process", order)?;
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
                "The complete Process snapshot does not fit the bounded tree admission limit.",
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
                "The complete filtered Process snapshot does not fit the bounded tree admission limit.",
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
    Ok(Payload {
        anchor: anchor(
            Some(segment.id()),
            Some(at),
            selected_at(&collected.records),
        ),
        data: json!({"processes": collected.records, "semantics": process_semantics()}),
        page: page_value,
        warnings: warnings(&collected.records),
        summary: format!(
            "Kronika returned {row_count} recorded Process row(s) for the {lens} lens."
        ),
    })
}

fn collect_hour(
    state: &State,
    from: i64,
    to: i64,
    budget: usize,
    records: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<ValueCollection, Failure> {
    run_route(
        state,
        Route::Hour(HourRequest {
            window: Window {
                from: Some(from),
                to: Some(to),
            },
            series: None,
        }),
        budget,
        records,
        cancelled,
    )
}

fn run_route(
    state: &State,
    route: Route,
    budget: usize,
    records: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<ValueCollection, Failure> {
    api::prepare_for_mcp(&state.data_root, state.sources, state.synthetic_demo, route)
        .map_err(api_failure)?
        .collect_values(
            ValueLimits {
                records,
                ndjson_bytes: budget.saturating_sub(2_048).max(1_024),
            },
            cancelled,
        )
        .map_err(api_failure)
}

fn snapshot_route(
    args: &Map<String, Value>,
    segment_id: i64,
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
        at,
        sections: vec![section.to_owned()],
        fields: string_array(args, "fields")?,
        by: order
            .map(|order| vec![stored_process_order(order)])
            .unwrap_or_default(),
        direction,
        group: None,
        page_size: Some(usize_arg(args, "page_size", 100, 500)?),
        cursor: optional_string(args, "cursor")?.map(str::to_owned),
        search: optional_string(args, "find")?.map(str::to_owned),
        first_match: false,
        text: None,
        filters: filter_values(args)?,
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
        .ok_or_else(|| {
            Failure::bounded(
                "no_such_segment",
                "No recorded segment exists at or before at_us.",
            )
        })
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
            "The selected recorded rows exceed the 1,000,000-row admission limit.",
        ));
    }
    Ok(())
}

fn heatmap_cut(
    surface: &str,
    cut: &str,
) -> Result<
    (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        Value,
    ),
    Failure,
> {
    let (section, fields, labels, unit, scale) = match (surface, cut) {
        ("processes", "cpu") => (
            "os_process",
            &["utime", "stime"][..],
            &[][..],
            "seconds",
            Some("clock_ticks"),
        ),
        ("processes", "rss") => (
            "os_process",
            &["rmem_kb"][..],
            &[][..],
            "bytes",
            Some("kib"),
        ),
        ("processes", "io_read") => ("os_process", &["read_bytes"][..], &[][..], "bytes", None),
        ("processes", "io_write") => ("os_process", &["write_bytes"][..], &[][..], "bytes", None),
        ("processes", "majflt") => ("os_process", &["majflt"][..], &[][..], "count", None),
        ("processes", "run_delay") => (
            "os_process",
            &["rundelay_ns"][..],
            &[][..],
            "nanoseconds",
            None,
        ),
        ("statements", "exec_time") => (
            "pg_stat_statements",
            &["total_exec_time"][..],
            &["datname", "usename"][..],
            "milliseconds",
            None,
        ),
        ("statements", "calls") => (
            "pg_stat_statements",
            &["calls"][..],
            &["datname", "usename"][..],
            "count",
            None,
        ),
        ("statements", "rows") => (
            "pg_stat_statements",
            &["rows"][..],
            &["datname", "usename"][..],
            "count",
            None,
        ),
        ("statements", "shared_read") => (
            "pg_stat_statements",
            &["shared_blks_read"][..],
            &["datname", "usename"][..],
            "bytes",
            Some("block_size"),
        ),
        ("statements", "shared_dirtied") => (
            "pg_stat_statements",
            &["shared_blks_dirtied"][..],
            &["datname", "usename"][..],
            "bytes",
            Some("block_size"),
        ),
        ("statements", "temp_written") => (
            "pg_stat_statements",
            &["temp_blks_written"][..],
            &["datname", "usename"][..],
            "bytes",
            Some("block_size"),
        ),
        ("statements", "wal_bytes") => (
            "pg_stat_statements",
            &["wal_bytes"][..],
            &["datname", "usename"][..],
            "bytes",
            None,
        ),
        ("plans", "exec_time") => (
            "pg_store_plans",
            &["total_time"][..],
            &["datname", "usename"][..],
            "milliseconds",
            None,
        ),
        ("plans", "calls") => (
            "pg_store_plans",
            &["calls"][..],
            &["datname", "usename"][..],
            "count",
            None,
        ),
        ("plans", "rows") => (
            "pg_store_plans",
            &["rows"][..],
            &["datname", "usename"][..],
            "count",
            None,
        ),
        ("plans", "shared_read") => (
            "pg_store_plans",
            &["shared_blks_read"][..],
            &["datname", "usename"][..],
            "bytes",
            Some("block_size"),
        ),
        ("plans", "temp_written") => (
            "pg_store_plans",
            &["temp_blks_written"][..],
            &["datname", "usename"][..],
            "bytes",
            Some("block_size"),
        ),
        ("databases", "commits") => (
            "pg_stat_database",
            &["xact_commit"][..],
            &["datname"][..],
            "count",
            None,
        ),
        ("databases", "rollbacks") => (
            "pg_stat_database",
            &["xact_rollback"][..],
            &["datname"][..],
            "count",
            None,
        ),
        ("databases", "db_read") => (
            "pg_stat_database",
            &["blks_read"][..],
            &["datname"][..],
            "bytes",
            Some("block_size"),
        ),
        ("databases", "temp_bytes") => (
            "pg_stat_database",
            &["temp_bytes"][..],
            &["datname"][..],
            "bytes",
            None,
        ),
        ("databases", "deadlocks") => (
            "pg_stat_database",
            &["deadlocks"][..],
            &["datname"][..],
            "count",
            None,
        ),
        ("tables", "writes") => (
            "pg_stat_user_tables",
            &["n_tup_ins", "n_tup_upd", "n_tup_del"][..],
            &["datname", "schemaname", "relname"][..],
            "count",
            None,
        ),
        ("tables", "seq_read") => (
            "pg_stat_user_tables",
            &["seq_tup_read"][..],
            &["datname", "schemaname", "relname"][..],
            "count",
            None,
        ),
        ("tables", "heap_read") => (
            "pg_stat_user_tables",
            &["heap_blks_read"][..],
            &["datname", "schemaname", "relname"][..],
            "bytes",
            Some("block_size"),
        ),
        ("tables", "dead_tuples") => (
            "pg_stat_user_tables",
            &["n_dead_tup"][..],
            &["datname", "schemaname", "relname"][..],
            "count",
            None,
        ),
        ("tables", "autovacuum_time") => (
            "pg_stat_user_tables",
            &["total_autovacuum_time"][..],
            &["datname", "schemaname", "relname"][..],
            "milliseconds",
            None,
        ),
        ("indexes", "idx_scan") => (
            "pg_stat_user_indexes",
            &["idx_scan"][..],
            &["datname", "schemaname", "relname", "indexrelname"][..],
            "count",
            None,
        ),
        ("indexes", "idx_tup_read") => (
            "pg_stat_user_indexes",
            &["idx_tup_read"][..],
            &["datname", "schemaname", "relname", "indexrelname"][..],
            "count",
            None,
        ),
        ("indexes", "idx_blks_read") => (
            "pg_stat_user_indexes",
            &["idx_blks_read"][..],
            &["datname", "schemaname", "relname", "indexrelname"][..],
            "bytes",
            Some("block_size"),
        ),
        ("cgroups", "cg_cpu") => (
            "os_cgroup_cpu",
            &["usage_usec"][..],
            &[][..],
            "microseconds",
            None,
        ),
        ("cgroups", "cg_throttled") => (
            "os_cgroup_cpu",
            &["throttled_usec"][..],
            &[][..],
            "microseconds",
            None,
        ),
        ("cgroups", "cg_read") => ("os_cgroup_io", &["rbytes"][..], &[][..], "bytes", None),
        ("cgroups", "cg_write") => ("os_cgroup_io", &["wbytes"][..], &[][..], "bytes", None),
        ("cgroups", "cg_rios") => ("os_cgroup_io", &["rios"][..], &[][..], "count", None),
        ("cgroups", "cg_wios") => ("os_cgroup_io", &["wios"][..], &[][..], "count", None),
        _ => {
            return Err(Failure::input(
                "cut",
                "cut is not an accepted metric for this surface.",
            ));
        }
    };
    Ok((
        section,
        fields,
        labels,
        json!({
            "id": format!("heatmap.{surface}.{cut}"),
            "origin": "accepted_presentation",
            "fields": fields,
            "unit": unit,
            "scale_by": scale,
        }),
    ))
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
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            Ok(Filter {
                column: column.clone(),
                value,
            })
        })
        .collect()
}

fn reject_cursor(args: &Map<String, Value>) -> Result<(), Failure> {
    if args.contains_key("cursor") {
        return Err(Failure::input(
            "cursor",
            "This first bounded implementation has no continuation cursor for this surface.",
        ));
    }
    Ok(())
}

fn process_order(lens: &str) -> &'static str {
    match lens {
        "cpu" => "utime",
        "memory" => "rmem_kb",
        "disk" => "read_bytes",
        _ => "pid",
    }
}

fn stored_process_order(order: &str) -> String {
    match order {
        "cpu_cores" | "user_cpu_cores" => "utime",
        "rss" => "rmem_kb",
        "disk_read_rate" => "read_bytes",
        "disk_write_rate" => "write_bytes",
        other => other,
    }
    .to_owned()
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

fn anchor(segment_id: Option<i64>, requested_at: Option<i64>, selected_at: Option<i64>) -> Value {
    json!({
        "hour_start_us": requested_at.map(|value| value.div_euclid(HOUR_US).saturating_mul(HOUR_US).to_string()),
        "requested_at_us": requested_at.map(|value| value.to_string()),
        "selected_at_us": selected_at.map(|value| value.to_string()),
        "segment_id": segment_id.map(|value| value.to_string()),
        "active_wal_position": Value::Null,
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

fn api_failure(error: ApiError) -> Failure {
    Failure {
        code: error.code(),
        message: error.to_string(),
        parameter: error.parameter().map(str::to_owned),
        retryable: error.source_changed_during_read(),
    }
}

fn unreadable(message: String) -> Failure {
    Failure {
        code: "unreadable",
        message,
        parameter: None,
        retryable: true,
    }
}
