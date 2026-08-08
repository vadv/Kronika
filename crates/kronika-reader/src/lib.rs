//! Reading a `Kronika` data directory back into rows.
//!
//! [`Reader::segments`] lists finished segments and the captured current
//! `active.wal` segment whose timestamps fall in a range;
//! [`Reader::open_segment`] opens either kind through the same row API.
//!
//! Nothing is cached between calls. A `Reader` that no one is asking holds an
//! open directory descriptor and nothing else.

mod dictionary;
mod error;
mod segment;

use std::ops::{Bound, RangeBounds};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kronika_store::{ActiveSnapshot, FinalUnit, LocalDir};

pub use dictionary::Dictionary;
pub use error::ReaderError;
pub use kronika_format::{BlobEntry, Resolved};
pub use kronika_registry::{Cell, Row};
pub use kronika_store::StoreWarning;
pub use segment::{Section, Segment};

#[derive(Debug, Clone)]
enum SegmentSource {
    Finished(FinalUnit),
    Active(ActiveSnapshot),
}

/// One segment captured by a directory listing.
///
/// The underlying source is deliberately opaque so finished `.zms` files and
/// the current `active.wal` prefix are opened through the same API.
#[derive(Debug, Clone)]
pub struct SegmentRef {
    source: SegmentSource,
    provenance: Arc<()>,
    segment_id: i64,
    min_ts: i64,
    max_ts: i64,
}

impl SegmentRef {
    /// Earliest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn min_ts(&self) -> i64 {
        self.min_ts
    }

    /// Latest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn max_ts(&self) -> i64 {
        self.max_ts
    }
}

/// What one directory scan found.
#[derive(Debug)]
pub struct Listing {
    /// Finished and current segments overlapping the requested range, oldest
    /// first.
    pub segments: Vec<SegmentRef>,
    /// Files the scan set aside, and why. Passing over a damaged segment
    /// without a word would report a quiet day instead of a broken one.
    pub warnings: Vec<StoreWarning>,
}

/// An open data directory.
#[derive(Debug)]
pub struct Reader {
    dir: LocalDir,
    root: PathBuf,
    provenance: Arc<()>,
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
            provenance: Arc::new(()),
        })
    }

    /// List finished segments and the captured current segment whose
    /// timestamps overlap `range`.
    ///
    /// The range is in unix microseconds, and `..` asks for everything. A
    /// segment covers an interval, so it is listed when any part of that
    /// interval falls inside the range.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory cannot be walked.
    pub fn segments<R: RangeBounds<i64>>(&self, range: R) -> Result<Listing, ReaderError> {
        let mut scan = self.dir.scan_catalogs()?;
        let active_id = scan.active.first().map(|part| part.segment_id);
        let finished_is_canonical = active_id.is_some_and(|active_id| {
            scan.finished
                .iter()
                .any(|unit| unit.address.id == active_id)
        });

        let active = if finished_is_canonical {
            None
        } else if let Some((min_ts, max_ts)) =
            active_bounds(&scan.active).filter(|&(min_ts, max_ts)| overlaps(&range, min_ts, max_ts))
        {
            self.dir
                .open_active_snapshot(&scan)?
                .map(|snapshot| (snapshot, min_ts, max_ts))
        } else {
            None
        };

        let mut segments = Vec::new();
        let finished = Arc::clone(&scan.finished);
        for unit in finished
            .iter()
            .filter(|unit| overlaps(&range, unit.summary.min_ts, unit.summary.max_ts))
        {
            if !self.dir.validate_finished(&mut scan, unit)? {
                continue;
            }
            segments.push(SegmentRef {
                source: SegmentSource::Finished(unit.clone()),
                provenance: Arc::clone(&self.provenance),
                segment_id: unit.address.id.get(),
                min_ts: unit.summary.min_ts,
                max_ts: unit.summary.max_ts,
            });
        }
        if let Some((snapshot, min_ts, max_ts)) = active {
            segments.push(SegmentRef {
                segment_id: snapshot.segment_id().get(),
                source: SegmentSource::Active(snapshot),
                provenance: Arc::clone(&self.provenance),
                min_ts,
                max_ts,
            });
        }
        segments.sort_by_key(|segment| segment.segment_id);
        Ok(Listing {
            segments,
            warnings: scan.warnings,
        })
    }

    /// Open one of the segments a listing returned.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is gone, changed under the listing, or
    /// its catalog is rejected. A reference returned by another reader is
    /// rejected as invalid input.
    pub fn open_segment(&self, unit: &SegmentRef) -> Result<Segment, ReaderError> {
        if !Arc::ptr_eq(&self.provenance, &unit.provenance) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "segment reference belongs to another reader",
            )
            .into());
        }
        Segment::open(&self.dir, &self.root, unit)
    }
}

fn active_bounds(parts: &[kronika_store::ActivePart]) -> Option<(i64, i64)> {
    if parts.is_empty() {
        return None;
    }
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    for part in parts {
        min_ts = min_ts.min(part.catalog.min_ts);
        max_ts = max_ts.max(part.catalog.max_ts);
    }
    if min_ts > max_ts {
        Some((0, 0))
    } else {
        Some((min_ts, max_ts))
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
