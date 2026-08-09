use kronika_registry::ColumnClass;

use super::{bucket_edges, bucket_of, identity_positions, numeric_column, scale, seconds_of};

/// `os_diskstats`: identity `major`,`minor`, and a long run of counters.
fn diskstats() -> &'static kronika_registry::TypeContract {
    super::contract_of(1_108_001).expect("the registry carries os_diskstats")
}

#[test]
fn a_column_is_found_where_the_index_wrote_it() {
    let (at, class, unit) = numeric_column(diskstats(), "reads").expect("reads is a counter");
    assert_eq!(at, 0, "reads is the first number of the section");
    assert_eq!(class, ColumnClass::Cumulative);
    assert_eq!(unit, "count/s", "a counter is answered as a rate");
}

#[test]
fn a_gauge_keeps_its_unit_as_it_stands() {
    let (_at, class, unit) =
        numeric_column(diskstats(), "io_in_progress").expect("io_in_progress is a gauge");
    assert_eq!(class, ColumnClass::Gauge);
    assert_eq!(unit, "count");
}

#[test]
fn a_label_is_not_a_column_to_order_by() {
    assert!(numeric_column(diskstats(), "device").is_none());
    assert!(numeric_column(diskstats(), "no_such_column").is_none());
}

#[test]
fn the_identity_is_found_among_the_labels_the_index_wrote() {
    // Labels in contract order: major, minor, device, scope.
    assert_eq!(identity_positions(diskstats()), vec![0, 1]);
}

#[test]
fn one_bucket_holds_everything() {
    assert_eq!(bucket_of((100, 200), 1, 100), 0);
    assert_eq!(bucket_of((100, 200), 1, 200), 0);
}

#[test]
fn a_segment_lands_in_the_bucket_its_end_falls_in() {
    let span = (0, 100);
    assert_eq!(bucket_of(span, 4, 0), 0);
    assert_eq!(bucket_of(span, 4, 24), 0);
    assert_eq!(bucket_of(span, 4, 25), 1);
    assert_eq!(bucket_of(span, 4, 99), 3);
}

#[test]
fn the_last_bucket_holds_the_end_of_the_window() {
    assert_eq!(bucket_of((0, 100), 4, 100), 3);
    assert_eq!(bucket_of((0, 100), 4, 10_000), 3, "past the end is the end");
}

#[test]
fn a_window_of_no_width_still_answers_one_bucket() {
    assert_eq!(bucket_of((50, 50), 3, 50), 0);
}

#[test]
fn the_edges_say_where_every_column_starts() {
    assert_eq!(bucket_edges((0, 100), 4), vec![0, 25, 50, 75]);
    assert_eq!(bucket_edges((1_000, 1_000), 1), vec![1_000]);
}

#[test]
fn a_bucket_lasts_as_long_as_the_segments_that_landed_in_it() {
    let observed = vec![3_000_000, 0, 1_500_000];
    assert!((seconds_of(&observed, 0) - 3.0).abs() < f64::EPSILON);
    assert!((seconds_of(&observed, 2) - 1.5).abs() < f64::EPSILON);
}

#[test]
fn a_bucket_no_segment_ended_in_holds_no_time() {
    let observed = vec![3_000_000, 0];
    assert!((seconds_of(&observed, 1) - 0.0).abs() < f64::EPSILON);
    assert!((seconds_of(&observed, 9) - 0.0).abs() < f64::EPSILON);
}

#[test]
fn a_counter_is_divided_by_the_time_it_ran_and_a_gauge_is_not() {
    assert!((scale(ColumnClass::Cumulative, 60.0, 2.0) - 30.0).abs() < f64::EPSILON);
    assert!((scale(ColumnClass::Gauge, 60.0, 2.0) - 60.0).abs() < f64::EPSILON);
}

#[test]
fn a_counter_over_no_time_is_left_as_it_is_rather_than_divided_by_zero() {
    assert!((scale(ColumnClass::Cumulative, 60.0, 0.0) - 60.0).abs() < f64::EPSILON);
}
