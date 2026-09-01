use super::TimeRange;

#[test]
fn half_open_range_accepts_empty_and_excludes_end() {
    let empty = TimeRange::new(7, 7).expect("empty range");
    assert!(!empty.contains(7));

    let range = TimeRange::new(7, 9).expect("range");
    assert!(range.contains(7));
    assert!(range.contains(8));
    assert!(!range.contains(9));
    assert_eq!(range.width(), 2);
    assert_eq!(
        TimeRange::new(i64::MIN, i64::MAX)
            .expect("full range")
            .width(),
        i128::from(i64::MAX) - i128::from(i64::MIN)
    );
    assert_eq!(
        TimeRange::new(9, 7).expect_err("reversed range"),
        "from (9) must not be after to (7)"
    );
    assert!(TimeRange::bounded(7, 10, 2).is_err());
    assert_eq!(TimeRange::bounded(7, 9, 2).expect("bounded range"), range);
}
