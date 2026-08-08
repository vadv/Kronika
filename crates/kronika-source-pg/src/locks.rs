//! `pg_locks` wait-tree collection for types `1_011_001` / `1_011_002`.
//!
//! The query returns one row per backend in a blocking component, not one row
//! per held lock. Its size is therefore bounded by the server's backend count.

use futures_util::{TryStreamExt, pin_mut};
use kronika_registry::pg_locks::{PgLocksV1, PgLocksV2};
use kronika_registry::{StrId, Ts};
use tokio_postgres::Client;
use tokio_postgres::types::{ToSql, Type};

const MARKER: &str = concat!(
    "/* kronika:",
    env!("CARGO_PKG_VERSION"),
    " crates/kronika-source-pg/src/locks.rs */ "
);

/// Layout selected by the `PostgreSQL` major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocksVersion {
    /// `PostgreSQL` 10-13, before `pg_locks.waitstart`.
    V1,
    /// `PostgreSQL` 14-18, with `pg_locks.waitstart`.
    V2,
}

/// Select the lock layout for a supported server.
#[must_use]
pub const fn locks_version(major: u32) -> LocksVersion {
    if major >= 14 {
        LocksVersion::V2
    } else {
        LocksVersion::V1
    }
}

/// Build the wait-tree query for one layout.
#[must_use]
pub fn locks_query(version: LocksVersion) -> String {
    let waitstart = match version {
        LocksVersion::V1 => "",
        LocksVersion::V2 => ", (extract(epoch from l.waitstart) * 1e6)::int8 AS waitstart_us",
    };
    format!(
        "{MARKER}WITH RECURSIVE activity AS (\
           SELECT a.*, ARRAY(\
             SELECT DISTINCT b.blocker FROM unnest(pg_blocking_pids(a.pid)) AS b(blocker) \
             ORDER BY b.blocker\
           )::int4[] AS blocked_by \
           FROM pg_stat_activity a WHERE a.pid <> pg_backend_pid()\
         ), involved_ids(pid) AS (\
           SELECT pid FROM activity WHERE cardinality(blocked_by) > 0 \
           UNION \
           SELECT b.blocker FROM involved_ids i \
           JOIN activity a ON a.pid = i.pid \
           CROSS JOIN LATERAL unnest(a.blocked_by) AS b(blocker) \
           WHERE b.blocker > 0\
         ), involved AS (\
           SELECT a.* FROM activity a JOIN involved_ids i ON i.pid = a.pid\
         ), roots(root_pid) AS (\
           SELECT pid FROM involved WHERE cardinality(blocked_by) = 0 \
           UNION ALL \
           SELECT 0 WHERE EXISTS (SELECT 1 FROM involved WHERE 0 = ANY(blocked_by))\
         ), walk(root_pid, pid, depth, path) AS (\
           SELECT root_pid, root_pid, 0, ARRAY[root_pid]::int4[] FROM roots \
           UNION ALL \
           SELECT w.root_pid, n.pid, w.depth + 1, w.path || n.pid \
           FROM walk w JOIN involved n ON w.pid = ANY(n.blocked_by) \
           WHERE NOT n.pid = ANY(w.path)\
         ), placement AS (\
           SELECT DISTINCT ON (pid) pid, root_pid, depth FROM walk WHERE pid <> 0 \
           ORDER BY pid, depth, root_pid\
         ) \
         SELECT (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us, \
         n.pid, n.blocked_by, p.depth, p.root_pid, coalesce(n.datid, 0::oid) AS datid, \
         coalesce(n.datname::text, '') AS datname, n.usename::text AS usename, \
         coalesce(n.application_name, '') AS application_name, \
         coalesce(n.client_addr::text, '') AS client_addr, \
         coalesce(n.backend_type, '') AS backend_type, n.state, n.wait_event_type, n.wait_event, \
         left(coalesce(n.query, ''), 65536) AS query, \
         age(n.backend_xid)::int8 AS backend_xid_age, \
         age(n.backend_xmin)::int8 AS backend_xmin_age, \
         (extract(epoch from n.backend_start) * 1e6)::int8 AS backend_start_us, \
         (extract(epoch from n.xact_start) * 1e6)::int8 AS xact_start_us, \
         (extract(epoch from n.query_start) * 1e6)::int8 AS query_start_us, \
         (extract(epoch from n.state_change) * 1e6)::int8 AS state_change_us, \
         l.locktype AS lock_locktype, l.mode AS lock_mode, l.granted AS lock_granted, \
         l.database AS lock_database, l.relation AS lock_relation, c.relname::text AS lock_relname, \
         l.page AS lock_page, l.tuple AS lock_tuple, l.virtualxid::text AS lock_virtualxid, \
         l.transactionid::text::int8 AS lock_transactionid, l.classid AS lock_classid, \
         l.objid AS lock_objid, l.objsubid AS lock_objsubid, l.fastpath AS lock_fastpath, \
         CASE WHEN c.oid IS NOT NULL THEN c.relname::text \
              WHEN l.transactionid IS NOT NULL THEN 'transaction ' || l.transactionid::text \
              WHEN l.virtualxid IS NOT NULL THEN 'virtualxid ' || l.virtualxid \
              WHEN l.classid IS NOT NULL THEN 'object ' || l.classid::text || '/' || l.objid::text \
              ELSE l.locktype END AS lock_target{waitstart} \
         FROM involved n JOIN placement p ON p.pid = n.pid \
         LEFT JOIN LATERAL (\
           SELECT held.* FROM pg_locks held \
           WHERE held.pid = n.pid AND NOT held.granted \
           ORDER BY held.locktype, held.database, held.relation, held.page, held.tuple, \
                    held.virtualxid, held.transactionid, held.classid, held.objid, held.objsubid \
           LIMIT 1\
         ) l ON true \
         LEFT JOIN pg_class c ON c.oid = l.relation \
           AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
         ORDER BY p.root_pid, p.depth, n.pid"
    )
}

