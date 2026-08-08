use super::{
    Flavour, OsscRow, StorePlansCapability, VadvRow, afford, capability, plan_identity,
    store_plans_query, to_ossc, to_vadv,
};
use crate::extension::{ExtensionSchema, InventoryEntry};
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

fn inventory(extversion: &str) -> InventoryEntry {
    InventoryEntry {
        name: "pg_store_plans".to_owned(),
        extversion: extversion.to_owned(),
        schema: ExtensionSchema::new("plans schema"),
        schema_usable: true,
        full_visibility: true,
        statements_reader: false,
        store_plans_zero_arg: false,
        store_plans_bool_arg: false,
        store_plans_key_getter: false,
        store_plans_ossc_columns: false,
        store_plans_vadv_columns: false,
    }
}

fn ossc_capability() -> StorePlansCapability {
    StorePlansCapability {
        flavour: Flavour::OsscCompatible,
        schema: ExtensionSchema::new("plans schema"),
        full_visibility: true,
    }
}

fn vadv_capability() -> StorePlansCapability {
    StorePlansCapability {
        flavour: Flavour::Vadv,
        schema: ExtensionSchema::new("plans schema"),
        full_visibility: true,
    }
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
        userid: 10,
        dbid: 16_400,
        queryid: 123_456,
        planid: 991,
        queryid_stat_statements: Some(-7),
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
fn exact_catalog_capabilities_not_extversion_select_the_interface() {
    let mut datasentinel = inventory("2.0");
    datasentinel.store_plans_zero_arg = true;
    datasentinel.store_plans_ossc_columns = true;
    assert_eq!(
        capability(&datasentinel).map(|found| found.flavour),
        Some(Flavour::OsscCompatible)
    );

    let mut ossc_1_9 = inventory("1.9");
    ossc_1_9.store_plans_zero_arg = true;
    ossc_1_9.store_plans_ossc_columns = true;
    assert_eq!(
        capability(&ossc_1_9).map(|found| found.flavour),
        Some(Flavour::OsscCompatible)
    );

    let mut vadv = inventory("1.9");
    vadv.store_plans_bool_arg = true;
    vadv.store_plans_key_getter = true;
    vadv.store_plans_vadv_columns = true;
    assert_eq!(
        capability(&vadv).map(|found| found.flavour),
        Some(Flavour::Vadv)
    );
}

#[test]
fn an_incomplete_or_inaccessible_interface_is_not_selected() {
    let mut incomplete = inventory("2.0");
    incomplete.store_plans_zero_arg = true;
    assert!(capability(&incomplete).is_none());
    incomplete.store_plans_ossc_columns = true;
    incomplete.schema_usable = false;
    assert!(capability(&incomplete).is_none());
}

#[test]
fn each_interface_asks_only_for_the_columns_it_has() {
    let ossc = store_plans_query(&ossc_capability(), 10);
    let vadv = store_plans_query(&vadv_capability(), 10);
    assert!(ossc.contains("s.shared_blk_read_time"), "{ossc}");
    assert!(!ossc.contains("slow_log_calls"), "{ossc}");
    assert!(vadv.contains("s.blk_read_time"), "{vadv}");
    assert!(vadv.contains("s.queryid_stat_statements"), "{vadv}");
    assert!(vadv.contains("slow_log_calls"), "{vadv}");
    assert!(vadv.contains("total_plan_time"), "{vadv}");
    assert!(
        ossc.contains("s.temp_blk_write_time, s.first_call, s.last_call"),
        "{ossc}"
    );
    assert!(
        vadv.contains("s.blk_write_time, s.first_call, s.last_call"),
        "{vadv}"
    );
}

#[test]
fn a_read_is_bounded_schema_qualified_and_costliest_first() {
    for capability in [ossc_capability(), vadv_capability()] {
        let sql = store_plans_query(&capability, 42);
        assert!(sql.contains("kronika:"), "{sql}");
        assert!(sql.contains("ORDER BY s.total_time DESC"), "{sql}");
        assert!(sql.contains("LIMIT 42"), "{sql}");
        assert!(sql.contains("\"plans schema\".\"pg_store_plans\""), "{sql}");
    }
}

#[test]
fn vadv_uses_one_query_and_the_exact_four_key_plan_getter() {
    let sql = store_plans_query(&vadv_capability(), 10);
    assert!(sql.contains("\"pg_store_plans\"(false)"), "{sql}");
    assert!(
        sql.contains(
            "\"pg_store_plans_get_plan\"(s.userid, s.dbid, s.queryid, s.planid) END AS plan"
        ),
        "{sql}"
    );
    assert!(sql.contains("s.queryid IS NOT NULL"), "{sql}");
    assert!(sql.contains("s.planid IS NOT NULL"), "{sql}");
    assert!(!sql.contains("pg_store_plans_get_plan($1)"), "{sql}");
}

#[test]
fn ossc_and_datasentinel_use_the_proved_zero_argument_interface() {
    let sql = store_plans_query(&ossc_capability(), 10);
    assert!(sql.contains("\"pg_store_plans\"()"), "{sql}");
    assert!(!sql.contains("pg_store_plans_get_plan"), "{sql}");
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
fn a_masked_query_or_plan_identity_is_skipped() {
    assert_eq!(plan_identity(None, Some(2), Some(3), Some(4)), None);
    assert_eq!(plan_identity(Some(1), None, Some(3), Some(4)), None);
    assert_eq!(
        plan_identity(Some(1), Some(2), Some(3), Some(4)),
        Some((1, 2, 3, 4))
    );
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
fn vadv_source_row_keeps_both_query_ids() {
    let row = vadv_row();
    assert_eq!(row.queryid, 123_456);
    assert_eq!(row.queryid_stat_statements, Some(-7));
    let encoded = to_vadv(&row, fake_intern).expect("intern");
    assert_eq!(encoded.queryid_stat_statements, -7);
    assert_eq!(encoded.slow_log_calls, 4);
    assert!((encoded.total_plan_time - 30.0).abs() < f64::EPSILON);
    assert!((encoded.blk_read_time - 12.0).abs() < f64::EPSILON);
}

#[test]
fn a_missing_best_effort_pgss_query_id_maps_to_zero() {
    let mut row = vadv_row();
    row.queryid_stat_statements = None;
    assert_eq!(
        to_vadv(&row, fake_intern)
            .expect("infallible intern")
            .queryid_stat_statements,
        0
    );
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
