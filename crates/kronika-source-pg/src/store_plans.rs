//! `pg_store_plans` collection for types `1_003_001` and `1_004_001`.
//!
//! Several extensions share a name and two query interfaces. The OSSC and
//! Datasentinel-compatible interface carries plan text in its zero-argument
//! result. The vadv interface carries an internal query id plus a separate
//! `pg_stat_statements` id and exposes plan text through a four-key function.
//!
//! Rows are streamed in bounded collector batches. PostgreSQL truncates each
//! plan text before it crosses the connection.

use kronika_registry::pg_store_plans::{PgStorePlansOsscV1, PgStorePlansVadvV1};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::Session;
use crate::extension::{ExtensionSchema, InventoryEntry};
use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};

/// Prefix a query literal with the kronika marker (SQL-transparency rule).
macro_rules! marked {
    ($sql:literal) => {
        concat!(
            "/* kronika:",
            env!("CARGO_PKG_VERSION"),
            " crates/kronika-source-pg/src/store_plans.rs */ ",
            $sql,
        )
    };
}

/// The extension name to look for.
pub const EXTENSION: &str = "pg_store_plans";

/// Which `pg_store_plans` is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// The OSSC or Datasentinel interface with an inline plan column.
    OsscCompatible,
    /// The vadv interface with two query ids and a keyed plan getter.
    Vadv,
}

/// A discovered, usable `pg_store_plans` interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePlansCapability {
    /// Catalog-selected interface semantics.
    pub flavour: Flavour,
    /// Schema containing the discovered functions.
    pub schema: ExtensionSchema,
    /// Whether the current role belongs to `pg_read_all_stats`.
    pub full_visibility: bool,
}

/// Select the SQL interface from catalog capabilities, never from extversion.
#[must_use]
pub fn capability(entry: &InventoryEntry) -> Option<StorePlansCapability> {
    if entry.name != EXTENSION || !entry.schema_usable || !entry.full_visibility {
        return None;
    }
    let flavour = if entry.store_plans_bool_arg
        && entry.store_plans_key_getter
        && entry.store_plans_vadv_columns
    {
        Flavour::Vadv
    } else if entry.store_plans_zero_arg && entry.store_plans_ossc_columns {
        Flavour::OsscCompatible
    } else {
        return None;
    };
    Some(StorePlansCapability {
        flavour,
        schema: entry.schema.clone(),
        full_visibility: entry.full_visibility,
    })
}

/// The statistics query for one discovered interface.
#[must_use]
pub fn store_plans_query(capability: &StorePlansCapability) -> String {
    let (columns, source, plan) = match capability.flavour {
        Flavour::OsscCompatible => (
            "s.queryid, s.planid, s.userid, s.dbid, left(s.plan, 65536) AS plan, \
             s.calls, s.total_time, s.min_time, s.max_time, s.mean_time, s.stddev_time, s.rows, \
             s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written, \
             s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written, \
             s.temp_blks_read, s.temp_blks_written, \
             s.shared_blk_read_time, s.shared_blk_write_time, \
             s.local_blk_read_time, s.local_blk_write_time, \
             s.temp_blk_read_time, s.temp_blk_write_time, \
             s.first_call, s.last_call",
            format!("{}()", capability.schema.qualify(EXTENSION)),
            String::new(),
        ),
        Flavour::Vadv => (
            "s.userid, s.dbid, s.queryid, s.planid, s.queryid_stat_statements, \
             s.calls, s.slow_log_calls, \
             s.total_time, s.min_time, s.max_time, s.mean_time, s.stddev_time, s.rows, \
             s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written, \
             s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written, \
             s.temp_blks_read, s.temp_blks_written, \
             s.blk_read_time, s.blk_write_time, \
             s.first_call, s.last_call, \
             s.total_plan_time, s.min_plan_time, s.max_plan_time, s.mean_plan_time",
            format!("{}(false)", capability.schema.qualify(EXTENSION)),
            format!(
                ", left({}(s.userid, s.dbid, s.queryid, s.planid), 65536) AS plan",
                capability.schema.qualify("pg_store_plans_get_plan")
            ),
        ),
    };
    format!(
        "{}{columns}{plan}, \
         d.datname::text AS datname, r.rolname::text AS usename, \
         (extract(epoch from s.first_call) * 1e6)::int8 AS first_call_us, \
         (extract(epoch from s.last_call) * 1e6)::int8 AS last_call_us, \
         (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
         FROM {source} s \
         LEFT JOIN pg_catalog.pg_database d ON d.oid = s.dbid \
         LEFT JOIN pg_catalog.pg_roles r ON r.oid = s.userid",
        marked!("SELECT ")
    )
}

