use super::TimeRange;

#[test]
fn range_validation_accepts_empty_and_tracks_exact_bounds() {
    let empty = TimeRange::new(7, 7).expect("empty range");
    assert_eq!((empty.from, empty.to_exclusive), (7, 7));

    let range = TimeRange::new(7, 9).expect("range");
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
