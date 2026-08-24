//! Direct `PostgreSQL` MCP surfaces over the typed web API readers.

mod locks;
mod vacuum;

use std::collections::HashSet;

use kronika_registry::contract;
use serde_json::{Map, Value, json};

use super::State;
use crate::api::{self, ApiError, ValueLimits, ValueStopReason};
use crate::route::{
    ActivityVisibility, Filter, Order, RelationGroup, Route, SnapshotRequest, Window,
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
const DATABASE_FIELDS: &[&str] = &[
    "datid",
    "datname",
    "xact_commit",
    "xact_rollback",
    "blks_read",
    "blks_hit",
    "tup_returned",
    "tup_fetched",
    "tup_inserted",
    "tup_updated",
    "tup_deleted",
    "conflicts",
    "temp_files",
    "temp_bytes",
    "deadlocks",
    "checksum_failures",
    "sessions",
    "sessions_abandoned",
    "sessions_fatal",
    "sessions_killed",
    "frozen_xid_age",
    "min_mxid_age",
];
const STATEMENT_FIELDS: &[&str] = &[
    "queryid",
    "dbid",
    "userid",
    "toplevel",
    "datname",
    "usename",
    "query",
    "calls",
    "total_exec_time",
    "total_time",
    "rows",
];
const STATEMENT_PER_CALL_FIELDS: &[&str] = &[
    "queryid",
    "dbid",
    "userid",
    "toplevel",
    "datname",
    "usename",
    "query",
    "calls",
    "total_exec_time",
    "total_time",
    "rows",
    "shared_blks_hit",
    "shared_blks_read",
    "local_blks_hit",
    "local_blks_read",
];
const STATEMENT_IO_FIELDS: &[&str] = &[
    "queryid",
    "dbid",
    "userid",
    "toplevel",
    "datname",
    "usename",
    "query",
    "calls",
    "shared_blks_hit",
    "shared_blks_read",
    "shared_blks_dirtied",
    "shared_blks_written",
    "local_blks_hit",
    "local_blks_read",
    "temp_blks_read",
    "temp_blks_written",
];
const STATEMENT_RESOURCE_FIELDS: &[&str] = &[
    "queryid",
    "dbid",
    "userid",
    "toplevel",
    "datname",
    "usename",
    "query",
    "calls",
    "total_exec_time",
    "total_time",
    "total_plan_time",
    "temp_blks_written",
    "wal_bytes",
    "wal_records",
    "wal_fpi",
    "wal_buffers_full",
];
const STATEMENT_STABILITY_FIELDS: &[&str] = &[
    "queryid",
    "dbid",
    "userid",
    "toplevel",
    "datname",
    "usename",
    "query",
    "calls",
    "min_time",
    "max_time",
    "mean_time",
    "stddev_time",
    "min_exec_time",
    "max_exec_time",
    "mean_exec_time",
    "stddev_exec_time",
];
const PLAN_FIELDS: &[&str] = &[
    "userid",
    "dbid",
    "queryid",
    "queryid_stat_statements",
    "planid",
    "datname",
    "usename",
    "plan",
    "calls",
    "total_time",
    "rows",
];
const PLAN_TIMING_FIELDS: &[&str] = &[
    "userid",
    "dbid",
    "queryid",
    "queryid_stat_statements",
    "planid",
    "datname",
    "usename",
    "plan",
    "calls",
    "min_time",
    "max_time",
    "mean_time",
    "stddev_time",
    "first_call",
    "last_call",
];
const PLAN_IO_FIELDS: &[&str] = &[
    "userid",
    "dbid",
    "queryid",
    "queryid_stat_statements",
    "planid",
    "datname",
    "usename",
    "plan",
    "calls",
    "shared_blks_read",
    "shared_blks_hit",
    "shared_blks_dirtied",
    "local_blks_hit",
    "local_blks_read",
    "temp_blks_read",
];
const PLAN_IDENTITY_FIELDS: &[&str] = &[
    "userid",
    "dbid",
    "queryid",
    "queryid_stat_statements",
    "planid",
    "datname",
    "usename",
    "plan",
    "calls",
    "cmd_type",
    "relids",
];
const TABLE_FIELDS: &[&str] = &[
    "tuple_throughput",
    "sequential_share_pct",
    "seq_scan",
    "idx_scan",
    "seq_tuples_per_scan",
    "idx_tuples_per_scan",
    "last_seq_scan",
    "last_seq_scan_never",
    "last_idx_scan",
    "last_idx_scan_never",
];
const TABLE_ACCESS_AGGREGATE_FIELDS: &[&str] = &[
    "tuple_throughput",
    "sequential_share_pct",
    "seq_scan",
    "idx_scan",
    "seq_tuples_per_scan",
    "idx_tuples_per_scan",
    "last_seq_scan_oldest",
    "last_seq_scan_latest",
    "last_seq_scan_never_count",
    "last_idx_scan_oldest",
    "last_idx_scan_latest",
    "last_idx_scan_never_count",
];
const TABLE_CHANGE_FIELDS: &[&str] = &[
    "dml_total",
    "insert_share_pct",
    "update_share_pct",
    "delete_share_pct",
    "hot_pct",
    "new_page_pct",
    "dead_pct",
    "n_mod_since_analyze",
    "n_ins_since_vacuum",
];
const TABLE_MAINTENANCE_FIELDS: &[&str] = &[
    "vacuum_count",
    "autovacuum_count",
    "analyze_count",
    "autoanalyze_count",
    "last_vacuum",
    "last_autovacuum",
    "last_analyze",
    "last_autoanalyze",
    "toast_last_autovacuum",
    "vacuum_mean_ms",
    "autovacuum_mean_ms",
    "analyze_mean_ms",
    "autoanalyze_mean_ms",
];
const TABLE_MAINTENANCE_AGGREGATE_FIELDS: &[&str] = &[
    "vacuum_count",
    "autovacuum_count",
    "analyze_count",
    "autoanalyze_count",
    "last_vacuum_oldest",
    "last_vacuum_latest",
    "last_vacuum_never_count",
    "last_autovacuum_oldest",
    "last_autovacuum_latest",
    "last_autovacuum_never_count",
    "last_analyze_oldest",
    "last_analyze_latest",
    "last_analyze_never_count",
    "last_autoanalyze_oldest",
    "last_autoanalyze_latest",
    "last_autoanalyze_never_count",
    "toast_last_autovacuum_oldest",
    "toast_last_autovacuum_latest",
    "toast_last_autovacuum_never_count",
    "vacuum_mean_ms",
    "autovacuum_mean_ms",
    "analyze_mean_ms",
    "autoanalyze_mean_ms",
];
const TABLE_SIZE_FIELDS: &[&str] = &[
    "displayed_storage_bytes",
    "main_fork_bytes",
    "toast_bytes",
    "toast_share_pct",
    "reltuples",
    "toast_n_live_tup",
    "toast_n_dead_tup",
    "toast_dead_pct",
    "buffer_hit_pct",
    "heap_buffer_hit_pct",
    "index_buffer_hit_pct",
    "toast_buffer_hit_pct",
    "tidx_buffer_hit_pct",
    "heap_blks_read",
    "heap_blks_hit",
    "idx_blks_read",
    "idx_blks_hit",
    "toast_blks_read",
    "toast_blks_hit",
    "tidx_blks_read",
    "tidx_blks_hit",
];
const TABLE_FREEZE_FIELDS: &[&str] = &[
    "xid_age",
    "mxid_age",
    "n_ins_since_vacuum",
    "last_vacuum",
    "last_autovacuum",
];
const TABLE_FREEZE_AGGREGATE_FIELDS: &[&str] = &[
    "xid_age",
    "mxid_age",
    "n_ins_since_vacuum",
    "last_vacuum_oldest",
    "last_vacuum_latest",
    "last_vacuum_never_count",
    "last_autovacuum_oldest",
    "last_autovacuum_latest",
    "last_autovacuum_never_count",
];
const INDEX_FIELDS: &[&str] = &[
    "idx_scan",
    "idx_tup_read",
    "idx_tup_fetch",
    "tuples_per_scan",
    "fetches_per_scan",
    "last_idx_scan",
    "last_idx_scan_never",
];
const INDEX_USAGE_AGGREGATE_FIELDS: &[&str] = &[
    "idx_scan",
    "idx_tup_read",
    "idx_tup_fetch",
    "tuples_per_scan",
    "fetches_per_scan",
    "last_idx_scan_oldest",
    "last_idx_scan_latest",
    "last_idx_scan_never_count",
];
const INDEX_LOW_ACTIVITY_FIELDS: &[&str] = &[
    "no_scans",
    "idx_scan",
    "last_idx_scan",
    "last_idx_scan_never",
    "main_fork_bytes",
];
const INDEX_LOW_ACTIVITY_AGGREGATE_FIELDS: &[&str] = &[
    "no_scan_count",
    "known_scan_count",
    "idx_scan",
    "last_idx_scan_oldest",
    "last_idx_scan_latest",
    "last_idx_scan_never_count",
    "main_fork_bytes",
];
const INDEX_SIZE_FIELDS: &[&str] = &[
    "main_fork_bytes",
    "idx_blks_read",
    "idx_blks_hit",
    "buffer_hit_pct",
];
const INDEX_STATE_FIELDS: &[&str] = &[
    "state_severity",
    "indisvalid",
    "indisready",
    "indisunique",
    "indisprimary",
    "indisexclusion",
];
const INDEX_STATE_AGGREGATE_FIELDS: &[&str] = &[
    "state_severity",
    "invalid_count",
    "unready_count",
    "unique_count",
    "primary_count",
    "exclusion_count",
];
const STATEMENT_DIRECT_ORDERS: &[&str] = &[
    "shared_blks_hit",
    "shared_blks_read",
    "shared_blks_dirtied",
    "shared_blks_written",
    "local_blks_hit",
    "local_blks_read",
    "local_blks_dirtied",
    "local_blks_written",
    "temp_blks_read",
    "temp_blks_written",
    "plans",
    "wal_bytes",
    "wal_records",
    "wal_fpi",
    "wal_buffers_full",
    "stats_since",
    "queryid",
];
const PLAN_DIRECT_ORDERS: &[&str] = &[
    "shared_blks_hit",
    "shared_blks_read",
    "shared_blks_dirtied",
    "shared_blks_written",
    "local_blks_hit",
    "local_blks_read",
    "local_blks_dirtied",
    "local_blks_written",
    "temp_blks_read",
    "temp_blks_written",
    "slow_log_calls",
    "first_call",
    "last_call",
    "queryid",
    "queryid_stat_statements",
    "planid",
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
    type_ids: HashSet<u32>,
    warnings: Vec<Value>,
}

#[derive(Clone, Copy)]
struct DirectSpec {
    section: &'static str,
    key: &'static str,
    default_order: &'static str,
    search: bool,
    relation: bool,
    whole_set: bool,
}

#[derive(Debug)]
struct ResolvedLens {
    fields: &'static [&'static str],
    default_order: &'static str,
    low_activity: bool,
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
                default_order: "query_duration_ms",
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
                default_order: "execution_ms_per_second",
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
                default_order: "execution_ms_per_second",
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
                default_order: "xact_commit",
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
                default_order: "tuple_throughput",
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
                default_order: "idx_scan",
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
    let lens = lens(args, &spec, group)?;
    let anchor = resolve_anchor(state, at, &[spec.section], cancelled)?;
    let defaults = defaults_for_layouts(spec.section, lens.fields, &anchor.type_ids);
    let fields = fields(args, &defaults)?;
    let page_size = if spec.whole_set {
        MAX_ROWS
    } else {
        page_size(args)?
    };
    let direction = direction(args)?;
    let order = args
        .get("order")
        .map(|value| string(value, "order"))
        .transpose()?
        .unwrap_or(lens.default_order);
    let by = order_tokens(spec.section, order, group)?;
    let filters = if lens.low_activity {
        vec![Filter {
            column: "no_scans".to_owned(),
            value: "true".to_owned(),
        }]
    } else {
        Vec::new()
    };
    let request = SnapshotRequest {
        segment_id: anchor.segment_id,
        active_position: anchor.active_wal_position,
        at,
        sections: vec![spec.section.to_owned()],
        fields,
        by,
        direction,
        group,
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
        filters,
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
            "the recorded lock set exceeds the 500-row whole-set bound",
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
        summary: format!("Returned {returned} recorded PostgreSQL {} rows.", spec.key),
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

#[expect(
    clippy::too_many_lines,
    reason = "the four fixed accepted lens registries stay visibly exhaustive together"
)]
fn lens(
    args: &Map<String, Value>,
    spec: &DirectSpec,
    group: Option<RelationGroup>,
) -> Result<ResolvedLens, PostgresqlFailure> {
    let requested = args
        .get("lens")
        .map(|value| string(value, "lens"))
        .transpose()?;
    let aggregate = group.is_some_and(|group| group != RelationGroup::Object);
    let resolved = match (spec.section, requested) {
        ("pg_stat_statements", None | Some("load")) => {
            resolved(STATEMENT_FIELDS, "execution_ms_per_second")
        }
        ("pg_stat_statements", Some("per_call")) => {
            resolved(STATEMENT_PER_CALL_FIELDS, "calls_per_second")
        }
        ("pg_stat_statements", Some("io")) => resolved(STATEMENT_IO_FIELDS, "shared_blks_read"),
        ("pg_stat_statements", Some("resources")) => {
            resolved(STATEMENT_RESOURCE_FIELDS, "wal_bytes")
        }
        ("pg_stat_statements", Some("stability")) => {
            resolved(STATEMENT_STABILITY_FIELDS, "calls_per_second")
        }
        ("pg_store_plans", None | Some("load")) => resolved(PLAN_FIELDS, "execution_ms_per_second"),
        ("pg_store_plans", Some("timing")) => resolved(PLAN_TIMING_FIELDS, "calls_per_second"),
        ("pg_store_plans", Some("io")) => resolved(PLAN_IO_FIELDS, "shared_blks_read"),
        ("pg_store_plans", Some("identity")) => resolved(PLAN_IDENTITY_FIELDS, "calls_per_second"),
        ("pg_stat_user_tables", None | Some("access")) => resolved(
            if aggregate {
                TABLE_ACCESS_AGGREGATE_FIELDS
            } else {
                TABLE_FIELDS
            },
            "tuple_throughput",
        ),
        ("pg_stat_user_tables", Some("changes")) => resolved(TABLE_CHANGE_FIELDS, "dml_total"),
        ("pg_stat_user_tables", Some("maintenance")) => resolved(
            if aggregate {
                TABLE_MAINTENANCE_AGGREGATE_FIELDS
            } else {
                TABLE_MAINTENANCE_FIELDS
            },
            "autovacuum_count",
        ),
        ("pg_stat_user_tables", Some("size_buffers")) => {
            resolved(TABLE_SIZE_FIELDS, "displayed_storage_bytes")
        }
        ("pg_stat_user_tables", Some("freeze")) => resolved(
            if aggregate {
                TABLE_FREEZE_AGGREGATE_FIELDS
            } else {
                TABLE_FREEZE_FIELDS
            },
            "xid_age",
        ),
        ("pg_stat_user_indexes", None | Some("usage")) => resolved(
            if aggregate {
                INDEX_USAGE_AGGREGATE_FIELDS
            } else {
                INDEX_FIELDS
            },
            "idx_scan",
        ),
        ("pg_stat_user_indexes", Some("low_activity")) => ResolvedLens {
            fields: if aggregate {
                INDEX_LOW_ACTIVITY_AGGREGATE_FIELDS
            } else {
                INDEX_LOW_ACTIVITY_FIELDS
            },
            default_order: "main_fork_bytes",
            low_activity: true,
        },
        ("pg_stat_user_indexes", Some("size_buffers")) => {
            resolved(INDEX_SIZE_FIELDS, "main_fork_bytes")
        }
        ("pg_stat_user_indexes", Some("state")) => resolved(
            if aggregate {
                INDEX_STATE_AGGREGATE_FIELDS
            } else {
                INDEX_STATE_FIELDS
            },
            "state_severity",
        ),
        ("pg_stat_activity", None) => resolved(&[], spec.default_order),
        ("pg_stat_database", None) => resolved(DATABASE_FIELDS, spec.default_order),
        ("pg_stat_statements", Some(_)) => {
            return Err(input(
                "lens",
                "lens must be load, per_call, io, resources, or stability",
            ));
        }
        ("pg_store_plans", Some(_)) => {
            return Err(input("lens", "lens must be load, timing, io, or identity"));
        }
        ("pg_stat_user_tables", Some(_)) => {
            return Err(input(
                "lens",
                "lens must be access, changes, maintenance, size_buffers, or freeze",
            ));
        }
        ("pg_stat_user_indexes", Some(_)) => {
            return Err(input(
                "lens",
                "lens must be usage, low_activity, size_buffers, or state",
            ));
        }
        (_, Some(_)) => return Err(input("lens", "lens is not supported by this surface")),
        (_, None) => resolved(&[], spec.default_order),
    };
    Ok(resolved)
}

