use std::collections::BTreeMap;

use super::{Counters, current_points, rate};

#[test]
fn counter_points_keep_unusable_subtractions_as_null_and_zero_as_data() {
    let stored = BTreeMap::from([(1, 10), (2, 10), (3, 5), (4, 12)]);
    assert_eq!(
        rate(&stored, |value, _seconds| value),
        vec![(1, None), (2, Some(0.0)), (3, None), (4, Some(7.0))]
    );
}

#[test]
fn a_segment_boundary_keeps_the_preceding_counter_reading() {
    let counters = Counters {
        busy_ticks: BTreeMap::from([(100, 10), (200, 20)]),
        ..Counters::default()
    };
    let current = current_points(&counters, 100, 1, 200, 200);
    let busy = current
        .iter()
        .find(|point| point.key == "cpu_busy")
        .expect("current busy point");
    assert_eq!(busy.ts, 200);
    assert_eq!(busy.value, Some(100_000.0));
}
