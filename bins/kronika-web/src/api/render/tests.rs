use kronika_index::{IdentityValue, Number, Observation, Sample};
use kronika_reader::{Cell, Dictionary};
use kronika_registry::contract;
use serde_json::json;

use super::{cell, identity, number, observation, projected_layout};

#[test]
fn sixty_four_bit_values_are_decimal_strings() {
    assert_eq!(number(Number::I64(i64::MIN)), json!(i64::MIN.to_string()));
    assert_eq!(number(Number::U64(u64::MAX)), json!(u64::MAX.to_string()));
    assert_eq!(
        identity(&IdentityValue::Ts(i64::MAX)),
        json!(i64::MAX.to_string())
    );
    assert_eq!(
        cell(&Cell::U64(u64::MAX), &Dictionary::default()).unwrap(),
        json!(u64::MAX.to_string())
    );
}

#[test]
fn finite_floats_are_numbers_and_non_finite_values_are_tagged() {
    assert_eq!(number(Number::F64(1.25)), json!(1.25));
    assert_eq!(
        number(Number::F64(f64::NAN)),
        json!({"representation": "non_finite", "value": "nan"})
    );
    assert_eq!(
        number(Number::F64(f64::INFINITY)),
        json!({
            "representation": "non_finite",
            "value": "positive_infinity",
        })
    );
    assert_eq!(
        number(Number::F64(f64::NEG_INFINITY)),
        json!({
            "representation": "non_finite",
            "value": "negative_infinity",
        })
    );
}

#[test]
fn binary_identities_and_blobs_keep_exact_bytes_and_metadata() {
    assert_eq!(
        identity(&IdentityValue::Text(vec![0xff, 0x00])),
        json!({"representation": "bytes", "base64": "/wA="})
    );
    assert_eq!(
        identity(&IdentityValue::Blob {
            stored_bytes: vec![0xff, 0x00],
            full_len: 9_007_199_254_740_993,
            truncated: true,
            full_sha256: Some([0xab; 32]),
        }),
        json!({
            "representation": "bytes",
            "stored_base64": "/wA=",
            "full_len": "9007199254740993",
            "truncated": true,
            "sha256": "abababababababababababababababababababababababababababababababab",
        })
    );
}

#[test]
fn observation_counts_timestamps_deltas_and_durations_are_js_safe() {
    let value = observation(Observation {
        count: u64::MAX,
        first: Some(Sample {
            ts: i64::MIN,
            value: Number::I64(i64::MIN),
        }),
        last: Some(Sample {
            ts: i64::MAX,
            value: Number::U64(u64::MAX),
        }),
        nonnegative_delta: Some(Number::U64(u64::MAX)),
        observed_us: u64::MAX,
    });

    assert_eq!(value["count"], u64::MAX.to_string());
    assert_eq!(value["first"]["ts"], i64::MIN.to_string());
    assert_eq!(value["first"]["value"], i64::MIN.to_string());
    assert_eq!(value["last"]["ts"], i64::MAX.to_string());
    assert_eq!(value["last"]["value"], u64::MAX.to_string());
    assert_eq!(value["nonnegative_delta"], u64::MAX.to_string());
    assert_eq!(value["observed_us"], u64::MAX.to_string());
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