/// One raw ossc row.
///
/// See [`PgStorePlansOsscV1`] for column meanings.
#[derive(Debug, Clone)]
pub struct OsscRow {
    /// Collection time, unix microseconds.
    pub ts: i64,
    /// Core query id of the statement this plan belongs to.
    pub queryid: i64,
    /// Plan id.
    pub planid: i64,
    /// Role oid.
    pub userid: u32,
    /// Database oid.
    pub dbid: u32,
    /// Database name resolved from `dbid`.
    pub datname: Option<String>,
    /// Role name resolved from `userid`.
    pub usename: Option<String>,
    /// Server-truncated plan text.
    pub plan: Option<String>,
    /// Executions.
    pub calls: i64,
    /// Total execution time, ms.
    pub total_time: f64,
    /// Minimum execution time, ms.
    pub min_time: f64,
    /// Maximum execution time, ms.
    pub max_time: f64,
    /// Mean execution time, ms.
    pub mean_time: f64,
    /// Standard deviation of execution time, ms.
    pub stddev_time: f64,
    /// Rows retrieved or affected.
    pub rows: i64,
    /// Shared-block buffer hits.
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    pub local_blks_hit: i64,
    /// Local blocks read.
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    pub local_blks_written: i64,
    /// Temp blocks read.
    pub temp_blks_read: i64,
    /// Temp blocks written.
    pub temp_blks_written: i64,
    /// Time reading shared blocks, ms.
    pub shared_blk_read_time: f64,
    /// Time writing shared blocks, ms.
    pub shared_blk_write_time: f64,
    /// Time reading local blocks, ms.
    pub local_blk_read_time: f64,
    /// Time writing local blocks, ms.
    pub local_blk_write_time: f64,
    /// Time reading temp blocks, ms.
    pub temp_blk_read_time: f64,
    /// Time writing temp blocks, ms.
    pub temp_blk_write_time: f64,
    /// First execution of this entry, unix microseconds.
    pub first_call: i64,
    /// Last execution of this entry, unix microseconds.
    pub last_call: i64,
}

/// One raw vadv-fork row.
///
/// See [`PgStorePlansVadvV1`] for column meanings.
#[derive(Debug, Clone)]
pub struct VadvRow {
    /// Collection time, unix microseconds.
    pub ts: i64,
    /// Role oid.
    pub userid: u32,
    /// Database oid.
    pub dbid: u32,
    /// Internal query id, part of the vadv entry key.
    pub queryid: i64,
    /// Plan id.
    pub planid: i64,
    /// Query id of the last statement that ran this plan.
    pub queryid_stat_statements: i64,
    /// Database name resolved from `dbid`.
    pub datname: Option<String>,
    /// Role name resolved from `userid`.
    pub usename: Option<String>,
    /// Server-truncated plan text.
    pub plan: Option<String>,
    /// Executions.
    pub calls: i64,
    /// Executions recorded as slow statements.
    pub slow_log_calls: i64,
    /// Total execution time, ms.
    pub total_time: f64,
    /// Minimum execution time, ms.
    pub min_time: f64,
    /// Maximum execution time, ms.
    pub max_time: f64,
    /// Mean execution time, ms.
    pub mean_time: f64,
    /// Standard deviation of execution time, ms.
    pub stddev_time: f64,
    /// Rows retrieved or affected.
    pub rows: i64,
    /// Shared-block buffer hits.
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    pub local_blks_hit: i64,
    /// Local blocks read.
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    pub local_blks_written: i64,
    /// Temp blocks read.
    pub temp_blks_read: i64,
    /// Temp blocks written.
    pub temp_blks_written: i64,
    /// Time reading blocks, ms.
    pub blk_read_time: f64,
    /// Time writing blocks, ms.
    pub blk_write_time: f64,
    /// First execution of this entry, unix microseconds.
    pub first_call: i64,
    /// Last execution of this entry, unix microseconds.
    pub last_call: i64,
    /// Total planning time, ms.
    pub total_plan_time: f64,
    /// Minimum planning time, ms.
    pub min_plan_time: f64,
    /// Maximum planning time, ms.
    pub max_plan_time: f64,
    /// Mean planning time, ms.
    pub mean_plan_time: f64,
}

