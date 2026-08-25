use super::DecimalI64;

#[test]
fn decimal_i64_serializes_as_a_json_string() {
    let value = serde_json::to_value(DecimalI64(9_007_199_254_740_993)).expect("serialize");
    assert_eq!(
        value,
        serde_json::Value::String("9007199254740993".to_string())
    );
}

#[test]
fn decimal_i64_round_trips_negative_values() {
    let value = serde_json::to_value(DecimalI64(-42)).expect("serialize");
    assert_eq!(value, serde_json::Value::String("-42".to_string()));
}
