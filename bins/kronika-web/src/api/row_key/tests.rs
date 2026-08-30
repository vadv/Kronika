use kronika_reader::{Cell, Row};
use serde_json::{Map, Value, json};

use super::{detail_locator, identity, identity_columns, is_detail_text, validate};

fn row(type_id: u32, values: &[(&str, Cell)]) -> Row {
    let contract = kronika_registry::contract(type_id).expect("registered fixture type");
    let mut cells = vec![Cell::Null; contract.columns.len()];
    for (name, value) in values {
        let index = contract
            .columns
            .iter()
            .position(|column| column.name == *name)
            .expect("fixture column");
        cells[index] = value.clone();
    }
    Row::new(contract, cells)
}

#[test]
fn mountinfo_identity_is_the_complete_registry_tuple() {
    let row = row(
        1_112_002,
        &[
            ("ts", Cell::Ts(11)),
            ("major", Cell::U32(8)),
            ("minor", Cell::U32(32)),
            ("mount_point", Cell::StrId(99)),
        ],
    );
    assert_eq!(
        identity(1_112_002, &row).expect("identity"),
        Map::from_iter([
            ("major".to_owned(), json!("8")),
            ("minor".to_owned(), json!("32")),
            ("mount_point".to_owned(), json!("99")),
        ])
    );
}

#[test]
fn event_identity_covers_every_non_timestamp_cell_without_raw_text() {
    let contract = kronika_registry::contract(2_100_001).expect("pgbouncer contract");
    assert_eq!(
        identity_columns(contract).collect::<Vec<_>>(),
        [
            "source_file",
            "level",
            "database",
            "username",
            "host",
            "text",
        ]
    );
    let row = row(
        2_100_001,
        &[
            ("ts", Cell::Ts(11)),
            ("source_file", Cell::StrId(1)),
            ("level", Cell::U32(2)),
            ("database", Cell::StrId(3)),
            ("text", Cell::StrId(9_999)),
        ],
    );
    let identity = identity(2_100_001, &row).expect("identity");
    assert_eq!(identity["text"], "9999");
    assert_eq!(identity["username"], Value::Null);
}

#[test]
fn every_event_layout_uses_every_non_timestamp_column() {
    for contract in kronika_registry::registry()
        .iter()
        .filter(|contract| contract.semantics == kronika_registry::Semantics::EventStream)
    {
        let expected = contract
            .columns
            .iter()
            .filter(|column| column.class != kronika_registry::ColumnClass::Timestamp)
            .map(|column| column.name)
            .collect::<Vec<_>>();
        assert_eq!(
            identity_columns(contract).collect::<Vec<_>>(),
            expected,
            "type_id {}",
            contract.type_id.get(),
        );
    }
}

#[test]
fn validation_requires_every_exact_member_and_rejects_extras() {
    let valid = Map::from_iter([
        ("major".to_owned(), json!("8")),
        ("minor".to_owned(), json!("32")),
        ("mount_point".to_owned(), json!("99")),
    ]);
    assert_eq!(validate(1_112_002, &valid), Ok(()));

    let mut missing = valid.clone();
    missing.remove("minor");
    let error = validate(1_112_002, &missing).expect_err("missing member");
    assert!(error.contains("missing [minor]"), "{error}");

    let mut extra = valid;
    extra.insert("guess".to_owned(), json!(1));
    let error = validate(1_112_002, &extra).expect_err("extra member");
    assert!(error.contains("unexpected [guess]"), "{error}");
}

#[test]
fn detail_locator_is_the_complete_nested_row_detail_input() {
    let identity = Map::from_iter([("pid".to_owned(), json!("42"))]);
    let locator = serde_json::to_value(detail_locator("os_process", 7, 11, 1_100_001, 3, identity))
        .expect("locator JSON");

    assert_eq!(
        locator,
        json!({
            "section": "os_process",
            "segment_id": "7",
            "at": "11",
            "type_id": "1100001",
            "row_ordinal": "3",
            "identity": { "pid": "42" },
        })
    );
}

#[test]
fn exact_cell_encoding_preserves_float_bits_and_nulls() {
    let row = row(
        2_005_002,
        &[
            ("ts", Cell::Ts(11)),
            ("system_identifier", Cell::U64(7)),
            ("source_file", Cell::StrId(8)),
            ("kind", Cell::U32(1)),
            ("pid", Cell::I32(42)),
            ("duration_ms", Cell::F64(-0.0)),
        ],
    );
    let identity = identity(2_005_002, &row).expect("event identity");
    assert_eq!(identity["duration_ms"], "f64:8000000000000000");
    assert_eq!(identity["pid"], "42");
    assert_eq!(identity["statement"], Value::Null);
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
