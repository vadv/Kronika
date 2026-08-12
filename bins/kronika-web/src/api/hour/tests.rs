use super::{HOUR, hours_of_ranges, latest_hour, overlaps_window};
use crate::route::Window;

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

#[test]
fn selected_segments_use_inclusive_window_bounds() {
    let window = Window {
        from: Some(100),
        to: Some(200),
    };
    assert!(!overlaps_window(0, 99, window));
    assert!(overlaps_window(0, 100, window));
    assert!(overlaps_window(150, 160, window));
    assert!(overlaps_window(200, 300, window));
    assert!(!overlaps_window(201, 300, window));
    assert!(overlaps_window(0, 300, window));
    assert!(overlaps_window(100, 200, Window::default()));
}
