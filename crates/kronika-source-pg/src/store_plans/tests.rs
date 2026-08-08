use super::{Flavour, OsscRow, VadvRow, afford, flavour, store_plans_query, to_ossc, to_vadv};
use crate::extension::parse_version;
use kronika_registry::StrId;
use std::convert::Infallible;

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match the fallible interner signature to_* expects"
)]
fn fake_intern(bytes: &[u8]) -> Result<StrId, Infallible> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(StrId(h | 1))
}

fn installed(extversion: &str) -> Option<Flavour> {
    parse_version(extversion).and_then(flavour)
}

fn ossc_row() -> OsscRow {
    OsscRow {
        ts: 2_000,
        queryid: -7,
        planid: 991,
        userid: 10,
        dbid: 16_400,
        datname: Some("appdb".to_owned()),
        usename: Some("app".to_owned()),
        plan: Some("Seq Scan on orders".to_owned()),
        calls: 300,
        total_time: 900.0,
        min_time: 1.0,
        max_time: 40.0,
        mean_time: 3.0,
        stddev_time: 0.5,
        rows: 12_000,
        shared_blks_hit: 90_000,
        shared_blks_read: 400,
        shared_blks_dirtied: 20,
        shared_blks_written: 5,
        local_blks_hit: 1,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 3,
        temp_blks_written: 4,
        shared_blk_read_time: 12.0,
        shared_blk_write_time: 1.0,
        local_blk_read_time: 0.1,
        local_blk_write_time: 0.2,
        temp_blk_read_time: 0.3,
        temp_blk_write_time: 0.4,
        first_call: 1_000,
        last_call: 1_900,
    }
}

fn vadv_row() -> VadvRow {
    VadvRow {
        ts: 2_000,
        queryid_stat_statements: -7,
        planid: 991,
        userid: 10,
        dbid: 16_400,
        datname: Some("appdb".to_owned()),
        usename: Some("app".to_owned()),
        plan: Some("Index Scan using orders_pkey".to_owned()),
        calls: 300,
        slow_log_calls: 4,
        total_time: 900.0,
        min_time: 1.0,
        max_time: 40.0,
        mean_time: 3.0,
        stddev_time: 0.5,
        rows: 12_000,
        shared_blks_hit: 90_000,
        shared_blks_read: 400,
        shared_blks_dirtied: 20,
        shared_blks_written: 5,
        local_blks_hit: 1,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 3,
        temp_blks_written: 4,
        blk_read_time: 12.0,
        blk_write_time: 1.0,
        first_call: 1_000,
        last_call: 1_900,
        total_plan_time: 30.0,
        min_plan_time: 0.05,
        max_plan_time: 2.0,
        mean_plan_time: 0.1,
    }
}

#[test]
fn the_major_version_tells_the_two_extensions_apart() {
    assert_eq!(installed("1.10"), Some(Flavour::Ossc));
    assert_eq!(installed("1.11"), Some(Flavour::Ossc));
    assert_eq!(installed("2.0"), Some(Flavour::Vadv));
    assert_eq!(installed("2.3"), Some(Flavour::Vadv));
}

#[test]
fn an_extension_without_the_split_io_timing_is_not_collected() {
    assert_eq!(installed("1.9"), None);
    assert_eq!(installed("3.0"), None);
}

#[test]
fn each_flavour_asks_for_the_columns_it_has() {
    let ossc = store_plans_query(Flavour::Ossc, 10);
    let vadv = store_plans_query(Flavour::Vadv, 10);
    assert!(ossc.contains("s.shared_blk_read_time"), "{ossc}");
    assert!(!ossc.contains("slow_log_calls"), "{ossc}");
    assert!(vadv.contains("s.blk_read_time"), "{vadv}");
    assert!(vadv.contains("slow_log_calls"), "{vadv}");
    assert!(vadv.contains("total_plan_time"), "{vadv}");
}

#[test]
fn a_read_is_bounded_and_takes_the_costliest_plans_first() {
    for flavour in [Flavour::Ossc, Flavour::Vadv] {
        let sql = store_plans_query(flavour, 42);
        assert!(sql.contains("kronika:"), "{sql}");
        assert!(sql.contains("ORDER BY s.total_time DESC"), "{sql}");
        assert!(sql.ends_with("LIMIT 42"), "{sql}");
    }
}

#[test]
fn only_the_fork_needs_a_second_query_for_the_plan_text() {
    assert!(store_plans_query(Flavour::Ossc, 10).contains("s.plan"));
    assert!(!store_plans_query(Flavour::Vadv, 10).contains("s.plan,"));
}

#[test]
fn a_plan_text_is_taken_while_the_budget_covers_it() {
    let mut left = 10;
    assert!(afford(&mut left, 4));
    assert_eq!(left, 6);
    assert!(afford(&mut left, 6));
    assert_eq!(left, 0);
}

#[test]
fn a_plan_text_larger_than_what_is_left_is_skipped_whole() {
    let mut left = 10;
    assert!(!afford(&mut left, 11));
    assert_eq!(left, 10, "a refused plan text must not spend the budget");
}

#[test]
fn a_budget_of_nothing_takes_no_plan_text() {
    let mut left = 0;
    assert!(!afford(&mut left, 1));
    assert!(afford(&mut left, 0));
}

#[test]
fn to_ossc_maps_every_column() {
    let r = to_ossc(&ossc_row(), fake_intern).expect("infallible intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.queryid, -7);
    assert_eq!(r.planid, 991);
    assert_eq!(r.datname, Some(fake_intern(b"appdb").unwrap()));
    assert_eq!(r.plan, Some(fake_intern(b"Seq Scan on orders").unwrap()));
    assert!((r.temp_blk_write_time - 0.4).abs() < f64::EPSILON);
    assert_eq!(r.last_call.0, 1_900);
}

#[test]
fn to_vadv_maps_every_column_including_the_planning_times() {
    let r = to_vadv(&vadv_row(), fake_intern).expect("intern");
    assert_eq!(r.queryid_stat_statements, -7);
    assert_eq!(r.slow_log_calls, 4);
    assert!((r.total_plan_time - 30.0).abs() < f64::EPSILON);
    assert!((r.blk_read_time - 12.0).abs() < f64::EPSILON);
}

#[test]
fn a_row_whose_plan_text_did_not_fit_keeps_its_numbers() {
    let mut row = vadv_row();
    row.plan = None;
    let r = to_vadv(&row, fake_intern).expect("intern");
    assert_eq!(r.plan, None);
    assert_eq!(r.calls, 300);
}

#[test]
fn intern_failure_propagates() {
    fn boom(_b: &[u8]) -> Result<StrId, &'static str> {
        Err("full")
    }
    assert_eq!(to_ossc(&ossc_row(), boom), Err("full"));
}
