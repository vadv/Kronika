//! `pg_stat_user_indexes` collection for types `1_014_001`..`1_014_002`.
//!
//! One row per index of the connected database. The scan counters come from
//! `pg_stat_user_indexes`, the buffer counters from `pg_statio_user_indexes`,
//! and the shape of the index from `pg_index`, `pg_am` and `pg_get_indexdef`,
//! so a row says both how the index was used and what it is. `last_idx_scan`
//! arrives in PG16 and selects the layout.

use kronika_registry::pg_stat_user_indexes::{PgStatUserIndexesV1, PgStatUserIndexesV2};
use kronika_registry::{StrId, Ts};
use tokio_postgres::Client;

use crate::databases::Database;

/// Prefix a query literal with the kronika marker (SQL-transparency rule).
macro_rules! marked {
    ($sql:literal) => {
        concat!(
            "/* kronika:",
            env!("CARGO_PKG_VERSION"),
            " crates/kronika-source-pg/src/user_indexes.rs */ ",
            $sql,
        )
    };
}

/// The `pg_stat_user_indexes` layout selected by the server major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserIndexesVersion {
    /// PG 10-15: type `1_014_001` (base layout).
    V1,
    /// PG 16-18: type `1_014_002` (adds `last_idx_scan`).
    V2,
}

/// Select the layout for a server major version.
#[must_use]
pub const fn user_indexes_version(major: u32) -> UserIndexesVersion {
    if major >= 16 {
        UserIndexesVersion::V2
    } else {
        UserIndexesVersion::V1
    }
}

/// The columns every layout shares, as one SQL fragment.
const COMMON_COLUMNS: &str = "si.indexrelid, si.relid, \
     si.schemaname::text AS schemaname, si.relname::text AS relname, \
     si.indexrelname::text AS indexrelname, \
     coalesce(ts.spcname, dts.spcname)::text AS tablespace, \
     coalesce(si.idx_scan, 0) AS idx_scan, \
     coalesce(si.idx_tup_read, 0) AS idx_tup_read, \
     coalesce(si.idx_tup_fetch, 0) AS idx_tup_fetch, \
     coalesce(pg_relation_size(si.indexrelid), 0) AS main_fork_bytes, \
     i.indisunique, i.indisprimary, i.indisvalid, i.indisexclusion, i.indisready, \
     am.amname::text AS amname, \
     pg_get_indexdef(si.indexrelid) AS indexdef, \
     coalesce(sio.idx_blks_read, 0) AS idx_blks_read, \
     coalesce(sio.idx_blks_hit, 0) AS idx_blks_hit, \
     (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us";

/// The joins every layout shares.
///
/// `pg_tablespace` is joined twice because `reltablespace` is `0` for a
/// relation in the database default, which is the tablespace to report.
const COMMON_FROM: &str = " FROM pg_stat_user_indexes si \
     JOIN pg_statio_user_indexes sio ON sio.indexrelid = si.indexrelid \
     JOIN pg_class c ON c.oid = si.indexrelid \
     JOIN pg_am am ON am.oid = c.relam \
     JOIN pg_index i ON i.indexrelid = si.indexrelid \
     LEFT JOIN pg_tablespace ts ON ts.oid = c.reltablespace \
     LEFT JOIN pg_tablespace dts ON dts.oid = \
         (SELECT dattablespace FROM pg_database WHERE datname = current_database())";

/// The SQL for one layout.
#[must_use]
pub fn user_indexes_query(version: UserIndexesVersion) -> String {
    let extra = match version {
        UserIndexesVersion::V1 => "",
        UserIndexesVersion::V2 => {
            ", (extract(epoch from si.last_idx_scan) * 1e6)::int8 AS last_idx_scan_us"
        }
    };
    format!("{}{COMMON_COLUMNS}{extra}{COMMON_FROM}", marked!("SELECT "))
}

/// One raw `pg_stat_user_indexes` row, a version-agnostic superset.
///
/// Strings are owned; the caller interns them. See [`PgStatUserIndexesV2`] for
/// column meanings.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent pg_index flag column, not interdependent state"
)]
#[derive(Debug, Clone)]
pub struct UserIndexesRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Database oid of the connection that produced this row.
    pub datid: u32,
    /// Database name of the connection.
    pub datname: String,
    /// Index oid.
    pub indexrelid: u32,
    /// Table oid the index belongs to.
    pub relid: u32,
    /// Schema name.
    pub schemaname: String,
    /// Table name.
    pub relname: String,
    /// Index name.
    pub indexrelname: String,
    /// Tablespace name.
    pub tablespace: String,
    /// Index scans.
    pub idx_scan: i64,
    /// Index entries returned by scans.
    pub idx_tup_read: i64,
    /// Live table rows fetched through this index.
    pub idx_tup_fetch: i64,
    /// Main-fork size in bytes.
    pub main_fork_bytes: i64,
    /// Last index scan, unix microseconds (V2); `None` if never or unknown.
    pub last_idx_scan: Option<i64>,
    /// Whether the index enforces uniqueness.
    pub indisunique: bool,
    /// Whether the index is a primary key.
    pub indisprimary: bool,
    /// Whether the index is valid for queries.
    pub indisvalid: bool,
    /// Whether the index enforces an exclusion constraint.
    pub indisexclusion: bool,
    /// Whether the index is ready for inserts.
    pub indisready: bool,
    /// Access method name.
    pub amname: String,
    /// Reconstructed index definition.
    pub indexdef: String,
    /// Shared-buffer misses for index blocks.
    pub idx_blks_read: i64,
    /// Shared-buffer hits for index blocks.
    pub idx_blks_hit: i64,
}

