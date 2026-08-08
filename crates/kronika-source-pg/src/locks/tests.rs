use std::convert::Infallible;

use kronika_registry::StrId;

use super::{LockRow, LocksVersion, locks_query, locks_version, to_v1, to_v2};

#[allow(
    clippy::unnecessary_wraps,
    reason = "the converter API is fallible; Infallible proves this test interner is not"
)]
fn intern(bytes: &[u8]) -> Result<StrId, Infallible> {
    Ok(StrId(u64::try_from(bytes.len()).unwrap_or(u64::MAX) + 1))
}

fn row() -> LockRow {
    LockRow {
        ts: 2_000_000,
        pid: 20,
        blocked_by: vec![10, 0],
        depth: 1,
        root_pid: 10,
        datid: 16_384,
        datname: "app".to_owned(),
        usename: Some("monitor".to_owned()),
        application_name: "psql".to_owned(),
        client_addr: "127.0.0.1".to_owned(),
        backend_type: "client backend".to_owned(),
        state: Some("active".to_owned()),
        wait_event_type: Some("Lock".to_owned()),
        wait_event: Some("transactionid".to_owned()),
        query: "update t set v = 1".to_owned(),
        backend_xid_age: Some(5),
        backend_xmin_age: Some(3),
        backend_start: Some(1_000_000),
        xact_start: Some(1_500_000),
        query_start: Some(1_750_000),
        state_change: Some(1_750_000),
        lock_locktype: Some("transactionid".to_owned()),
        lock_mode: Some("ShareLock".to_owned()),
        lock_granted: Some(false),
        lock_database: Some(16_384),
        lock_relation: None,
        lock_relname: None,
        lock_page: None,
        lock_tuple: None,
        lock_virtualxid: None,
        lock_transactionid: Some(42),
        lock_classid: None,
        lock_objid: None,
        lock_objsubid: None,
        lock_fastpath: Some(false),
        lock_target: Some("transaction 42".to_owned()),
        waitstart: Some(1_900_000),
    }
}

#[test]
fn version_changes_with_waitstart() {
    assert_eq!(locks_version(10), LocksVersion::V1);
    assert_eq!(locks_version(13), LocksVersion::V1);
    assert_eq!(locks_version(14), LocksVersion::V2);
    assert_eq!(locks_version(18), LocksVersion::V2);
}

#[test]
fn query_builds_a_bounded_backend_graph() {
    let v1 = locks_query(LocksVersion::V1);
    let v2 = locks_query(LocksVersion::V2);
    assert!(v1.contains("pg_blocking_pids"));
    assert!(v1.contains("a.pid <> pg_backend_pid()"));
    assert!(v1.contains("SELECT DISTINCT ON (pid)"));
    assert!(!v1.contains("waitstart_us"));
    assert!(v2.contains("waitstart_us"));
    assert!(v2.contains("kronika:"));
}

#[test]
fn converters_preserve_edges_and_versioned_waitstart() {
    let raw = row();
    let v1 = to_v1(&raw, intern).expect("infallible intern");
    let v2 = to_v2(&raw, intern).expect("infallible intern");
    assert_eq!(v1.blocked_by, [10, 0]);
    assert_eq!(v1.root_pid, 10);
    assert_eq!(v1.lock_transactionid, Some(42));
    assert_eq!(v2.blocked_by, [10, 0]);
    assert_eq!(v2.waitstart.map(|ts| ts.0), Some(1_900_000));
}
