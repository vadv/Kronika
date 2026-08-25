//! Public product-surface projections shared by HTTP and MCP.

use std::collections::HashSet;

use kronika_registry::contract;

use super::ApiError;
use crate::route::{
    Filter, IndexLens, Order, PlanLens, PostgresqlSurface, ProcessLens, RelationGroup,
    SnapshotRequest, StatementLens, TableLens,
};

pub(crate) const PROCESS_PAGE_SIZE: usize = 200;

pub(crate) fn resolve_process_surface(
    request: &mut SnapshotRequest,
) -> Result<ProcessLens, ApiError> {
    let Some(product) = request.process.as_ref() else {
        return Err(ApiError::NoSuchSection);
    };
    if request.sections.as_slice() != ["os_process"] {
        return Err(ApiError::NoSuchSection);
    }
    let lens = product.lens;
    if lens == ProcessLens::Tree {
        return Ok(lens);
    }
    let default_order = match lens {
        ProcessLens::Generic => "pid",
        ProcessLens::Cpu => "utime",
        ProcessLens::Memory => "rmem_kb",
        ProcessLens::Disk => "read_bytes",
        ProcessLens::Tree => unreachable!(),
    };
    let order = product.order.as_deref().unwrap_or(default_order);
    request.by = process_order_tokens(lens, order)
        .ok_or_else(|| ApiError::NoSuchColumn(order.to_owned()))?;
    request.direction = product
        .direction
        .unwrap_or(if lens == ProcessLens::Generic {
            Order::Asc
        } else {
            Order::Desc
        });
    request.page_size.get_or_insert(PROCESS_PAGE_SIZE);
    Ok(lens)
}

fn process_order_tokens(lens: ProcessLens, requested: &str) -> Option<Vec<String>> {
    const GENERIC: &[&str] = &[
        "pid",
        "command",
        "ppid",
        "gid",
        "egid",
        "num_threads",
        "tty",
        "exit_signal",
        "state",
    ];
    const CPU: &[&str] = &[
        "pid",
        "command",
        "utime",
        "stime",
        "rundelay_ns",
        "blkdelay_ticks",
        "nvcsw",
        "nivcsw",
        "curcpu",
        "nice",
        "prio",
        "rtprio",
        "policy",
        "state",
    ];
    const MEMORY: &[&str] = &[
        "pid", "command", "rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt", "state",
    ];
    const DISK: &[&str] = &[
        "pid",
        "command",
        "read_bytes",
        "write_bytes",
        "syscr",
        "syscw",
        "rchar",
        "wchar",
        "cancelled_write_bytes",
        "blkdelay_ticks",
        "state",
    ];
    let accepted = match lens {
        ProcessLens::Generic => GENERIC,
        ProcessLens::Cpu => CPU,
        ProcessLens::Memory => MEMORY,
        ProcessLens::Disk => DISK,
        ProcessLens::Tree => return None,
    };
    accepted.contains(&requested).then(|| {
        if requested == "command" {
            vec!["cmdline".to_owned(), "comm".to_owned()]
        } else {
            vec![requested.to_owned()]
        }
    })
}

const ACTIVITY_DEFAULT_FIELDS: &[&str] = &[
    "pid",
    "datid",
    "datname",
    "usename",
    "application_name",
    "client_addr",
    "backend_type",
    "state",
    "wait_event_type",
    "wait_event",
    "query",
    "query_id",
    "backend_xid_age",
    "backend_xmin_age",
    "backend_start",
    "xact_start",
    "query_start",
    "state_change",
];

const LOCK_DEFAULT_FIELDS: &[&str] = &[
    "pid",
    "blocked_by",
    "datid",
    "datname",
    "usename",
    "application_name",
    "backend_type",
    "state",
    "wait_event_type",
    "wait_event",
    "query",
    "lock_locktype",
    "lock_mode",
    "lock_database",
    "lock_relation",
    "lock_relname",
    "lock_page",
    "lock_tuple",
    "lock_virtualxid",
    "lock_transactionid",
    "lock_classid",
    "lock_objid",
    "lock_objsubid",
    "lock_target",
    "waitstart",
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PostgresqlSurfaceSpec {
    pub(crate) fields: &'static [&'static str],
    pub(crate) default_order: &'static str,
    pub(crate) low_activity: bool,
}