/// Build a `1_014_002` row (PG16+ layout), interning every string.
///
/// # Errors
/// Returns the interner's error.
pub fn to_v2<E>(
    row: &UserIndexesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserIndexesV2, E> {
    Ok(PgStatUserIndexesV2 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        indexrelid: row.indexrelid,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        indexrelname: intern(row.indexrelname.as_bytes())?,
        tablespace: intern(row.tablespace.as_bytes())?,
        idx_scan: row.idx_scan,
        idx_tup_read: row.idx_tup_read,
        idx_tup_fetch: row.idx_tup_fetch,
        main_fork_bytes: row.main_fork_bytes,
        last_idx_scan: row.last_idx_scan.map(Ts),
        indisunique: row.indisunique,
        indisprimary: row.indisprimary,
        indisvalid: row.indisvalid,
        indisexclusion: row.indisexclusion,
        indisready: row.indisready,
        amname: intern(row.amname.as_bytes())?,
        indexdef: intern(row.indexdef.as_bytes())?,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
    })
}

/// Build a `1_014_001` row (PG10-15 base layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v1<E>(
    row: &UserIndexesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserIndexesV1, E> {
    Ok(PgStatUserIndexesV1 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        indexrelid: row.indexrelid,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        indexrelname: intern(row.indexrelname.as_bytes())?,
        tablespace: intern(row.tablespace.as_bytes())?,
        idx_scan: row.idx_scan,
        idx_tup_read: row.idx_tup_read,
        idx_tup_fetch: row.idx_tup_fetch,
        main_fork_bytes: row.main_fork_bytes,
        indisunique: row.indisunique,
        indisprimary: row.indisprimary,
        indisvalid: row.indisvalid,
        indisexclusion: row.indisexclusion,
        indisready: row.indisready,
        amname: intern(row.amname.as_bytes())?,
        indexdef: intern(row.indexdef.as_bytes())?,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
    })
}

/// Read a raw row, filling in the database the connection is attached to.
fn row_from_pg(
    row: &tokio_postgres::Row,
    database: &Database,
    version: UserIndexesVersion,
) -> UserIndexesRow {
    UserIndexesRow {
        ts: row.get("ts_us"),
        datid: database.oid,
        datname: database.name.clone(),
        indexrelid: row.get("indexrelid"),
        relid: row.get("relid"),
        schemaname: row.get("schemaname"),
        relname: row.get("relname"),
        indexrelname: row.get("indexrelname"),
        tablespace: row.get("tablespace"),
        idx_scan: row.get("idx_scan"),
        idx_tup_read: row.get("idx_tup_read"),
        idx_tup_fetch: row.get("idx_tup_fetch"),
        main_fork_bytes: row.get("main_fork_bytes"),
        last_idx_scan: match version {
            UserIndexesVersion::V1 => None,
            UserIndexesVersion::V2 => row.get("last_idx_scan_us"),
        },
        indisunique: row.get("indisunique"),
        indisprimary: row.get("indisprimary"),
        indisvalid: row.get("indisvalid"),
        indisexclusion: row.get("indisexclusion"),
        indisready: row.get("indisready"),
        amname: row.get("amname"),
        indexdef: row.get("indexdef"),
        idx_blks_read: row.get("idx_blks_read"),
        idx_blks_hit: row.get("idx_blks_hit"),
    }
}

/// Collect every index of the database `client` is attached to.
///
/// # Errors
/// Returns the [`tokio_postgres::Error`] if the query fails.
pub async fn collect_user_indexes(
    client: &Client,
    database: &Database,
    major: u32,
) -> Result<(UserIndexesVersion, Vec<UserIndexesRow>), tokio_postgres::Error> {
    let version = user_indexes_version(major);
    let rows = client.query(&user_indexes_query(version), &[]).await?;
    let parsed = rows
        .iter()
        .map(|row| row_from_pg(row, database, version))
        .collect();
    Ok((version, parsed))
}

#[cfg(test)]
mod tests;
