use super::{
    StatementsRow, StatementsVersion, statements_query, statements_version, to_v1, to_v2, to_v4,
    to_v6,
};
use crate::extension::parse_version;
use kronika_registry::StrId;
use std::convert::Infallible;

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match the fallible interner signature to_v* expects"
)]
fn fake_intern(bytes: &[u8]) -> Result<StrId, Infallible> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(StrId(h | 1))
}

fn layout(extversion: &str) -> Option<StatementsVersion> {
    parse_version(extversion).and_then(statements_version)
}

fn sample_row() -> StatementsRow {
    StatementsRow {
        ts: 2_000,
        queryid: Some(-42),
        userid: 10,
        dbid: 16_400,
        toplevel: Some(true),
        datname: Some("appdb".to_owned()),
        usename: Some("app".to_owned()),
        calls: 1_000,
        rows: 25_000,
        plans: Some(900),
        total_exec_time: 4_200.5,
        total_plan_time: Some(120.0),
        min_exec_time: 0.5,
        max_exec_time: 90.0,
        mean_exec_time: 4.2,
        stddev_exec_time: 1.1,
        min_plan_time: Some(0.1),
        max_plan_time: Some(5.0),
        mean_plan_time: Some(0.13),
        stddev_plan_time: Some(0.02),
        shared_blks_hit: 900_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 300,
        shared_blks_written: 100,
        local_blks_hit: 10,
        local_blks_read: 2,
        local_blks_dirtied: 1,
        local_blks_written: 0,
        temp_blks_read: 7,
        temp_blks_written: 9,
        shared_blk_read_time: 33.0,
        shared_blk_write_time: 4.0,
        local_blk_read_time: Some(0.5),
        local_blk_write_time: Some(0.25),
        temp_blk_read_time: Some(2.0),
        temp_blk_write_time: Some(3.0),
        wal_records: Some(50_000),
        wal_fpi: Some(400),
        wal_bytes: Some(9_000_000),
        wal_buffers_full: Some(12),
        jit_functions: Some(6),
        jit_generation_time: Some(1.5),
        jit_inlining_count: Some(2),
        jit_inlining_time: Some(0.75),
        jit_optimization_count: Some(2),
        jit_optimization_time: Some(3.5),
        jit_emission_count: Some(2),
        jit_emission_time: Some(2.25),
        jit_deform_count: Some(4),
        jit_deform_time: Some(0.9),
        parallel_workers_to_launch: Some(8),
        parallel_workers_launched: Some(6),
        stats_since: Some(1_000),
        minmax_stats_since: Some(1_500),
    }
}

#[test]
fn the_extension_version_selects_the_layout() {
    assert_eq!(layout("1.6"), Some(StatementsVersion::V1));
    assert_eq!(layout("1.7"), Some(StatementsVersion::V1));
    assert_eq!(layout("1.8"), Some(StatementsVersion::V2));
    assert_eq!(layout("1.9"), Some(StatementsVersion::V3));
    assert_eq!(layout("1.10"), Some(StatementsVersion::V4));
    assert_eq!(layout("1.11"), Some(StatementsVersion::V5));
    assert_eq!(layout("1.12"), Some(StatementsVersion::V6));
}

#[test]
fn an_extension_too_old_or_unknown_is_not_collected() {
    assert_eq!(layout("1.5"), None);
    assert_eq!(layout("2.0"), None);
}

#[test]
fn a_later_release_of_the_same_line_keeps_the_newest_layout() {
    assert_eq!(layout("1.13"), Some(StatementsVersion::V6));
}

#[test]
fn a_query_asks_only_for_columns_its_extension_has() {
    assert!(statements_query(StatementsVersion::V1).contains("s.total_time"));
    assert!(!statements_query(StatementsVersion::V1).contains("s.plans"));
    assert!(statements_query(StatementsVersion::V2).contains("s.total_plan_time"));
    assert!(!statements_query(StatementsVersion::V2).contains("s.toplevel"));
    assert!(statements_query(StatementsVersion::V3).contains("s.toplevel"));
    assert!(!statements_query(StatementsVersion::V3).contains("jit_functions"));
    assert!(statements_query(StatementsVersion::V4).contains("jit_functions"));
    assert!(!statements_query(StatementsVersion::V4).contains("jit_deform_count"));
    assert!(statements_query(StatementsVersion::V5).contains("jit_deform_count"));
    assert!(!statements_query(StatementsVersion::V5).contains("wal_buffers_full"));
    assert!(statements_query(StatementsVersion::V6).contains("wal_buffers_full"));
}