/// One wait-tree row before string interning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRow {
    ts: i64,
    pid: i32,
    blocked_by: Vec<i32>,
    depth: i32,
    root_pid: i32,
    datid: u32,
    datname: String,
    usename: Option<String>,
    application_name: String,
    client_addr: String,
    backend_type: String,
    state: Option<String>,
    wait_event_type: Option<String>,
    wait_event: Option<String>,
    query: String,
    backend_xid_age: Option<i64>,
    backend_xmin_age: Option<i64>,
    backend_start: Option<i64>,
    xact_start: Option<i64>,
    query_start: Option<i64>,
    state_change: Option<i64>,
    lock_locktype: Option<String>,
    lock_mode: Option<String>,
    lock_granted: Option<bool>,
    lock_database: Option<u32>,
    lock_relation: Option<u32>,
    lock_relname: Option<String>,
    lock_page: Option<i32>,
    lock_tuple: Option<i16>,
    lock_virtualxid: Option<String>,
    lock_transactionid: Option<i64>,
    lock_classid: Option<u32>,
    lock_objid: Option<u32>,
    lock_objsubid: Option<i16>,
    lock_fastpath: Option<bool>,
    lock_target: Option<String>,
    waitstart: Option<i64>,
}

fn opt<E>(
    intern: &mut impl FnMut(&[u8]) -> Result<StrId, E>,
    value: Option<&str>,
) -> Result<Option<StrId>, E> {
    value.map(|text| intern(text.as_bytes())).transpose()
}

