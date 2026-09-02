//! Time types shared by product queries.

/// A half-open timestamp range, `[from, to_exclusive)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TimeRange {
    pub(crate) from: i64,
    pub(crate) to_exclusive: i64,
}

impl TimeRange {
    pub(crate) fn new(from: i64, to_exclusive: i64) -> Result<Self, String> {
        if from > to_exclusive {
            return Err(format!(
                "from ({from}) must not be after to ({to_exclusive})"
            ));
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
        Self::new(from, to_exclusive)
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

#[cfg(test)]
mod tests;
