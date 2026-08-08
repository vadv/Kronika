use super::{Stall, health};

/// One second, in the microseconds the counters use.
const SECOND: i64 = 1_000_000;

const fn stall(cpu: i64, memory: i64, io: i64) -> Stall {
    Stall { cpu, memory, io }
}

const ZERO: Stall = stall(0, 0, 0);

#[test]
fn an_idle_interval_is_a_hundred() {
    assert_eq!(health(ZERO, 0, ZERO, SECOND), Some(100));
}

#[test]
fn a_resource_stalled_the_whole_interval_is_zero() {
    assert_eq!(health(ZERO, 0, stall(SECOND, 0, 0), SECOND), Some(0));
}

#[test]
fn the_worst_resource_decides_and_the_others_do_not_dilute_it() {
    // An idle CPU must not hide a disk that waited for four fifths of the time.
    let after = stall(0, 0, SECOND / 5 * 4);
    assert_eq!(health(ZERO, 0, after, SECOND), Some(20));
}

#[test]
fn each_resource_counts_the_same_way() {
    let quarter = SECOND / 4;
    assert_eq!(health(ZERO, 0, stall(quarter, 0, 0), SECOND), Some(75));
    assert_eq!(health(ZERO, 0, stall(0, quarter, 0), SECOND), Some(75));
    assert_eq!(health(ZERO, 0, stall(0, 0, quarter), SECOND), Some(75));
}

#[test]
fn counters_carry_their_history_so_only_the_delta_matters() {
    let before = stall(700 * SECOND, 12 * SECOND, 5 * SECOND);
    let after = stall(700 * SECOND + SECOND / 2, 12 * SECOND, 5 * SECOND);
    assert_eq!(health(before, 0, after, SECOND), Some(50));
}

#[test]
fn a_counter_that_went_backwards_has_no_health() {
    let before = stall(SECOND, SECOND, SECOND);
    assert_eq!(health(before, 0, ZERO, SECOND), None);
    assert_eq!(health(before, 0, stall(SECOND, 0, SECOND), SECOND), None);
}

#[test]
fn a_zero_or_negative_interval_has_no_health() {
    assert_eq!(health(ZERO, SECOND, ZERO, SECOND), None);
    assert_eq!(health(ZERO, SECOND, ZERO, 0), None);
}

#[test]
fn stalling_longer_than_the_interval_is_zero_not_below() {
    // Several tasks waiting at once bill more microseconds than elapsed.
    let after = stall(0, 0, 5 * SECOND);
    assert_eq!(health(ZERO, 0, after, SECOND), Some(0));
}

#[test]
fn a_share_rounds_to_the_nearest_whole_percent() {
    // 0.4% and 0.6% of the interval sit either side of half a percent.
    assert_eq!(health(ZERO, 0, stall(4_000, 0, 0), SECOND), Some(100));
    assert_eq!(health(ZERO, 0, stall(6_000, 0, 0), SECOND), Some(99));
}

#[test]
fn a_long_interval_does_not_overflow_the_multiplication() {
    let day = 86_400 * SECOND;
    assert_eq!(health(ZERO, 0, stall(day / 2, 0, 0), day), Some(50));
    assert_eq!(health(ZERO, 0, stall(i64::MAX, 0, 0), day), Some(0));
}
