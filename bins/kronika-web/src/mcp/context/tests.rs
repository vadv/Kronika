use super::exclusive_recorded_range;

#[test]
fn recorded_range_becomes_a_checked_half_open_range() {
    assert_eq!(exclusive_recorded_range(None), Ok(None));
    assert_eq!(
        exclusive_recorded_range(Some((100, 300))),
        Ok(Some((100, 301)))
    );
    assert_eq!(
        exclusive_recorded_range(Some((0, i64::MAX))),
        Err("last recorded timestamp cannot form an exclusive upper bound")
    );
}
