use serde_json::json;

use super::{FilterInput, Op, build_search};
use crate::api::snapshot::search::{SearchOperator, SearchValue};

#[test]
fn a_single_equality_filter_builds_a_matching_predicate() {
    let filters = vec![FilterInput {
        field: "state".to_owned(),
        op: Op::Eq,
        value: json!("running"),
    }];
    let search = build_search("os_process", &filters)
        .expect("valid filter")
        .expect("non-empty filter list produces a search");
    assert!(search.matches_all(|clause| clause.key == "state"));
}

#[test]
fn an_unknown_field_for_the_section_is_rejected() {
    let filters = vec![FilterInput {
        field: "not_a_real_field".to_owned(),
        op: Op::Eq,
        value: json!("x"),
    }];
    let error = build_search("os_process", &filters).expect_err("unknown field");
    assert!(error.contains("not_a_real_field"));
}

#[test]
fn no_filters_produces_no_search() {
    assert!(
        build_search("os_process", &[])
            .expect("empty filters are valid")
            .is_none()
    );
}

#[test]
fn two_filters_are_anded_together() {
    // `pid` (an identifier field) cannot take `Gt` — comparison operators
    // only apply to quantity fields — so the second predicate here uses
    // `rss`, a real quantity field on `os_process`.
    let filters = vec![
        FilterInput {
            field: "state".to_owned(),
            op: Op::Eq,
            value: json!("running"),
        },
        FilterInput {
            field: "rss".to_owned(),
            op: Op::Gt,
            value: json!(1_000),
        },
    ];
    let search = build_search("os_process", &filters)
        .expect("valid filters")
        .expect("search");
    // Both clauses must be visited by matches_all's predicate — a pure AND
    // has exactly two Predicate leaves, no Or node.
    let mut seen = Vec::new();
    search.matches_all(|clause| {
        seen.push(clause.key);
        true
    });
    assert_eq!(seen.len(), 2);
}

#[test]
fn a_quantity_filter_builds_a_strict_greater_than_clause() {
    let filters = vec![FilterInput {
        field: "rss".to_owned(),
        op: Op::Gt,
        value: json!(1_048_576),
    }];
    let search = build_search("os_process", &filters)
        .expect("valid filter")
        .expect("search");
    let mut checked = false;
    search.matches_all(|clause| {
        assert_eq!(clause.key, "rss");
        assert_eq!(clause.operator, SearchOperator::Greater);
        assert!(matches!(clause.value, SearchValue::Quantity(_)));
        checked = true;
        true
    });
    assert!(checked);
}

#[test]
fn a_comparison_operator_on_a_string_field_is_rejected() {
    let filters = vec![FilterInput {
        field: "state".to_owned(),
        op: Op::Gt,
        value: json!("running"),
    }];
    let error = build_search("os_process", &filters).expect_err("comparison on a string field");
    assert!(error.contains("state"));
}

#[test]
fn contains_on_an_identifier_field_is_rejected() {
    let filters = vec![FilterInput {
        field: "pid".to_owned(),
        op: Op::Contains,
        value: json!(100),
    }];
    let error = build_search("os_process", &filters).expect_err("contains on an identifier field");
    assert!(error.contains("pid"));
}

#[test]
fn negation_is_rejected_for_every_field_kind() {
    let filters = vec![FilterInput {
        field: "state".to_owned(),
        op: Op::Ne,
        value: json!("running"),
    }];
    let error = build_search("os_process", &filters).expect_err("no negation primitive exists");
    assert!(error.contains("state"));
}

#[test]
fn inclusive_bounds_are_rejected_for_quantity_fields() {
    for op in [Op::Gte, Op::Lte] {
        let filters = vec![FilterInput {
            field: "rss".to_owned(),
            op,
            value: json!(1_000),
        }];
        let error = build_search("os_process", &filters).expect_err("no inclusive bound exists");
        assert!(error.contains("rss"));
    }
}

#[test]
fn a_negative_quantity_value_is_rejected() {
    let filters = vec![FilterInput {
        field: "rss".to_owned(),
        op: Op::Gt,
        value: json!(-1),
    }];
    let error = build_search("os_process", &filters)
        .expect_err("quantity fields take non-negative integers");
    assert!(error.contains("rss"));
}

#[test]
fn a_non_integer_quantity_value_is_rejected() {
    let filters = vec![FilterInput {
        field: "rss".to_owned(),
        op: Op::Gt,
        value: json!(1.5),
    }];
    let error =
        build_search("os_process", &filters).expect_err("quantity fields take whole numbers");
    assert!(error.contains("rss"));
}

#[test]
fn an_identifier_value_may_be_a_decimal_string() {
    let filters = vec![FilterInput {
        field: "pid".to_owned(),
        op: Op::Eq,
        value: json!("4242"),
    }];
    let search = build_search("os_process", &filters)
        .expect("valid filter")
        .expect("search");
    assert!(search.matches_all(|clause| matches!(
        &clause.value,
        SearchValue::Identifier(text) if text == "4242"
    )));
}