#[test]
fn the_block_timing_columns_are_asked_for_under_the_name_that_release_uses() {
    assert!(statements_query(StatementsVersion::V4).contains("s.blk_read_time AS"));
    assert!(!statements_query(StatementsVersion::V4).contains("s.local_blk_read_time"));
    assert!(statements_query(StatementsVersion::V5).contains("s.shared_blk_read_time"));
    assert!(statements_query(StatementsVersion::V5).contains("s.local_blk_read_time"));
}

#[test]
fn every_query_carries_the_marker_and_asks_for_no_statement_text() {
    for version in [
        StatementsVersion::V1,
        StatementsVersion::V2,
        StatementsVersion::V3,
        StatementsVersion::V4,
        StatementsVersion::V5,
        StatementsVersion::V6,
    ] {
        let sql = statements_query(version);
        assert!(sql.contains("kronika:"), "{sql}");
        assert!(sql.contains("pg_stat_statements(false)"), "{sql}");
        assert!(!sql.contains("s.query,"), "{sql}");
    }
}

#[test]
fn to_v6_maps_every_column_and_leaves_the_text_out() {
    let r = to_v6(&sample_row(), fake_intern).expect("infallible intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.queryid, Some(-42));
    assert_eq!(r.datname, Some(fake_intern(b"appdb").unwrap()));
    assert_eq!(r.query, None);
    assert!(r.toplevel);
    assert_eq!(r.calls, 1_000);
    assert!((r.total_exec_time - 4_200.5).abs() < f64::EPSILON);
    assert!((r.shared_blk_read_time - 33.0).abs() < f64::EPSILON);
    assert_eq!(r.wal_buffers_full, 12);
    assert_eq!(r.parallel_workers_launched, 6);
    assert_eq!(r.stats_since.map(|ts| ts.0), Some(1_000));
}

#[test]
fn a_masked_query_id_stays_absent() {
    let mut row = sample_row();
    row.queryid = None;
    assert_eq!(to_v6(&row, fake_intern).expect("intern").queryid, None);
}

#[test]
fn a_column_the_extension_never_sent_lands_as_zero() {
    let mut row = sample_row();
    row.wal_buffers_full = None;
    row.jit_deform_count = None;
    row.total_plan_time = None;
    let r = to_v6(&row, fake_intern).expect("intern");
    assert_eq!(r.wal_buffers_full, 0);
    assert_eq!(r.jit_deform_count, 0);
    assert!(r.total_plan_time.abs() < f64::EPSILON);
}

#[test]
fn the_older_layouts_keep_the_block_timing_under_its_old_name() {
    let r = to_v4(&sample_row(), fake_intern).expect("intern");
    assert!((r.blk_read_time - 33.0).abs() < f64::EPSILON);
    assert!((r.blk_write_time - 4.0).abs() < f64::EPSILON);
}

#[test]
fn to_v2_has_the_planning_columns_but_no_toplevel() {
    let r = to_v2(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.plans, 900);
    assert!((r.total_plan_time - 120.0).abs() < f64::EPSILON);
}

#[test]
fn to_v1_maps_execution_time_onto_the_name_that_release_used() {
    let r = to_v1(&sample_row(), fake_intern).expect("intern");
    assert!((r.total_time - 4_200.5).abs() < f64::EPSILON);
    assert!((r.mean_time - 4.2).abs() < f64::EPSILON);
    assert_eq!(r.calls, 1_000);
}

#[test]
fn intern_failure_propagates() {
    fn boom(_b: &[u8]) -> Result<StrId, &'static str> {
        Err("full")
    }
    assert_eq!(to_v6(&sample_row(), boom), Err("full"));
}
