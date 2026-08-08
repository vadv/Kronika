use super::percent;

#[test]
fn a_share_of_nothing_is_zero_rather_than_a_division_by_zero() {
    assert_eq!(percent(0, 0), 0);
    assert_eq!(percent(5, 0), 0);
}

#[test]
fn a_share_rounds_to_the_nearest_whole_percent() {
    assert_eq!(percent(1, 3), 33);
    assert_eq!(percent(2, 3), 67);
    assert_eq!(percent(1, 1), 100);
}

#[test]
fn a_huge_part_does_not_overflow_the_multiplication() {
    assert_eq!(percent(u64::MAX, u64::MAX), 100);
    assert_eq!(percent(u64::MAX / 2, u64::MAX), 50);
}

#[test]
fn a_part_larger_than_the_whole_is_capped_at_a_hundred() {
    assert_eq!(percent(3, 2), 100);
}
