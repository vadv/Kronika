use std::cmp::Ordering;
use std::collections::BTreeMap;

use kronika_reader::{Cell, Row};
use kronika_registry::{TypeContract, contract};
use serde_json::{Value, json};

use super::{
    OrderedNumber, RankedRow, StagedRow, TopRows, available_field_index, compare_ordered,
    ordered_cell, rate,
};
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
    assert!(ordered_cell(&Cell::F64(f64::NAN)).is_none());
    assert!(ordered_cell(&Cell::F64(f64::INFINITY)).is_none());
}

fn ranked(
    contract: &'static TypeContract,
    ordinal: u64,
    value: Option<OrderedNumber>,
) -> RankedRow {
    RankedRow {
        staged: StagedRow {
            ordinal,
            row: Row::new(contract, Vec::new()),
            identity: Vec::new(),
        },
        value,
    }
}

fn reference_top(values: &[Option<OrderedNumber>], limit: usize) -> Vec<u64> {
    let mut rows = values
        .iter()
        .copied()
        .enumerate()
        .map(|(ordinal, value)| (u64::try_from(ordinal).expect("fixture ordinal"), value))
        .collect::<Vec<_>>();
    rows.sort_by(|(left_ordinal, left), (right_ordinal, right)| {
        compare_ordered(*right, *left).then_with(|| left_ordinal.cmp(right_ordinal))
    });
    rows.truncate(limit);
    rows.into_iter().map(|(ordinal, _value)| ordinal).collect()
}

fn bounded_top(
    contract: &'static TypeContract,
    values: &[Option<OrderedNumber>],
    limit: usize,
) -> (Vec<u64>, usize) {
    let mut rows = TopRows::new(limit);
    let mut peak = 0;
    for (ordinal, value) in values.iter().copied().enumerate() {
        rows.push(ranked(
            contract,
            u64::try_from(ordinal).expect("fixture ordinal"),
            value,
        ));
        peak = peak.max(rows.retained_len());
    }
    (
        rows.finish().into_iter().map(|row| row.ordinal).collect(),
        peak,
    )
}

#[test]
fn bounded_top_k_matches_full_sort_for_large_statement_and_plan_sections() {
    const ROWS: usize = 5_000;
    const LIMIT: usize = 200;
    for type_id in [1_002_006, 1_003_001] {
        let contract = contract(type_id).expect("fixture contract");
        let values = (0..ROWS)
            .map(|ordinal| {
                (ordinal % 37 != 0).then_some(OrderedNumber::Float(f64::from(
                    u32::try_from((ordinal * 7_919) % 997).expect("fixture value"),
                )))
            })
            .collect::<Vec<_>>();
        let expected = reference_top(&values, LIMIT);
        let (actual, peak) = bounded_top(contract, &values, LIMIT);
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), LIMIT);
        assert_eq!(peak, LIMIT);
    }
}

#[test]
fn bounded_top_k_orders_exact_large_integers_and_nulls_deterministically() {
    let contract = contract(1_002_006).expect("fixture contract");
    let base = i128::from(1_u64 << 53);
    let values = [
        None,
        Some(OrderedNumber::Integer(base)),
        Some(OrderedNumber::Integer(base + 1)),
        Some(OrderedNumber::Integer(base + 1)),
        Some(OrderedNumber::Integer(base - 1)),
    ];
    let (actual, peak) = bounded_top(contract, &values, 4);
    assert_eq!(actual, [2, 3, 1, 4]);
    assert_eq!(peak, 4);
    assert_eq!(actual, reference_top(&values, 4));
}
