//! `pg_store_plans` collection for types `1_003_001` and `1_004_001`.
//!
//! Two extensions share a name and little else. The ossc upstream keys an entry
//! by the core query id and carries the plan text in the view; the vadv fork
//! keys by the plan alone and hands out plan text through a function. They are
//! collected as two sections rather than one with holes in it.
//!
//! A read takes the costliest plans and stops fetching plan text once it has
//! taken enough of it. A plan text runs to kilobytes, and an instance can hold
//! thousands of entries, so an unbounded read would cost more than the
//! statistics it collects.

use kronika_registry::pg_store_plans::{PgStorePlansOsscV1, PgStorePlansVadvV1};
use kronika_registry::{StrId, Ts};
use tokio_postgres::Client;

use crate::extension::ExtensionVersion;

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

/// How many plan entries one read takes, costliest first.
pub const TOP_N: i64 = 500;

/// How many bytes of plan text one read takes before it stops asking for more.
pub const PLAN_TEXT_BUDGET: usize = 1 << 20;

/// Which `pg_store_plans` is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// The ossc upstream (extension 1.x): type `1_003_001`.
    Ossc,
    /// The vadv fork (extension 2.x): type `1_004_001`.
    Vadv,
}

/// Select the flavour from the installed extension version.
///
/// The fork carries the major version 2 precisely so the two can be told
/// apart; anything below 1.10 lacks the split I/O timing columns and is not
/// collected.
#[must_use]
pub const fn flavour(extension: ExtensionVersion) -> Option<Flavour> {
    match extension.major {
        1 if extension.minor >= 10 => Some(Flavour::Ossc),
        2 => Some(Flavour::Vadv),
        _ => None,
    }
}

/// The statistics query for one flavour, costliest plans first.
#[must_use]
pub fn store_plans_query(flavour: Flavour, limit: i64) -> String {
    let columns = match flavour {
        Flavour::Ossc => {
            "s.queryid, s.planid, s.userid, s.dbid, s.plan, \
             s.calls, s.total_time, s.min_time, s.max_time, s.mean_time, s.stddev_time, s.rows, \
             s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written, \
             s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written, \
             s.temp_blks_read, s.temp_blks_written, \
             s.shared_blk_read_time, s.shared_blk_write_time, \
             s.local_blk_read_time, s.local_blk_write_time, \
             s.temp_blk_read_time, s.temp_blk_write_time"
        }
        Flavour::Vadv => {
            "s.queryid AS queryid_stat_statements, s.planid, s.userid, s.dbid, \
             s.calls, s.slow_log_calls, \
             s.total_time, s.min_time, s.max_time, s.mean_time, s.stddev_time, s.rows, \
             s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written, \
             s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written, \
             s.temp_blks_read, s.temp_blks_written, \
             s.blk_read_time, s.blk_write_time, \
             s.total_plan_time, s.min_plan_time, s.max_plan_time, s.mean_plan_time"
        }
    };
    format!(
        "{}{columns}, \
         d.datname::text AS datname, r.rolname::text AS usename, \
         (extract(epoch from s.first_call) * 1e6)::int8 AS first_call_us, \
         (extract(epoch from s.last_call) * 1e6)::int8 AS last_call_us, \
         (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
         FROM pg_store_plans s \
         LEFT JOIN pg_database d ON d.oid = s.dbid \
         LEFT JOIN pg_roles r ON r.oid = s.userid \
         ORDER BY s.total_time DESC LIMIT {limit}",
        marked!("SELECT ")
    )
}

/// The plan-text query of the vadv fork, which keeps the text behind a
/// function instead of a column.
const PLAN_TEXT_QUERY: &str = marked!("SELECT pg_store_plans_get_plan($1) AS plan");

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
    /// Plan text; `None` once the plan-text budget is spent.
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
    /// Query id of the last statement that ran this plan.
    pub queryid_stat_statements: i64,
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
    /// Plan text; `None` once the plan-text budget is spent.
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
    Ok(PgStorePlansVadvV1 {
        ts: Ts(row.ts),
        queryid_stat_statements: row.queryid_stat_statements,
        planid: row.planid,
        userid: row.userid,
        dbid: row.dbid,
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

/// Whether a plan text of `len` bytes still fits the budget, spending it.
///
/// The budget is checked before the text is taken rather than after, so one
/// enormous plan cannot push a read past the limit.
const fn afford(left: &mut usize, len: usize) -> bool {
    if len > *left {
        return false;
    }
    *left -= len;
    true
}

fn ossc_row_from_pg(row: &tokio_postgres::Row, left: &mut usize) -> OsscRow {
    let plan: Option<String> = row.get("plan");
    OsscRow {
        ts: row.get("ts_us"),
        queryid: row.get("queryid"),
        planid: row.get("planid"),
        userid: row.get("userid"),
        dbid: row.get("dbid"),
        datname: row.get("datname"),
        usename: row.get("usename"),
        plan: plan.filter(|text| afford(left, text.len())),
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
        queryid_stat_statements: row.get("queryid_stat_statements"),
        planid: row.get("planid"),
        userid: row.get("userid"),
        dbid: row.get("dbid"),
        datname: row.get("datname"),
        usename: row.get("usename"),
        plan: None,
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

/// Collect the costliest ossc plan entries.
///
/// # Errors
/// Returns the [`tokio_postgres::Error`] if the query fails.
pub async fn collect_ossc(
    client: &Client,
    limit: i64,
    plan_text_budget: usize,
) -> Result<Vec<OsscRow>, tokio_postgres::Error> {
    let rows = client
        .query(&store_plans_query(Flavour::Ossc, limit), &[])
        .await?;
    let mut left = plan_text_budget;
    Ok(rows
        .iter()
        .map(|row| ossc_row_from_pg(row, &mut left))
        .collect())
}

/// Collect the costliest vadv-fork plan entries, then their plan texts while
/// the budget lasts.
///
/// # Errors
/// Returns the [`tokio_postgres::Error`] if a query fails.
pub async fn collect_vadv(
    client: &Client,
    limit: i64,
    plan_text_budget: usize,
) -> Result<Vec<VadvRow>, tokio_postgres::Error> {
    let queried = client
        .query(&store_plans_query(Flavour::Vadv, limit), &[])
        .await?;
    let mut rows: Vec<VadvRow> = queried.iter().map(vadv_row_from_pg).collect();
    let mut left = plan_text_budget;
    for row in &mut rows {
        if left == 0 {
            break;
        }
        let text: Option<String> = client
            .query_one(PLAN_TEXT_QUERY, &[&row.planid])
            .await?
            .get("plan");
        row.plan = text.filter(|text| afford(&mut left, text.len()));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