const fn resolved(fields: &'static [&'static str], default_order: &'static str) -> ResolvedLens {
    ResolvedLens {
        fields,
        default_order,
        low_activity: false,
    }
}

fn defaults_for_layouts<'a>(
    section: &str,
    defaults: &'a [&'a str],
    type_ids: &HashSet<u32>,
) -> Vec<&'a str> {
    if !matches!(section, "pg_stat_statements" | "pg_store_plans") || type_ids.is_empty() {
        return defaults.to_vec();
    }
    defaults
        .iter()
        .copied()
        .filter(|name| {
            type_ids
                .iter()
                .filter_map(|type_id| contract(*type_id))
                .any(|layout| layout.column(name).is_some())
        })
        .collect()
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

fn order_tokens(
    section: &str,
    requested: &str,
    group: Option<RelationGroup>,
) -> Result<Vec<String>, PostgresqlFailure> {
    if let Some(group) = group {
        if !api::relation_field_is_available(section, group, requested) {
            return Err(input(
                "order",
                format!("order {requested:?} is not accepted for the selected relation group"),
            ));
        }
        return Ok(vec![relation_order_token(requested)]);
    }
    direct_order_tokens(section, requested).ok_or_else(|| {
        input(
            "order",
            format!("order {requested:?} is not accepted for {section}"),
        )
    })
}

