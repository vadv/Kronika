use serde_json::{Value, json};

use super::{decimal_i64, decimal_u32, decimal_u64, normalize_detail_text};

#[test]
fn decimal_i64_accepts_a_plain_number() {
    assert_eq!(decimal_i64("segment_id", &json!(42)), Ok(42));
}

#[test]
fn decimal_i64_accepts_a_decimal_string_beyond_json_safe_range() {
    assert_eq!(
        decimal_i64("segment_id", &json!("9007199254740993")),
        Ok(9_007_199_254_740_993)
    );
}

#[test]
fn decimal_i64_rejects_a_non_numeric_string() {
    assert!(decimal_i64("segment_id", &json!("not a number")).is_err());
}

#[test]
fn decimal_i64_rejects_a_non_integer_value() {
    assert!(decimal_i64("segment_id", &json!(true)).is_err());
}

#[test]
fn decimal_u64_accepts_a_plain_number() {
    assert_eq!(decimal_u64("row_ordinal", &json!(7)), Ok(7));
}

#[test]
fn decimal_u64_accepts_a_decimal_string() {
    assert_eq!(
        decimal_u64("row_ordinal", &json!("18446744073709551615")),
        Ok(u64::MAX)
    );
}

#[test]
fn decimal_u64_rejects_a_negative_number() {
    assert!(decimal_u64("row_ordinal", &json!(-1)).is_err());
}

#[test]
fn decimal_u64_rejects_a_negative_string() {
    assert!(decimal_u64("row_ordinal", &json!("-1")).is_err());
}

#[test]
fn decimal_u32_accepts_a_plain_number() {
    assert_eq!(decimal_u32("type_id", &json!(1_100_001)), Ok(1_100_001));
}

#[test]
fn decimal_u32_accepts_a_decimal_string() {
    assert_eq!(decimal_u32("type_id", &json!("1100001")), Ok(1_100_001));
}

#[test]
fn decimal_u32_rejects_a_number_beyond_32_bits() {
    assert!(decimal_u32("type_id", &json!(u64::from(u32::MAX) + 1)).is_err());
}

#[test]
fn decimal_u32_rejects_a_negative_string() {
    assert!(decimal_u32("type_id", &json!("-1")).is_err());
}

#[test]
fn every_present_detail_text_has_one_stable_shape() {
    let mut activity = json!({
        "query": "select 'привет'",
        "state": "active",
    })
    .as_object()
    .expect("activity")
    .clone();
    normalize_detail_text("pg_stat_activity", &mut activity).expect("normalize activity");
    assert_eq!(
        activity["query"],
        json!({
            "stored_text": "select 'привет'",
            "full_len": "21",
            "truncated": false,
            "sha256": null,
        })
    );
    assert_eq!(activity["state"], "active");

    let mut error = json!({
        "sample": {
            "representation": "text",
            "stored_text": "stored prefix",
            "full_len": "9000",
            "truncated": true,
            "sha256": "abc123",
        },
        "detail": null,
        "pattern": "duplicate key value violates constraint ?",
    })
    .as_object()
    .expect("error")
    .clone();
    normalize_detail_text("pg_log_errors", &mut error).expect("normalize error");
    assert_eq!(
        error["sample"],
        json!({
            "stored_text": "stored prefix",
            "full_len": "9000",
            "truncated": true,
            "sha256": "abc123",
        })
    );
    assert_eq!(error["detail"], Value::Null);
    assert!(error["sample"].get("representation").is_none());
    assert_eq!(
        error["pattern"],
        "duplicate key value violates constraint ?"
    );
}
