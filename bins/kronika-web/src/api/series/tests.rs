use super::rates;

/// One second in the microseconds the timestamps use.
const SECOND: i64 = 1_000_000;

#[test]
fn a_counter_becomes_the_rate_between_two_readings() {
    let points = rates(&[(0, Some(100.0)), (2 * SECOND, Some(160.0))]);
    assert_eq!(points.len(), 1, "two readings make one rate");
    assert_eq!(
        points[0].0,
        2 * SECOND,
        "a rate is stamped at its later end"
    );
    assert!((points[0].1.expect("a rate") - 30.0).abs() < f64::EPSILON);
}

#[test]
fn one_reading_alone_makes_no_point() {
    assert!(rates(&[(0, Some(100.0))]).is_empty());
    assert!(rates(&[]).is_empty());
}

#[test]
fn a_counter_that_went_backwards_has_no_rate() {
    let points = rates(&[(0, Some(900.0)), (SECOND, Some(100.0))]);
    assert_eq!(points[0].1, None);
}

#[test]
fn a_reading_the_segment_did_not_hold_breaks_the_pair() {
    let points = rates(&[(0, Some(10.0)), (SECOND, None), (2 * SECOND, Some(50.0))]);
    assert_eq!(points[0].1, None, "no reading, no rate");
    assert_eq!(points[1].1, None, "nothing to subtract from either");
}

#[test]
fn two_readings_at_the_same_instant_make_no_rate() {
    let points = rates(&[(SECOND, Some(10.0)), (SECOND, Some(20.0))]);
    assert_eq!(points[0].1, None, "no time passed to divide by");
}

#[test]
fn a_counter_that_stood_still_reads_as_zero_rather_than_nothing() {
    let points = rates(&[(0, Some(42.0)), (SECOND, Some(42.0))]);
    assert_eq!(points[0].1, Some(0.0));
}
