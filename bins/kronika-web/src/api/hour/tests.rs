use super::{HOUR, hours_of_ranges, latest_hour};

#[test]
fn long_segments_include_every_intersected_hour() {
    assert_eq!(
        hours_of_ranges([(HOUR + 1, 3 * HOUR + 1), (2 * HOUR, 2 * HOUR)]),
        [HOUR, 2 * HOUR, 3 * HOUR],
    );
}

#[test]
fn exact_and_maximum_boundaries_are_safe() {
    let maximum_hour = i64::MAX.div_euclid(HOUR) * HOUR;
    assert_eq!(hours_of_ranges([(HOUR, HOUR)]), [HOUR]);
    assert_eq!(hours_of_ranges([(i64::MAX, i64::MAX)]), [maximum_hour]);
    assert_eq!(latest_hour(&[maximum_hour]).to, Some(i64::MAX));
    assert!(hours_of_ranges([(3, 2)]).is_empty());
}