fn direct_order_tokens(section: &str, requested: &str) -> Option<Vec<String>> {
    let fixed = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    match (section, requested) {
        ("pg_stat_activity", "backend_age_ms") => Some(fixed(&["derived.backend_age_ms"])),
        ("pg_stat_activity", "query_duration_ms") => Some(fixed(&["derived.query_duration_ms"])),
        ("pg_stat_activity", "state_duration_ms") => Some(fixed(&["derived.state_duration_ms"])),
        ("pg_stat_activity", "transaction_duration_ms") => {
            Some(fixed(&["derived.transaction_duration_ms"]))
        }
        ("pg_stat_activity", name) if api::surface_field_is_public("pg_stat_activity", name) => {
            Some(vec![name.to_owned()])
        }
        ("pg_stat_statements" | "pg_store_plans", "calls_per_second") => Some(fixed(&["calls"])),
        ("pg_stat_statements", "execution_ms_per_second") => {
            Some(fixed(&["total_exec_time", "total_time"]))
        }
        ("pg_store_plans", "execution_ms_per_second") => Some(fixed(&["total_time"])),
        ("pg_stat_statements" | "pg_store_plans", "rows_per_second") => Some(fixed(&["rows"])),
        ("pg_stat_statements" | "pg_store_plans", "planning_ms_per_second") => {
            Some(fixed(&["total_plan_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "shared_blk_read_ms_per_second") => {
            Some(fixed(&["blk_read_time", "shared_blk_read_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "shared_blk_write_ms_per_second") => {
            Some(fixed(&["blk_write_time", "shared_blk_write_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "local_blk_read_ms_per_second") => {
            Some(fixed(&["local_blk_read_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "local_blk_write_ms_per_second") => {
            Some(fixed(&["local_blk_write_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "temp_blk_read_ms_per_second") => {
            Some(fixed(&["temp_blk_read_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "temp_blk_write_ms_per_second") => {
            Some(fixed(&["temp_blk_write_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "mean_exec_ms_per_call") => {
            Some(fixed(&["derived.mean_exec_ms_per_call"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "rows_per_call") => {
            Some(fixed(&["derived.rows_per_call"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "blocks_per_call") => {
            Some(fixed(&["derived.blocks_per_call"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "hit_pct") => Some(fixed(&["derived.hit_pct"])),
        ("pg_stat_statements", "wal_per_call") => Some(fixed(&["derived.wal_per_call"])),
        ("pg_stat_statements", "plan_time_pct") => Some(fixed(&["derived.plan_time_pct"])),
        ("pg_stat_statements" | "pg_store_plans", "cv") => Some(fixed(&["derived.cv"])),
        ("pg_stat_statements" | "pg_store_plans", "min_exec_time_ms") => {
            Some(fixed(&["min_exec_time", "min_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "max_exec_time_ms") => {
            Some(fixed(&["max_exec_time", "max_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "mean_exec_time_ms") => {
            Some(fixed(&["mean_exec_time", "mean_time"]))
        }
        ("pg_stat_statements" | "pg_store_plans", "stddev_exec_time_ms") => {
            Some(fixed(&["stddev_exec_time", "stddev_time"]))
        }
        ("pg_stat_statements", name) if STATEMENT_DIRECT_ORDERS.contains(&name) => {
            Some(vec![name.to_owned()])
        }
        ("pg_store_plans", name) if PLAN_DIRECT_ORDERS.contains(&name) => {
            Some(vec![name.to_owned()])
        }
        ("pg_stat_database", name) if DATABASE_FIELDS.contains(&name) => {
            Some(vec![name.to_owned()])
        }
        _ => None,
    }
}

fn relation_order_token(name: &str) -> String {
    if name.ends_with("_pct")
        || name.ends_with("_per_scan")
        || name.ends_with("_mean_ms")
        || matches!(
            name,
            "state_severity" | "tuple_throughput" | "dml_total" | "displayed_storage_bytes"
        )
    {
        format!("derived.{name}")
    } else {
        name.to_owned()
    }
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
        summary: format!("Returned {returned} recorded PostgreSQL overview rows."),
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
                let type_ids = record
                    .get("sections")
                    .and_then(Value::as_array)
                    .map(|stored| {
                        stored
                            .iter()
                            .filter(|section| {
                                section
                                    .get("logical_name")
                                    .and_then(Value::as_str)
                                    .is_some_and(|name| wanted.contains(name))
                            })
                            .filter_map(|section| {
                                section
                                    .get("type_id")
                                    .and_then(Value::as_str)
                                    .and_then(|value| value.parse::<u32>().ok())
                            })
                            .collect::<HashSet<_>>()
                    })
                    .unwrap_or_default();
                any.push((id, wal, type_ids.clone()));
                if !type_ids.is_empty() {
                    matching.push((id, wal, type_ids));
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
        .max_by_key(|(id, _, _)| *id)
        .or_else(|| any.into_iter().max_by_key(|(id, _, _)| *id))
        .ok_or_else(|| {
            failure(
                "no_recorded_data",
                "no recorded segment exists at the requested time",
                Some("at_us"),
            )
        })?;
    Ok(Anchor {
        segment_id: selected.0,
        active_wal_position: selected.1,
        type_ids: selected.2,
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
                "source_provenance_unusable",
                "the recorded snapshot has no physical source prefix",
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
                "source_provenance_unusable",
                "the recorded snapshot has an invalid physical source prefix",
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
    PostgresqlFailure {
        code: error.code(),
        message: error.to_string(),
        parameter: parameter.map(str::to_owned),
        retryable,
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
