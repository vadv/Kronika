//! Time types shared by product queries.

/// A half-open timestamp range, `[from, to_exclusive)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TimeRange {
    pub(crate) from: i64,
    pub(crate) to_exclusive: i64,
}

impl TimeRange {
    pub(crate) const fn new(from: i64, to_exclusive: i64) -> Result<Self, ReversedTimeRange> {
        if from > to_exclusive {
            return Err(ReversedTimeRange { from, to_exclusive });
        }
        Ok(Self { from, to_exclusive })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReversedTimeRange {
    from: i64,
    to_exclusive: i64,
}

impl std::fmt::Display for ReversedTimeRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "from ({}) must not be after to ({})",
            self.from, self.to_exclusive
        )
    }
}

impl std::error::Error for ReversedTimeRange {}

#[cfg(test)]
mod tests {
    use super::TimeRange;

    #[test]
    fn half_open_range_accepts_empty_and_excludes_end() {
        let empty = TimeRange::new(7, 7).expect("empty range");
        assert_eq!((empty.from, empty.to_exclusive), (7, 7));

        let range = TimeRange::new(7, 9).expect("range");
        assert_eq!((range.from, range.to_exclusive), (7, 9));
        assert!(TimeRange::new(9, 7).is_err());
    }
}
