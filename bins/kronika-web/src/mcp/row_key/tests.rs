use serde_json::{Map, Value, json};

use super::{attach, discriminator, matches, verify};

#[test]
fn every_locator_emitting_section_names_its_discriminator() {
    for (section, column) in [
        ("pg_stat_statements", "queryid"),
        ("pg_store_plans", "planid"),
        ("pg_stat_activity", "pid"),
        ("pg_stat_progress_vacuum", "pid"),
        ("pg_locks", "pid"),
        ("pg_log_lock_waits", "pid"),
        ("pg_stat_database", "datid"),
        ("pg_settings", "name"),
        ("os_process", "pid"),
        ("pg_log_errors", "pattern"),
        ("pg_log_slow_queries", "pattern"),
        ("pg_log_checkpoints", "phase"),
        ("pg_log_autovacuum", "relation"),
        ("pg_log_temp_files", "size_bytes"),
        ("pg_log_lifecycle", "kind"),
        ("pgbouncer_events", "text"),
    ] {
        assert_eq!(discriminator(section), Some(column), "{section}");
    }
    assert_eq!(discriminator("instance_metadata"), None);
    assert_eq!(discriminator("os_meminfo"), None);
}

#[test]
fn attach_copies_the_value_and_skips_null_or_missing_columns() {
    let mut row: Map<String, Value> = Map::new();
    row.insert("planid".to_owned(), json!("-7"));
    attach("pg_store_plans", &mut row);
    assert_eq!(row["row_key"], "-7");

    let mut null_row: Map<String, Value> = Map::new();
    null_row.insert("relation".to_owned(), Value::Null);
    attach("pg_log_autovacuum", &mut null_row);
    assert!(!null_row.contains_key("row_key"));

    let mut bare: Map<String, Value> = Map::new();
    attach("pg_settings", &mut bare);
    assert!(!bare.contains_key("row_key"));

    let mut keyless: Map<String, Value> = Map::new();
    keyless.insert("mem_total".to_owned(), json!(1));
    attach("os_meminfo", &mut keyless);
    assert!(!keyless.contains_key("row_key"));
}

#[test]
fn verify_covers_all_four_outcomes() {
    assert_eq!(
        verify("pg_log_autovacuum", "relation", None, &Value::Null),
        Ok(())
    );
    let required = verify("pg_stat_statements", "queryid", None, &json!("1"))
        .expect_err("a keyed row demands a row_key");
    assert!(required.contains("row_key is required"), "{required}");
    assert_eq!(
        verify(
            "pg_stat_statements",
            "queryid",
            Some(&json!(1)),
            &json!("1")
        ),
        Ok(())
    );
    let stale = verify(
        "pg_stat_statements",
        "queryid",
        Some(&json!("2")),
        &json!("1"),
    )
    .expect_err("a mismatch is stale");
    assert!(
        stale.contains("stale locator") && stale.contains("re-run"),
        "{stale}"
    );
}

#[test]
fn stale_errors_keep_kilobyte_keys_to_one_line() {
    let long = "x".repeat(5000);
    let stale = verify(
        "pg_log_errors",
        "pattern",
        Some(&json!("other")),
        &json!(long),
    )
    .expect_err("mismatch");
    assert!(
        stale.len() < 600,
        "error stays short: {} bytes",
        stale.len()
    );
    assert!(stale.contains('…'));
}

#[test]
fn matching_bridges_numbers_and_decimal_strings() {
    assert!(matches(&json!("101"), &json!(101)));
    assert!(matches(&json!(101), &json!("101")));
    assert!(matches(&json!("alpha"), &json!("alpha")));
    assert!(!matches(&json!("101"), &json!(102)));
    assert!(!matches(&json!("alpha"), &json!("beta")));
    assert!(!matches(&Value::Null, &json!(101)));
}
