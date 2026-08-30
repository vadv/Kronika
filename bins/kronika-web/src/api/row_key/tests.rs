use serde_json::{Map, Value, json};

use super::{attach, detail_locator, discriminator, is_detail_text, matches, verify};

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

#[test]
fn detail_locator_is_the_complete_nested_row_detail_input() {
    let fields = serde_json::json!({"pid": 42, "cmdline": "private --argument"})
        .as_object()
        .expect("fields")
        .clone();
    let locator = serde_json::to_value(detail_locator("os_process", 7, 11, 1_100_001, 3, &fields))
        .expect("locator JSON");

    assert_eq!(
        locator,
        json!({
            "section": "os_process",
            "segment_id": "7",
            "at": "11",
            "type_id": "1100001",
            "row_ordinal": "3",
            "row_key": 42,
        })
    );
}

#[test]
fn detail_text_policy_is_section_aware() {
    for (section, field) in [
        ("os_process", "cmdline"),
        ("pg_stat_activity", "query"),
        ("pg_locks", "query"),
        ("pg_stat_statements", "query"),
        ("pg_store_plans", "plan"),
        ("pg_log_errors", "sample"),
        ("pg_log_errors", "detail"),
        ("pg_log_errors", "hint"),
        ("pg_log_errors", "context"),
        ("pg_log_errors", "statement"),
        ("pg_log_slow_queries", "sample"),
        ("pg_log_checkpoints", "reason"),
        ("pg_log_lock_waits", "detail"),
        ("pg_log_lock_waits", "context"),
        ("pg_log_lock_waits", "statement"),
        ("pg_log_temp_files", "statement"),
        ("pg_log_lifecycle", "message"),
        ("pg_log_lifecycle", "query_detail"),
        ("pgbouncer_events", "text"),
    ] {
        assert!(is_detail_text(section, field), "{section}.{field}");
    }
    for (section, field) in [
        ("pg_log_errors", "pattern"),
        ("pg_log_slow_queries", "pattern"),
        ("pg_settings", "context"),
        ("pg_stat_io", "context"),
        ("pg_stat_activity", "queryid"),
    ] {
        assert!(!is_detail_text(section, field), "{section}.{field}");
    }
}

#[test]
fn raw_text_discriminator_is_hashed_and_still_verifies() {
    let raw = json!("closing because: private connection details");
    let mut fields = Map::new();
    fields.insert("text".to_owned(), raw.clone());
    attach("pgbouncer_events", &mut fields);

    let key = fields["row_key"].as_str().expect("hashed key");
    assert!(key.starts_with("sha256:"));
    assert_eq!(key.len(), "sha256:".len() + 64);
    assert!(!key.contains("private"));
    assert_eq!(
        verify("pgbouncer_events", "text", fields.get("row_key"), &raw,),
        Ok(())
    );

    let rendered_truncated = json!({
        "representation": "text",
        "stored_text": "stored prefix",
        "full_len": "99",
        "truncated": true,
        "sha256": "full-value-digest",
    });
    let mut fields = Map::new();
    fields.insert("text".to_owned(), rendered_truncated);
    attach("pgbouncer_events", &mut fields);
    assert_eq!(fields["row_key"], "sha256:full-value-digest");
}
