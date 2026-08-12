use std::cmp::Ordering;
use std::collections::BTreeMap;

use kronika_reader::{Cell, Row};
use kronika_registry::contract;
use serde_json::{Value, json};

use super::{
    GlobPattern, PageOrderValue, PageRankedRow, PageRows, PageStagedRow, SnapshotCursor,
    available_field_index, compare_ordered, ordered_cell, rate, snapshot_binding,
};
use crate::api::query::OutputField;
use crate::route::{Filter, SnapshotRequest};

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
fn decreasing_and_missing_counters_have_no_rate() {
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
    let now = Cell::I64(10);
    assert_eq!(rate(Some(&now), None, COLUMN, Some(1_000_000)), Value::Null);
    assert_eq!(
        rate(Some(&now), Some(&BTreeMap::new()), COLUMN, Some(1_000_000)),
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
fn numeric_ordering_preserves_large_integer_precision_and_nulls() {
    let base = 1_u64 << 53;
    assert_eq!(
        compare_ordered(
            ordered_cell(&Cell::U64(base + 1)),
            ordered_cell(&Cell::U64(base)),
        ),
        Ordering::Greater
    );
    assert_eq!(
        compare_ordered(ordered_cell(&Cell::I64(-1)), ordered_cell(&Cell::I64(-2)),),
        Ordering::Greater
    );
    assert!(ordered_cell(&Cell::F64(f64::NAN)).is_none());
    assert!(ordered_cell(&Cell::F64(f64::INFINITY)).is_none());
}

fn ranked(layout_index: usize, ordinal: u64, value: Option<PageOrderValue>) -> PageRankedRow {
    ranked_for(1_002_006, layout_index, ordinal, value)
}

fn ranked_for(
    type_id: u32,
    layout_index: usize,
    ordinal: u64,
    value: Option<PageOrderValue>,
) -> PageRankedRow {
    let contract = contract(type_id).expect("fixture contract");
    PageRankedRow {
        staged: PageStagedRow {
            layout_index,
            ordinal,
            row: Row::new(contract, Vec::new()),
            identity: Vec::new(),
        },
        value,
    }
}

#[test]
fn five_thousand_statement_and_plan_candidates_keep_only_one_bounded_page() {
    const ROWS: usize = 5_000;
    const RETAINED: usize = 201;
    for type_id in [1_002_006, 1_003_001] {
        let values = (0..ROWS)
            .map(|ordinal| {
                (ordinal % 37 != 0).then_some(i128::try_from((ordinal * 7_919) % 997).unwrap())
            })
            .collect::<Vec<_>>();
        let mut expected = values
            .iter()
            .enumerate()
            .map(|(ordinal, value)| (ordinal, *value))
            .collect::<Vec<_>>();
        expected.sort_by(|(left_ordinal, left), (right_ordinal, right)| {
            right
                .cmp(left)
                .then_with(|| left_ordinal.cmp(right_ordinal))
        });
        expected.truncate(RETAINED);

        let mut page = PageRows::new(RETAINED);
        for (ordinal, value) in values.into_iter().enumerate() {
            page.push(ranked_for(
                type_id,
                0,
                u64::try_from(ordinal).unwrap(),
                value.map(PageOrderValue::Integer),
            ));
            assert!(page.retained_len() <= RETAINED);
        }
        let actual = page
            .finish()
            .into_iter()
            .map(|row| usize::try_from(row.staged.ordinal).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            expected
                .into_iter()
                .map(|(ordinal, _value)| ordinal)
                .collect::<Vec<_>>()
        );
        assert_eq!(actual.len(), RETAINED);
    }
}

#[test]
fn page_heap_is_bounded_and_ties_use_layout_then_ordinal() {
    let mut page = PageRows::new(3);
    for (layout, ordinal, value) in [(1, 1, 9), (0, 2, 9), (0, 1, 9), (0, 0, 8), (1, 0, 10)] {
        page.push(ranked(
            layout,
            ordinal,
            Some(PageOrderValue::Integer(value)),
        ));
        assert!(page.retained_len() <= 3);
    }
    let rows = page
        .finish()
        .into_iter()
        .map(|row| (row.staged.layout_index, row.staged.ordinal))
        .collect::<Vec<_>>();
    assert_eq!(rows, [(1, 0), (0, 1), (0, 2)]);
}

#[test]
fn integer_rate_ordering_cross_multiplies_elapsed_time_exactly() {
    let faster = ranked(
        0,
        0,
        Some(PageOrderValue::IntegerRate {
            delta: (1_i128 << 100) + 1,
            elapsed: 2,
        }),
    );
    let slower = ranked(
        0,
        1,
        Some(PageOrderValue::IntegerRate {
            delta: 1_i128 << 99,
            elapsed: 1,
        }),
    );
    assert_eq!(faster.cmp(&slower), Ordering::Greater);
}

#[test]
fn integer_ratio_ordering_is_exact_without_cross_product_overflow() {
    let larger = PageOrderValue::IntegerRatio {
        numerator: u128::MAX - 1,
        denominator: u128::MAX - 2,
    };
    let smaller = PageOrderValue::IntegerRatio {
        numerator: u128::MAX,
        denominator: u128::MAX - 1,
    };
    assert_eq!(
        super::compare_page_order_values(Some(&larger), Some(&smaller)),
        Ordering::Greater
    );

    let ratio_winner = PageOrderValue::IntegerRatio {
        numerator: 60,
        denominator: 2,
    };
    let raw_winner = PageOrderValue::IntegerRatio {
        numerator: 100,
        denominator: 10,
    };
    assert_eq!(
        super::compare_page_order_values(Some(&ratio_winner), Some(&raw_winner)),
        Ordering::Greater
    );

    for left_numerator in 0..30 {
        for left_denominator in 1..30 {
            for right_numerator in 0..30 {
                for right_denominator in 1..30 {
                    assert_eq!(
                        super::compare_u128_ratios(
                            left_numerator,
                            left_denominator,
                            right_numerator,
                            right_denominator,
                        ),
                        (left_numerator * right_denominator)
                            .cmp(&(right_numerator * left_denominator))
                    );
                }
            }
        }
    }
}

#[test]
fn cursor_round_trips_and_rejects_malformed_values() {
    let cursor = SnapshotCursor {
        segment_id: -4,
        active_position: 8,
        layout_index: 2,
        ordinal: 99,
        binding: u64::MAX,
    };
    assert_eq!(
        SnapshotCursor::parse(&cursor.encode()).expect("cursor"),
        cursor
    );
    for invalid in ["", "1,2,3,4", "1,2,3,4,5,6", "x,2,3,4,5"] {
        assert!(SnapshotCursor::parse(invalid).is_err());
    }
}

fn request() -> SnapshotRequest {
    SnapshotRequest {
        segment_id: 7,
        at: 11,
        sections: vec!["pg_stat_statements".to_owned()],
        fields: vec!["queryid".to_owned(), "query".to_owned()],
        by: vec!["calls".to_owned()],
        page_size: Some(200),
        cursor: None,
        search: vec!["needle*".to_owned()],
        text: Some(80),
        filters: vec![Filter {
            column: "dbid".to_owned(),
            value: "4".to_owned(),
        }],
        type_id: Some(1_002_006),
        row_ordinal: None,
    }
}

#[test]
fn cursor_binding_covers_query_shape_but_excludes_page_size_and_cursor() {
    let baseline = request();
    let expected = snapshot_binding(&baseline);
    let mut harmless = baseline.clone();
    harmless.page_size = Some(5_000);
    harmless.cursor = Some("opaque".to_owned());
    assert_eq!(snapshot_binding(&harmless), expected);

    let mut variants = Vec::new();
    let mut changed = baseline.clone();
    changed.segment_id += 1;
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.at += 1;
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.sections.push("other".to_owned());
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.fields.reverse();
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.by.push("rows".to_owned());
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.search.push("second".to_owned());
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.text = Some(81);
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.filters[0].value = "5".to_owned();
    variants.push(changed);
    let mut changed = baseline;
    changed.type_id = None;
    variants.push(changed);

    for changed in variants {
        assert_ne!(snapshot_binding(&changed), expected);
    }
}

#[test]
fn glob_supports_substrings_wildcards_literals_and_unicode_case() {
    for (pattern, candidate, matches) in [
        ("needle", "a NEEDLE here", true),
        ("a*c", "xxa/bbcyy", true),
        ("a?c", "xxaécyy", true),
        ("a?c", "xxaéécyy", false),
        ("select (x)+[y]", "SELECT (x)+[y]", true),
        ("*Σ?", "prefix σx", true),
        ("wanted", "unrelated", false),
    ] {
        assert_eq!(GlobPattern::new(pattern).matches(candidate), matches);
    }
}
