use kronika_reader::{Cell, Dictionary};
use kronika_registry::contract;
use serde_json::json;

use super::{cell, projected_layout};

#[test]
fn sixty_four_bit_values_are_decimal_strings() {
    assert_eq!(
        cell(&Cell::I64(i64::MIN), &Dictionary::default()).unwrap(),
        json!(i64::MIN.to_string())
    );
    assert_eq!(
        cell(&Cell::U64(u64::MAX), &Dictionary::default()).unwrap(),
        json!(u64::MAX.to_string())
    );
}

#[test]
fn finite_floats_are_numbers_and_non_finite_values_are_tagged() {
    assert_eq!(
        cell(&Cell::F64(1.25), &Dictionary::default()).unwrap(),
        json!(1.25)
    );
    assert_eq!(
        cell(&Cell::F64(f64::NAN), &Dictionary::default()).unwrap(),
        json!({"representation": "non_finite", "value": "nan"})
    );
    assert_eq!(
        cell(&Cell::F64(f64::INFINITY), &Dictionary::default()).unwrap(),
        json!({
            "representation": "non_finite",
            "value": "positive_infinity",
        })
    );
    assert_eq!(
        cell(&Cell::F64(f64::NEG_INFINITY), &Dictionary::default()).unwrap(),
        json!({
            "representation": "non_finite",
            "value": "negative_infinity",
        })
    );
}

#[test]
fn projected_layout_marks_layout_absent_fields_without_dropping_them() {
    let contract = contract(1_004_001).expect("known vadv layout");
    let userid = contract.column("userid");
    let value = projected_layout(
        "pg_store_plans",
        contract,
        &[("userid", userid), ("layout_specific_elsewhere", None)],
    );

    assert_eq!(value["columns"][0]["name"], "userid");
    assert_eq!(value["columns"][0]["type"], "u32");
    assert_eq!(value["columns"][0]["available"], true);
    assert_eq!(value["columns"][1]["name"], "layout_specific_elsewhere");
    assert_eq!(value["columns"][1]["available"], false);
    assert_eq!(
        value["columns"][1].as_object().map(serde_json::Map::len),
        Some(2)
    );
}
