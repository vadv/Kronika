//! Reading a `Kronika` data directory back into rows.
//!
//! [`Reader::segments`] lists the finished segments whose timestamps fall in a
//! range, reading only catalogs; [`Reader::open_segment`] opens one of them and
//! hands out its sections as rows.
//!
//! Nothing is cached between calls. A `Reader` that no one is asking holds an
//! open directory descriptor and nothing else.

mod error;
mod segment;
mod strings;

use std::ops::{Bound, RangeBounds};
use std::path::{Path, PathBuf};

use kronika_store::LocalDir;

pub use error::ReaderError;
pub use kronika_registry::{Cell, Row};
pub use kronika_store::{FinalUnit, StoreWarning};
pub use segment::Segment;
pub use strings::Strings;

/// What one directory scan found.
#[derive(Debug)]
pub struct Listing {
    /// Finished segments overlapping the requested range, oldest first.
    pub segments: Vec<FinalUnit>,
    /// Files the scan set aside, and why. Passing over a damaged segment
    /// without a word would report a quiet day instead of a broken one.
    pub warnings: Vec<StoreWarning>,
}

/// An open data directory.
#[derive(Debug)]
pub struct Reader {
    dir: LocalDir,
    root: PathBuf,
}

impl Reader {
    /// Open `root` as a data directory.
    ///
    /// Only the directory descriptor is opened here; nothing is read until
    /// [`segments`](Self::segments).
    ///
    /// # Errors
    ///
    /// Returns an I/O error when `root` is not a directory or cannot be
    /// accessed.
    pub fn open(root: &Path) -> Result<Self, ReaderError> {
        Ok(Self {
            dir: LocalDir::open(root)?,
            root: root.to_path_buf(),
        })
    }

    /// List the finished segments whose timestamps overlap `range`.
    ///
    /// The range is in unix microseconds, and `..` asks for everything. A
    /// segment covers an interval, so it is listed when any part of that
    /// interval falls inside the range.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory cannot be walked.
    pub fn segments<R: RangeBounds<i64>>(&self, range: R) -> Result<Listing, ReaderError> {
        let scan = self.dir.scan()?;
        Ok(Listing {
            segments: scan
                .finished
                .iter()
                .filter(|unit| overlaps(&range, unit.summary.min_ts, unit.summary.max_ts))
                .cloned()
                .collect(),
            warnings: scan.warnings,
        })
    }

    /// Open one of the segments a listing returned.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is gone, changed under the listing, or
    /// its catalog is rejected.
    pub fn open_segment(&self, unit: &FinalUnit) -> Result<Segment, ReaderError> {
        Segment::open(&self.dir, &self.root, unit)
    }
}

/// Whether a segment covering `[min_ts, max_ts]` has anything inside `range`.
///
/// Timestamps are whole microseconds, so an excluded bound moves one
/// microsecond inwards and both ends become inclusive. An empty range then
/// yields no instants at all and matches nothing.
fn overlaps<R: RangeBounds<i64>>(range: &R, min_ts: i64, max_ts: i64) -> bool {
    let start = match range.start_bound() {
        Bound::Unbounded => i64::MIN,
        Bound::Included(start) => *start,
        Bound::Excluded(start) => {
            let Some(start) = start.checked_add(1) else {
                return false;
            };
            start
        }
    };
    let end = match range.end_bound() {
        Bound::Unbounded => i64::MAX,
        Bound::Included(end) => *end,
        Bound::Excluded(end) => {
            let Some(end) = end.checked_sub(1) else {
                return false;
            };
            end
        }
    };
    min_ts.max(start) <= max_ts.min(end)
}

#[cfg(test)]
mod tests;