/// Build a `PostgreSQL` 10-13 row, interning labels.
///
/// # Errors
///
/// Returns the interner error.
pub fn to_v1<E>(
    row: &LockRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgLocksV1, E> {
    Ok(PgLocksV1 {
        ts: Ts(row.ts),
        pid: row.pid,
        blocked_by: row.blocked_by.clone(),
        depth: row.depth,
        root_pid: row.root_pid,
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        application_name: intern(row.application_name.as_bytes())?,
        client_addr: intern(row.client_addr.as_bytes())?,
        backend_type: intern(row.backend_type.as_bytes())?,
        state: opt(&mut intern, row.state.as_deref())?,
        wait_event_type: opt(&mut intern, row.wait_event_type.as_deref())?,
        wait_event: opt(&mut intern, row.wait_event.as_deref())?,
        query: intern(row.query.as_bytes())?,
        backend_xid_age: row.backend_xid_age,
        backend_xmin_age: row.backend_xmin_age,
        backend_start: row.backend_start.map(Ts),
        xact_start: row.xact_start.map(Ts),
        query_start: row.query_start.map(Ts),
        state_change: row.state_change.map(Ts),
        lock_locktype: opt(&mut intern, row.lock_locktype.as_deref())?,
        lock_mode: opt(&mut intern, row.lock_mode.as_deref())?,
        lock_granted: row.lock_granted,
        lock_database: row.lock_database,
        lock_relation: row.lock_relation,
        lock_relname: opt(&mut intern, row.lock_relname.as_deref())?,
        lock_page: row.lock_page,
        lock_tuple: row.lock_tuple,
        lock_virtualxid: opt(&mut intern, row.lock_virtualxid.as_deref())?,
        lock_transactionid: row.lock_transactionid,
        lock_classid: row.lock_classid,
        lock_objid: row.lock_objid,
        lock_objsubid: row.lock_objsubid,
        lock_fastpath: row.lock_fastpath,
        lock_target: opt(&mut intern, row.lock_target.as_deref())?,
    })
}

/// Build a `PostgreSQL` 14-18 row, interning labels.
///
/// # Errors
///
/// Returns the interner error.
pub fn to_v2<E>(
    row: &LockRow,
    intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgLocksV2, E> {
    let base = to_v1(row, intern)?;
    Ok(PgLocksV2 {
        ts: base.ts,
        pid: base.pid,
        blocked_by: base.blocked_by,
        depth: base.depth,
        root_pid: base.root_pid,
        datid: base.datid,
        datname: base.datname,
        usename: base.usename,
        application_name: base.application_name,
        client_addr: base.client_addr,
        backend_type: base.backend_type,
        state: base.state,
        wait_event_type: base.wait_event_type,
        wait_event: base.wait_event,
        query: base.query,
        backend_xid_age: base.backend_xid_age,
        backend_xmin_age: base.backend_xmin_age,
        backend_start: base.backend_start,
        xact_start: base.xact_start,
        query_start: base.query_start,
        state_change: base.state_change,
        lock_locktype: base.lock_locktype,
        lock_mode: base.lock_mode,
        lock_granted: base.lock_granted,
        lock_database: base.lock_database,
        lock_relation: base.lock_relation,
        lock_relname: base.lock_relname,
        lock_page: base.lock_page,
        lock_tuple: base.lock_tuple,
        lock_virtualxid: base.lock_virtualxid,
        lock_transactionid: base.lock_transactionid,
        lock_classid: base.lock_classid,
        lock_objid: base.lock_objid,
        lock_objsubid: base.lock_objsubid,
        lock_fastpath: base.lock_fastpath,
        lock_target: base.lock_target,
        waitstart: row.waitstart.map(Ts),
    })
}

fn from_pg(
    row: &tokio_postgres::Row,
    version: LocksVersion,
) -> Result<LockRow, tokio_postgres::Error> {
    Ok(LockRow {
        ts: row.try_get("ts_us")?,
        pid: row.try_get("pid")?,
        blocked_by: row.try_get("blocked_by")?,
        depth: row.try_get("depth")?,
        root_pid: row.try_get("root_pid")?,
        datid: row.try_get("datid")?,
        datname: row.try_get("datname")?,
        usename: row.try_get("usename")?,
        application_name: row.try_get("application_name")?,
        client_addr: row.try_get("client_addr")?,
        backend_type: row.try_get("backend_type")?,
        state: row.try_get("state")?,
        wait_event_type: row.try_get("wait_event_type")?,
        wait_event: row.try_get("wait_event")?,
        query: row.try_get("query")?,
        backend_xid_age: row.try_get("backend_xid_age")?,
        backend_xmin_age: row.try_get("backend_xmin_age")?,
        backend_start: row.try_get("backend_start_us")?,
        xact_start: row.try_get("xact_start_us")?,
        query_start: row.try_get("query_start_us")?,
        state_change: row.try_get("state_change_us")?,
        lock_locktype: row.try_get("lock_locktype")?,
        lock_mode: row.try_get("lock_mode")?,
        lock_granted: row.try_get("lock_granted")?,
        lock_database: row.try_get("lock_database")?,
        lock_relation: row.try_get("lock_relation")?,
        lock_relname: row.try_get("lock_relname")?,
        lock_page: row.try_get("lock_page")?,
        lock_tuple: row.try_get("lock_tuple")?,
        lock_virtualxid: row.try_get("lock_virtualxid")?,
        lock_transactionid: row.try_get("lock_transactionid")?,
        lock_classid: row.try_get("lock_classid")?,
        lock_objid: row.try_get("lock_objid")?,
        lock_objsubid: row.try_get("lock_objsubid")?,
        lock_fastpath: row.try_get("lock_fastpath")?,
        lock_target: row.try_get("lock_target")?,
        waitstart: if matches!(version, LocksVersion::V2) {
            row.try_get("waitstart_us")?
        } else {
            None
        },
    })
}

/// Stream one wait-tree snapshot into `on_row` using one unnamed statement.
///
/// # Errors
///
/// Returns `PostgreSQL` protocol or row-decoding errors.
pub async fn collect_locks(
    client: &Client,
    major: u32,
    mut on_row: impl FnMut(LockRow),
) -> Result<usize, tokio_postgres::Error> {
    let version = locks_version(major);
    let sql = locks_query(version);
    let stream = client
        .query_typed_raw(&sql, std::iter::empty::<(&(dyn ToSql + Sync), Type)>())
        .await?;
    pin_mut!(stream);
    let mut count = 0;
    while let Some(row) = stream.try_next().await? {
        on_row(from_pg(&row, version)?);
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
