use serde_json::json;

use super::{decimal_i64, decimal_u64};

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