pub(crate) fn postgresql_surface(
    surface: PostgresqlSurface,
    group: Option<RelationGroup>,
) -> PostgresqlSurfaceSpec {
    let aggregate = group.is_some_and(|group| group != RelationGroup::Object);
    match surface {
        PostgresqlSurface::Statements(StatementLens::Load) => {
            spec(STATEMENT_FIELDS, "execution_ms_per_second")
        }
        PostgresqlSurface::Statements(StatementLens::PerCall) => {
            spec(STATEMENT_PER_CALL_FIELDS, "calls_per_second")
        }
        PostgresqlSurface::Statements(StatementLens::Io) => {
            spec(STATEMENT_IO_FIELDS, "shared_blks_read")
        }
        PostgresqlSurface::Statements(StatementLens::Resources) => {
            spec(STATEMENT_RESOURCE_FIELDS, "wal_bytes")
        }
        PostgresqlSurface::Statements(StatementLens::Stability) => {
            spec(STATEMENT_STABILITY_FIELDS, "calls_per_second")
        }
        PostgresqlSurface::Plans(PlanLens::Load) => spec(PLAN_FIELDS, "execution_ms_per_second"),
        PostgresqlSurface::Plans(PlanLens::Timing) => spec(PLAN_TIMING_FIELDS, "calls_per_second"),
        PostgresqlSurface::Plans(PlanLens::Io) => spec(PLAN_IO_FIELDS, "shared_blks_read"),
        PostgresqlSurface::Plans(PlanLens::Identity) => {
            spec(PLAN_IDENTITY_FIELDS, "calls_per_second")
        }
        PostgresqlSurface::Tables(TableLens::Access) => spec(
            if aggregate {
                TABLE_ACCESS_AGGREGATE_FIELDS
            } else {
                TABLE_FIELDS
            },
            "tuple_throughput",
        ),
        PostgresqlSurface::Tables(TableLens::Changes) => spec(TABLE_CHANGE_FIELDS, "dml_total"),
        PostgresqlSurface::Tables(TableLens::Maintenance) => spec(
            if aggregate {
                TABLE_MAINTENANCE_AGGREGATE_FIELDS
            } else {
                TABLE_MAINTENANCE_FIELDS
            },
            "autovacuum_count",
        ),
        PostgresqlSurface::Tables(TableLens::SizeBuffers) => {
            spec(TABLE_SIZE_FIELDS, "displayed_storage_bytes")
        }
        PostgresqlSurface::Tables(TableLens::Freeze) => spec(
            if aggregate {
                TABLE_FREEZE_AGGREGATE_FIELDS
            } else {
                TABLE_FREEZE_FIELDS
            },
            "xid_age",
        ),
        PostgresqlSurface::Indexes(IndexLens::Usage) => spec(
            if aggregate {
                INDEX_USAGE_AGGREGATE_FIELDS
            } else {
                INDEX_FIELDS
            },
            "idx_scan",
        ),
        PostgresqlSurface::Indexes(IndexLens::LowActivity) => PostgresqlSurfaceSpec {
            fields: if aggregate {
                INDEX_LOW_ACTIVITY_AGGREGATE_FIELDS
            } else {
                INDEX_LOW_ACTIVITY_FIELDS
            },
            default_order: "main_fork_bytes",
            low_activity: true,
        },
        PostgresqlSurface::Indexes(IndexLens::SizeBuffers) => {
            spec(INDEX_SIZE_FIELDS, "main_fork_bytes")
        }
        PostgresqlSurface::Indexes(IndexLens::State) => spec(
            if aggregate {
                INDEX_STATE_AGGREGATE_FIELDS
            } else {
                INDEX_STATE_FIELDS
            },
            "state_severity",
        ),
        PostgresqlSurface::Activity => spec(&[], "query_duration_ms"),
        PostgresqlSurface::Locks => spec(LOCK_DEFAULT_FIELDS, "pid"),
        PostgresqlSurface::Databases => spec(DATABASE_FIELDS, "xact_commit"),
    }
}

