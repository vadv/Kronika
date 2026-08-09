use kronika_reader::Cell;
use kronika_registry::ColumnClass;

use super::Accum;
use crate::Number;

#[test]
fn one_counter_reading_is_not_a_real_zero_interval() {
    let mut value = Accum::new(ColumnClass::Cumulative);
    value.observe(100, Some(&Cell::U64(0)));
    let summary = value.finish();
    assert_eq!(summary.count, 1);
    assert_eq!(summary.nonnegative_delta, None);
    assert_eq!(summary.observed_us, 0);
}

#[test]
fn two_equal_zero_readings_have_an_observed_interval() {
    let mut value = Accum::new(ColumnClass::Cumulative);
    value.observe(100, Some(&Cell::U64(0)));
    value.observe(250, Some(&Cell::U64(0)));
    let summary = value.finish();
    assert_eq!(summary.count, 2);
    assert_eq!(summary.nonnegative_delta, Some(Number::U64(0)));
    assert_eq!(summary.observed_us, 150);
}

#[test]
fn a_decreasing_counter_has_no_nonnegative_delta() {
    let mut value = Accum::new(ColumnClass::Cumulative);
    value.observe(100, Some(&Cell::U64(9)));
    value.observe(200, Some(&Cell::U64(2)));
    let summary = value.finish();
    assert_eq!(summary.count, 2);
    assert_eq!(summary.nonnegative_delta, None);
    assert_eq!(
        summary.first.map(|sample| sample.value),
        Some(Number::U64(9))
    );
    assert_eq!(
        summary.last.map(|sample| sample.value),
        Some(Number::U64(2))
    );
}

#[test]
fn counter_delta_uses_whole_interval_endpoints() {
    let mut value = Accum::new(ColumnClass::Cumulative);
    value.observe(100, Some(&Cell::U64(10)));
    value.observe(200, Some(&Cell::U64(5)));
    value.observe(300, Some(&Cell::U64(20)));
    let summary = value.finish();
    assert_eq!(summary.nonnegative_delta, Some(Number::U64(10)));
    assert_eq!(summary.observed_us, 200);
}

#[test]
fn a_nonfinite_sample_is_preserved_but_has_no_counter_delta() {
    let mut value = Accum::new(ColumnClass::Cumulative);
    value.observe(100, Some(&Cell::F64(f64::NAN)));
    value.observe(200, Some(&Cell::F64(f64::INFINITY)));
    let summary = value.finish();
    assert_eq!(summary.count, 2);
    assert!(matches!(
        summary.first.map(|sample| sample.value),
        Some(Number::F64(value)) if value.is_nan()
    ));
    assert!(matches!(
        summary.last.map(|sample| sample.value),
        Some(Number::F64(value)) if value == f64::INFINITY
    ));
    assert_eq!(summary.nonnegative_delta, None);
}

#[test]
fn an_absent_numeric_cell_has_no_sample() {
    let mut value = Accum::new(ColumnClass::Gauge);
    value.observe(100, Some(&Cell::Null));
    let summary = value.finish();
    assert_eq!(summary.count, 0);
    assert!(summary.first.is_none());
    assert!(summary.last.is_none());
}