/// Intern an optional string, preserving `None`.
fn opt<E>(
    intern: &mut impl FnMut(&[u8]) -> Result<StrId, E>,
    value: Option<&str>,
) -> Result<Option<StrId>, E> {
    match value {
        Some(text) => Ok(Some(intern(text.as_bytes())?)),
        None => Ok(None),
    }
}

/// Build a `1_003_001` row (ossc upstream).
///
/// # Errors
/// Returns the interner's error.
pub fn to_ossc<E>(
    row: &OsscRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStorePlansOsscV1, E> {
    Ok(PgStorePlansOsscV1 {
        ts: Ts(row.ts),
        queryid: row.queryid,
        planid: row.planid,
        userid: row.userid,
        dbid: row.dbid,
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        plan: opt(&mut intern, row.plan.as_deref())?,
        calls: row.calls,
        total_time: row.total_time,
        min_time: row.min_time,
        max_time: row.max_time,
        mean_time: row.mean_time,
        stddev_time: row.stddev_time,
        rows: row.rows,
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        shared_blk_read_time: row.shared_blk_read_time,
        shared_blk_write_time: row.shared_blk_write_time,
        local_blk_read_time: row.local_blk_read_time,
        local_blk_write_time: row.local_blk_write_time,
        temp_blk_read_time: row.temp_blk_read_time,
        temp_blk_write_time: row.temp_blk_write_time,
        first_call: Ts(row.first_call),
        last_call: Ts(row.last_call),
    })
}

/// Build a `1_004_001` row (vadv fork).
///
/// # Errors
/// Returns the interner's error.
pub fn to_vadv<E>(
    row: &VadvRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStorePlansVadvV1, E> {
    // The registry/schema commit adds the internal key field. Keeping both ids
    // in the source row makes that integration a local mapping-only change.
    Ok(PgStorePlansVadvV1 {
        ts: Ts(row.ts),
        userid: row.userid,
        dbid: row.dbid,
        queryid: row.queryid,
        planid: row.planid,
        queryid_stat_statements: row.queryid_stat_statements,
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        plan: opt(&mut intern, row.plan.as_deref())?,
        calls: row.calls,
        slow_log_calls: row.slow_log_calls,
        total_time: row.total_time,
        min_time: row.min_time,
        max_time: row.max_time,
        mean_time: row.mean_time,
        stddev_time: row.stddev_time,
        rows: row.rows,
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        blk_read_time: row.blk_read_time,
        blk_write_time: row.blk_write_time,
        first_call: Ts(row.first_call),
        last_call: Ts(row.last_call),
        total_plan_time: row.total_plan_time,
        min_plan_time: row.min_plan_time,
        max_plan_time: row.max_plan_time,
        mean_plan_time: row.mean_plan_time,
    })
}

fn ossc_row_from_pg(row: &tokio_postgres::Row) -> OsscRow {
    OsscRow {
        ts: row.get("ts_us"),
        queryid: row.get("queryid"),
        planid: row.get("planid"),
        userid: row.get("userid"),
        dbid: row.get("dbid"),
        datname: row.get("datname"),
        usename: row.get("usename"),
        plan: row.get("plan"),
        calls: row.get("calls"),
        total_time: row.get("total_time"),
        min_time: row.get("min_time"),
        max_time: row.get("max_time"),
        mean_time: row.get("mean_time"),
        stddev_time: row.get("stddev_time"),
        rows: row.get("rows"),
        shared_blks_hit: row.get("shared_blks_hit"),
        shared_blks_read: row.get("shared_blks_read"),
        shared_blks_dirtied: row.get("shared_blks_dirtied"),
        shared_blks_written: row.get("shared_blks_written"),
        local_blks_hit: row.get("local_blks_hit"),
        local_blks_read: row.get("local_blks_read"),
        local_blks_dirtied: row.get("local_blks_dirtied"),
        local_blks_written: row.get("local_blks_written"),
        temp_blks_read: row.get("temp_blks_read"),
        temp_blks_written: row.get("temp_blks_written"),
        shared_blk_read_time: row.get("shared_blk_read_time"),
        shared_blk_write_time: row.get("shared_blk_write_time"),
        local_blk_read_time: row.get("local_blk_read_time"),
        local_blk_write_time: row.get("local_blk_write_time"),
        temp_blk_read_time: row.get("temp_blk_read_time"),
        temp_blk_write_time: row.get("temp_blk_write_time"),
        first_call: row.get("first_call_us"),
        last_call: row.get("last_call_us"),
    }
}

fn vadv_row_from_pg(row: &tokio_postgres::Row) -> VadvRow {
    VadvRow {
        ts: row.get("ts_us"),
        userid: row.get("userid"),
        dbid: row.get("dbid"),
        queryid: row.get("queryid"),
        planid: row.get("planid"),
        queryid_stat_statements: row.get("queryid_stat_statements"),
        datname: row.get("datname"),
        usename: row.get("usename"),
        plan: row.get("plan"),
        calls: row.get("calls"),
        slow_log_calls: row.get("slow_log_calls"),
        total_time: row.get("total_time"),
        min_time: row.get("min_time"),
        max_time: row.get("max_time"),
        mean_time: row.get("mean_time"),
        stddev_time: row.get("stddev_time"),
        rows: row.get("rows"),
        shared_blks_hit: row.get("shared_blks_hit"),
        shared_blks_read: row.get("shared_blks_read"),
        shared_blks_dirtied: row.get("shared_blks_dirtied"),
        shared_blks_written: row.get("shared_blks_written"),
        local_blks_hit: row.get("local_blks_hit"),
        local_blks_read: row.get("local_blks_read"),
        local_blks_dirtied: row.get("local_blks_dirtied"),
        local_blks_written: row.get("local_blks_written"),
        temp_blks_read: row.get("temp_blks_read"),
        temp_blks_written: row.get("temp_blks_written"),
        blk_read_time: row.get("blk_read_time"),
        blk_write_time: row.get("blk_write_time"),
        first_call: row.get("first_call_us"),
        last_call: row.get("last_call_us"),
        total_plan_time: row.get("total_plan_time"),
        min_plan_time: row.get("min_plan_time"),
        max_plan_time: row.get("max_plan_time"),
        mean_plan_time: row.get("mean_plan_time"),
    }
}

/// Collect every OSSC-compatible plan entry.
///
/// # Errors
/// Returns the [`tokio_postgres::Error`] if the query fails.
pub async fn collect_ossc<E>(
    session: Session<'_>,
    capability: &StorePlansCapability,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<OsscRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    query::read_batched(
        session,
        &store_plans_query(capability),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        ossc_row_from_pg,
        sink,
    )
    .await
}

/// Collect every vadv entry and its bounded plan text in one set-based query.
///
/// # Errors
/// Returns the [`tokio_postgres::Error`] if a query fails.
pub async fn collect_vadv<E>(
    session: Session<'_>,
    capability: &StorePlansCapability,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<VadvRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    query::read_batched(
        session,
        &store_plans_query(capability),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        vadv_row_from_pg,
        sink,
    )
    .await
}

#[cfg(test)]
mod tests;
