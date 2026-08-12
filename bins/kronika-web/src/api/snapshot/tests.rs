use std::cmp::Ordering;
use std::collections::BTreeMap;

use kronika_reader::Cell;
use serde_json::{Value, json};

use super::{available_field_index, compare_ordered, ordered_cell, rate};
use crate::api::query::OutputField;

const COLUMN: &str = "counter";

fn predecessor(value: Cell) -> BTreeMap<&'static str, Cell> {
    BTreeMap::from([(COLUMN, value)])
}

#[test]
fn integer_rates_subtract_exact_values_above_two_to_the_fifty_third() {
    let signed = 1_i64 << 53;
    let before = predecessor(Cell::I64(signed));
    assert_eq!(
        rate(
            Some(&Cell::I64(signed + 1)),
            Some(&before),
            COLUMN,
            Some(1_000_000),
        ),
        json!(1.0)
    );

    let unsigned = 1_u64 << 53;
    let before = predecessor(Cell::U64(unsigned));
    assert_eq!(
        rate(
            Some(&Cell::U64(unsigned + 1)),
            Some(&before),
            COLUMN,
            Some(1_000_000),
        ),
        json!(1.0)
    );
}

#[test]
fn floating_point_counters_have_their_own_delta_path() {
    let before = predecessor(Cell::F64(10.25));
    assert_eq!(
        rate(
            Some(&Cell::F64(10.75)),
            Some(&before),
            COLUMN,
            Some(500_000),
        ),
        json!(1.0)
    );
}

#[test]
fn decreasing_counters_have_no_rate() {
    for (now, earlier) in [
        (Cell::I64(9), Cell::I64(10)),
        (Cell::U64(9), Cell::U64(10)),
        (Cell::F64(9.0), Cell::F64(10.0)),
    ] {
        let before = predecessor(earlier);
        assert_eq!(
            rate(Some(&now), Some(&before), COLUMN, Some(1_000_000)),
            Value::Null
        );
    }
}

#[test]
fn a_missing_predecessor_has_no_rate() {
    let now = Cell::I64(10);
    assert_eq!(rate(Some(&now), None, COLUMN, Some(1_000_000)), Value::Null);
    assert_eq!(
        rate(Some(&now), Some(&BTreeMap::new()), COLUMN, Some(1_000_000),),
        Value::Null
    );
}

#[test]
fn ordering_uses_the_first_candidate_present_in_the_physical_layout() {
    let fields = [
        OutputField {
            name: "total_time".to_owned(),
            column: None,
        },
        OutputField {
            name: "total_exec_time".to_owned(),
            column: Some("total_exec_time"),
        },
        OutputField {
            name: "calls".to_owned(),
            column: Some("calls"),
        },
    ];
    assert_eq!(available_field_index(&fields, "total_time"), None);
    assert_eq!(
        ["total_time", "total_exec_time", "calls"]
            .iter()
            .find_map(|name| available_field_index(&fields, name)),
        Some(1)
    );
}

#[test]
fn ordering_keeps_integer_precision_above_two_to_the_fifty_third() {
    let base = 1_u64 << 53;
    assert_eq!(
        compare_ordered(
            ordered_cell(&Cell::U64(base + 1)),
            ordered_cell(&Cell::U64(base)),
        ),
        Ordering::Greater
    );
}
