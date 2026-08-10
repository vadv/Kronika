use super::{Column, ColumnClass, ColumnType, LintError, Semantics, TypeContract, Unit, lint};
use crate::TypeId;

const TS: Column = Column {
    name: "ts",
    ty: ColumnType::Ts,
    class: ColumnClass::Timestamp,
    nullable: false,
    unit: None,
};
const VALUE: Column = Column {
    name: "value",
    ty: ColumnType::I64,
    class: ColumnClass::Cumulative,
    nullable: false,
    unit: Some(Unit::Count),
};

fn contract(
    type_id: u32,
    columns: &'static [Column],
    sort_key: &'static [&'static str],
) -> TypeContract {
    TypeContract {
        type_id: TypeId::new(type_id).expect("test type_id must be valid"),
        name: "test",
        semantics: Semantics::SnapshotFull,
        columns,
        sort_key,
        identity: &[],
        deprecated: false,
    }
}

#[test]
fn rejects_a_counter_without_a_unit() {
    const UNITLESS: Column = Column {
        name: "value",
        ty: ColumnType::I64,
        class: ColumnClass::Cumulative,
        nullable: false,
        unit: None,
    };
    let c = contract(1_006_001, &[TS, UNITLESS], &["ts"]);
    assert_eq!(
        lint(&[c]),
        Err(vec![LintError::MissingUnit {
            type_id: 1_006_001,
            column: "value",
        }])
    );
}

#[test]
fn accepts_a_valid_contract() {
    let c = contract(1_006_001, &[TS, VALUE], &["ts"]);
    assert_eq!(lint(&[c]), Ok(()));
}

#[test]
fn an_empty_registry_lints_clean() {
    assert_eq!(lint(&[]), Ok(()));
}

#[test]
fn rejects_duplicate_ids() {
    let a = contract(1_006_001, &[TS], &["ts"]);
    let b = contract(1_006_001, &[TS], &["ts"]);
    assert_eq!(
        lint(&[a, b]),
        Err(vec![LintError::DuplicateTypeId { type_id: 1_006_001 }])
    );
}

#[test]
fn rejects_sort_key_that_is_not_a_column() {
    let c = contract(1_006_001, &[TS], &["pid"]);
    assert_eq!(
        lint(&[c]),
        Err(vec![LintError::SortKeyColumnMissing {
            type_id: 1_006_001,
            column: "pid"
        }])
    );
}

#[test]
fn rejects_identity_that_is_not_a_column() {
    let c = TypeContract {
        identity: &["pid"],
        ..contract(1_006_001, &[TS, VALUE], &["ts"])
    };
    assert_eq!(
        lint(&[c]),
        Err(vec![LintError::IdentityColumnMissing {
            type_id: 1_006_001,
            column: "pid"
        }])
    );
}

#[test]
fn rejects_identity_column_that_is_not_a_label() {
    // `value` is a Cumulative column, not a Label.
    let c = TypeContract {
        identity: &["value"],
        ..contract(1_006_001, &[TS, VALUE], &["ts"])
    };
    assert_eq!(
        lint(&[c]),
        Err(vec![LintError::IdentityColumnNotLabel {
            type_id: 1_006_001,
            column: "value"
        }])
    );
}

#[test]
fn rejects_changed_type_without_baseline() {
    let mut c = contract(1_002_001, &[TS, VALUE], &["ts"]);
    c.semantics = Semantics::Changed;
    assert_eq!(
        lint(&[c]),
        Err(vec![LintError::MissingBaseline { type_id: 1_002_001 }])
    );
}
