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

    pub(crate) fn bounded(from: i64, to_exclusive: i64, max_width: i64) -> Result<Self, String> {
        let width = to_exclusive.checked_sub(from).ok_or_else(|| {
            format!("window is invalid: to ({to_exclusive}) minus from ({from}) overflows")
        })?;
        if width > max_width {
            return Err(format!(
                "window too wide: to - from is {width} microseconds, the limit is {max_width} microseconds"
            ));
        }
        Self::new(from, to_exclusive).map_err(|error| error.to_string())
    }

    pub(crate) const fn contains(self, timestamp: i64) -> bool {
        timestamp >= self.from && timestamp < self.to_exclusive
    }

    #[cfg(test)]
    pub(crate) const fn width(self) -> i128 {
        self.to_exclusive as i128 - self.from as i128
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotPoint {
    LatestRecorded,
    At(i64),
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
mod tests;
