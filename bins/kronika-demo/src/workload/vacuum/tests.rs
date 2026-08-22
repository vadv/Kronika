use super::{run_sql, setup_sql};

#[test]
fn setup_creates_a_visible_table_of_the_requested_size() {
    let sql = setup_sql(100_000);
    assert!(sql.contains("tenant_0.vacuum_showcase"));
    assert!(sql.contains("generate_series(1, 100000)"));
    assert!(sql.contains("repeat(md5(series::text), 8)"));
}

#[test]
fn each_round_has_a_bounded_update_and_throttled_vacuum() {
    let sql = run_sql(30).join("; ");
    assert!(sql.contains("statement_timeout = '30s'"));
    assert!(!sql.contains("statement_timeout = 0"));
    assert!(sql.contains("update tenant_0.vacuum_showcase"));
    assert!(sql.contains("vacuum (analyze) tenant_0.vacuum_showcase"));
    assert!(sql.contains("vacuum_cost_delay = '25ms'"));
    assert!(sql.contains("vacuum_cost_limit = 10"));
}