pub(crate) fn postgresql_fields_for_layouts<'a>(
    surface: PostgresqlSurface,
    defaults: &'a [&'a str],
    type_ids: &HashSet<u32>,
) -> Vec<&'a str> {
    if matches!(
        surface,
        PostgresqlSurface::Activity
            | PostgresqlSurface::Locks
            | PostgresqlSurface::Tables(_)
            | PostgresqlSurface::Indexes(_)
    ) || type_ids.is_empty()
    {
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

pub(crate) fn postgresql_order_tokens(
    surface: PostgresqlSurface,
    requested: &str,
    group: Option<RelationGroup>,
) -> Option<Vec<String>> {
    let section = surface.section();
    if let Some(group) = group {
        return super::snapshot::relation_field_is_available(section, group, requested)
            .then(|| vec![relation_order_token(requested)]);
    }
    direct_order_tokens(section, requested)
}

pub(crate) fn resolve_postgresql_surface(
    request: &mut SnapshotRequest,
    type_ids: &HashSet<u32>,
) -> Result<(), ApiError> {
    let Some(product) = request.postgresql.as_ref() else {
        return Ok(());
    };
    let surface = product.surface;
    let requested_order = product.order.clone();
    if request.sections.as_slice() != [surface.section()] {
        return Err(ApiError::NoSuchSection);
    }
    let resolved = postgresql_surface(surface, request.group);
    if request.fields.is_empty() {
        request.fields = postgresql_fields_for_layouts(surface, resolved.fields, type_ids)
            .into_iter()
            .map(str::to_owned)
            .collect();
    }
    if request.by.is_empty() {
        let order = requested_order.as_deref().unwrap_or(resolved.default_order);
        request.by = postgresql_order_tokens(surface, order, request.group)
            .ok_or_else(|| ApiError::NoSuchColumn(order.to_owned()))?;
    }
    if resolved.low_activity
        && !request
            .filters
            .iter()
            .any(|filter| filter.column == "no_scans")
    {
        request.filters.push(Filter {
            column: "no_scans".to_owned(),
            value: "true".to_owned(),
        });
    }
    Ok(())
}

const fn spec(
    fields: &'static [&'static str],
    default_order: &'static str,
) -> PostgresqlSurfaceSpec {
    PostgresqlSurfaceSpec {
        fields,
        default_order,
        low_activity: false,
    }
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
        ("pg_stat_activity", name) if field_is_public("pg_stat_activity", name) => {
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

pub(super) const fn default_fields(logical_name: &str) -> Option<&'static [&'static str]> {
    match logical_name.as_bytes() {
        b"pg_stat_activity" => Some(ACTIVITY_DEFAULT_FIELDS),
        b"pg_locks" => Some(LOCK_DEFAULT_FIELDS),
        _ => None,
    }
}

fn field_is_public(logical_name: &str, name: &str) -> bool {
    match logical_name {
        "pg_stat_activity" => name == "leader_pid" || ACTIVITY_DEFAULT_FIELDS.contains(&name),
        "pg_locks" => LOCK_DEFAULT_FIELDS.contains(&name),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use kronika_registry::{PgStatStatementsV1, PgStorePlansOsscV1, Section as _};

    use super::{
        postgresql_fields_for_layouts, postgresql_order_tokens, postgresql_surface,
        resolve_process_surface,
    };
    use crate::route::{Order, PostgresqlSurface, ProcessLens, RelationGroup, Route, parse};

    #[test]
    fn process_lenses_share_web_defaults_and_semantic_command_order() {
        for (lens, expected_lens, expected_order, expected_direction) in [
            (
                "generic",
                ProcessLens::Generic,
                ["pid"].as_slice(),
                Order::Asc,
            ),
            ("cpu", ProcessLens::Cpu, ["utime"].as_slice(), Order::Desc),
            (
                "memory",
                ProcessLens::Memory,
                ["rmem_kb"].as_slice(),
                Order::Desc,
            ),
            (
                "disk",
                ProcessLens::Disk,
                ["read_bytes"].as_slice(),
                Order::Desc,
            ),
        ] {
            let Route::Snapshot(mut request) = parse(
                "/api/segments/7/snapshot",
                Some(&format!("at=9&section=os_process&lens={lens}")),
            )
            .expect("Process route") else {
                panic!("snapshot route");
            };
            assert_eq!(
                resolve_process_surface(&mut request).expect("resolve Process surface"),
                expected_lens
            );
            assert_eq!(request.by, expected_order, "{lens}");
            assert_eq!(request.direction, expected_direction, "{lens}");
            assert_eq!(request.page_size, Some(200), "{lens}");
        }

        let Route::Snapshot(mut request) = parse(
            "/api/segments/7/snapshot",
            Some("at=9&section=os_process&lens=generic&order=command"),
        )
        .expect("command order") else {
            panic!("snapshot route");
        };
        assert_eq!(
            resolve_process_surface(&mut request).expect("resolve Process surface"),
            ProcessLens::Generic
        );
        assert_eq!(request.by, ["cmdline", "comm"]);
    }

    #[test]
    fn every_postgresql_lens_keeps_its_projection_and_default_order() {
        let direct = [
            (
                "pg_stat_statements",
                "load",
                "execution_ms_per_second",
                false,
                11,
            ),
            (
                "pg_stat_statements",
                "per_call",
                "calls_per_second",
                false,
                15,
            ),
            ("pg_stat_statements", "io", "shared_blks_read", false, 16),
            ("pg_stat_statements", "resources", "wal_bytes", false, 16),
            (
                "pg_stat_statements",
                "stability",
                "calls_per_second",
                false,
                16,
            ),
            (
                "pg_store_plans",
                "load",
                "execution_ms_per_second",
                false,
                11,
            ),
            ("pg_store_plans", "timing", "calls_per_second", false, 15),
            ("pg_store_plans", "io", "shared_blks_read", false, 15),
            ("pg_store_plans", "identity", "calls_per_second", false, 11),
            (
                "pg_stat_user_tables",
                "access",
                "tuple_throughput",
                false,
                10,
            ),
            ("pg_stat_user_tables", "changes", "dml_total", false, 9),
            (
                "pg_stat_user_tables",
                "maintenance",
                "autovacuum_count",
                false,
                13,
            ),
            (
                "pg_stat_user_tables",
                "size_buffers",
                "displayed_storage_bytes",
                false,
                21,
            ),
            ("pg_stat_user_tables", "freeze", "xid_age", false, 5),
            ("pg_stat_user_indexes", "usage", "idx_scan", false, 7),
            (
                "pg_stat_user_indexes",
                "low_activity",
                "main_fork_bytes",
                true,
                5,
            ),
            (
                "pg_stat_user_indexes",
                "size_buffers",
                "main_fork_bytes",
                false,
                4,
            ),
            ("pg_stat_user_indexes", "state", "state_severity", false, 6),
        ];
        for (section, requested, expected_order, expected_low_activity, expected_fields) in direct {
            let surface = PostgresqlSurface::parse(section, Some(requested))
                .unwrap_or_else(|| panic!("{section} {requested}"));
            let lens = postgresql_surface(surface, Some(RelationGroup::Object));
            assert_eq!(lens.default_order, expected_order, "{section} {requested}");
            assert_eq!(
                lens.low_activity, expected_low_activity,
                "{section} {requested}"
            );
            assert_eq!(lens.fields.len(), expected_fields, "{section} {requested}");
        }

        for (section, default) in [
            ("pg_stat_statements", "load"),
            ("pg_store_plans", "load"),
            ("pg_stat_user_tables", "access"),
            ("pg_stat_user_indexes", "usage"),
        ] {
            let surface = PostgresqlSurface::parse(section, None).expect("default surface");
            let explicit = PostgresqlSurface::parse(section, Some(default)).expect("lens surface");
            assert_eq!(
                postgresql_surface(surface, Some(RelationGroup::Object)),
                postgresql_surface(explicit, Some(RelationGroup::Object)),
                "{section} default lens",
            );
        }
    }

    #[test]
    fn grouped_postgresql_lenses_use_relation_registry_fields_and_orders() {
        for (section, lenses) in [
            (
                "pg_stat_user_tables",
                ["access", "changes", "maintenance", "size_buffers", "freeze"].as_slice(),
            ),
            (
                "pg_stat_user_indexes",
                ["usage", "low_activity", "size_buffers", "state"].as_slice(),
            ),
        ] {
            for group in [
                RelationGroup::Database,
                RelationGroup::Schema,
                RelationGroup::Tablespace,
            ] {
                for requested in lenses {
                    let surface =
                        PostgresqlSurface::parse(section, Some(requested)).expect("grouped lens");
                    let lens = postgresql_surface(surface, Some(group));
                    assert!(lens.fields.iter().all(|field| {
                        super::super::snapshot::relation_field_is_available(section, group, field)
                    }));
                    assert!(super::super::snapshot::relation_field_is_available(
                        section,
                        group,
                        lens.default_order,
                    ));
                }
            }
        }
    }

    #[test]
    fn postgresql_order_tokens_keep_physical_fallbacks_and_derived_names() {
        for (section, requested, expected) in [
            (
                "pg_stat_statements",
                "execution_ms_per_second",
                ["total_exec_time", "total_time"].as_slice(),
            ),
            (
                "pg_store_plans",
                "execution_ms_per_second",
                ["total_time"].as_slice(),
            ),
            (
                "pg_stat_statements",
                "shared_blk_read_ms_per_second",
                ["blk_read_time", "shared_blk_read_time"].as_slice(),
            ),
            (
                "pg_store_plans",
                "mean_exec_ms_per_call",
                ["derived.mean_exec_ms_per_call"].as_slice(),
            ),
            (
                "pg_stat_database",
                "xact_commit",
                ["xact_commit"].as_slice(),
            ),
        ] {
            let surface = PostgresqlSurface::parse(section, None).expect("direct surface");
            let actual = postgresql_order_tokens(surface, requested, None)
                .unwrap_or_else(|| panic!("{section} {requested}"));
            assert_eq!(actual, expected, "{section} {requested}");
        }

        assert_eq!(
            postgresql_order_tokens(
                PostgresqlSurface::parse("pg_stat_user_tables", None).expect("Table surface"),
                "dead_pct",
                Some(RelationGroup::Object),
            ),
            Some(vec!["derived.dead_pct".to_owned()])
        );
        assert_eq!(
            postgresql_order_tokens(
                PostgresqlSurface::parse("pg_stat_user_tables", None).expect("Table surface"),
                "last_vacuum_oldest",
                Some(RelationGroup::Database),
            ),
            Some(vec!["last_vacuum_oldest".to_owned()])
        );
        assert!(
            postgresql_order_tokens(
                PostgresqlSurface::parse("pg_stat_user_tables", None).expect("Table surface"),
                "last_vacuum",
                Some(RelationGroup::Database),
            )
            .is_none()
        );
        assert!(postgresql_order_tokens(PostgresqlSurface::Databases, "made_up", None).is_none());
    }

    #[test]
    fn legacy_statement_and_plan_layouts_filter_only_unavailable_lens_fields() {
        for (section, requested, contract, missing) in [
            (
                "pg_stat_statements",
                "load",
                &PgStatStatementsV1::CONTRACT,
                ["toplevel", "total_exec_time"].as_slice(),
            ),
            (
                "pg_store_plans",
                "identity",
                &PgStorePlansOsscV1::CONTRACT,
                ["queryid_stat_statements", "cmd_type", "relids"].as_slice(),
            ),
        ] {
            let surface = PostgresqlSurface::parse(section, Some(requested)).expect("legacy lens");
            let lens = postgresql_surface(surface, None);
            let type_ids = HashSet::from([contract.type_id.get()]);
            let filtered = postgresql_fields_for_layouts(surface, lens.fields, &type_ids);
            assert!(!filtered.is_empty());
            assert!(
                filtered
                    .iter()
                    .all(|field| contract.column(field).is_some())
            );
            assert!(missing.iter().all(|field| !filtered.contains(field)));
        }
    }
}
