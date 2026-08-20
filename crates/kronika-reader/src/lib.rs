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

use kronika_format::Catalog;
use kronika_store::{ActiveSnapshot, FinalUnit, LocalDir, read_catalog};

pub use dictionary::Dictionary;
pub use error::ReaderError;
pub use kronika_format::{BlobEntry, Resolved, StrId};
pub use kronika_registry::{Cell, Row};
pub use kronika_store::{StoreObject, StoreWarning, StoreWarningReason};
pub use segment::{Section, Segment};

/// Whether a listed segment is immutable or the captured journal prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    /// An immutable `.zms` file.
    Finished,
    /// The committed prefix of `active.wal` captured by the listing.
    Active,
}

/// One physical section actually present in a listed segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentSection {
    /// Exact physical registry layout.
    pub type_id: u32,
    /// Rows recorded by the segment catalog.
    pub rows: u64,
    /// Compressed section body bytes.
    pub bytes: u64,
}

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
    captured_bytes: u64,
    sections: Arc<[SegmentSection]>,
}

impl SegmentRef {
    /// Stable segment id: unix microseconds of its first appended window.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.segment_id
    }

    /// Whether this reference names an immutable file or captured live data.
    #[must_use]
    pub const fn kind(&self) -> SegmentKind {
        match self.source {
            SegmentSource::Finished(_) => SegmentKind::Finished,
            SegmentSource::Active(_) => SegmentKind::Active,
        }
    }

    /// Exact committed journal-prefix position for an active reference.
    ///
    /// Finished segments have no journal position and return `None`.
    #[must_use]
    pub const fn active_position(&self) -> Option<u64> {
        match self.source {
            SegmentSource::Finished(_) => None,
            SegmentSource::Active(_) => Some(self.captured_bytes),
        }
    }

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

    /// Physical sections actually present, in numeric layout order.
    #[must_use]
    pub fn sections(&self) -> &[SegmentSection] {
        &self.sections
    }

    /// Pin an active reference to an earlier committed cursor position.
    ///
    /// Finished references and positions that are not complete frame
    /// boundaries are rejected.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input or store framing error for an unusable
    /// position.
    pub fn at_active_position(&self, position: u64) -> Result<Self, ReaderError> {
        let SegmentSource::Active(snapshot) = &self.source else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a finished segment has no active position",
            )
            .into());
        };
        let snapshot = snapshot.at_position(position)?;
        let (min_ts, max_ts) = active_bounds(snapshot.parts()).unwrap_or((0, 0));
        let sections = sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
        Ok(Self {
            source: SegmentSource::Active(snapshot),
            provenance: Arc::clone(&self.provenance),
            segment_id: self.segment_id,
            min_ts,
            max_ts,
            captured_bytes: position,
            sections,
        })
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
        self.list_segments(range, true, false)
    }

    /// List segment catalogs without reading finished section bodies.
    ///
    /// This narrow discovery path lets a caller validate and use an existing
    /// derived index without paying to checksum every source section first.
    /// Opening rows through [`open_segment`](Self::open_segment) still verifies
    /// each selected body before decoding it.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory or a segment catalog cannot be
    /// read safely.
    pub fn catalog_segments<R: RangeBounds<i64>>(&self, range: R) -> Result<Listing, ReaderError> {
        self.list_segments(range, false, false)
    }

    /// List segment catalogs in `range` plus the closest finished predecessor.
    ///
    /// The predecessor is selected from scan summaries before section catalogs
    /// are opened. This supports bounded counter lookback without opening every
    /// older segment merely to locate the adjacent sample.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory or a selected segment catalog
    /// cannot be read safely.
    pub fn catalog_segments_with_predecessor<R: RangeBounds<i64>>(
        &self,
        range: R,
    ) -> Result<Listing, ReaderError> {
        self.list_segments(range, false, true)
    }

    fn list_segments<R: RangeBounds<i64>>(
        &self,
        range: R,
        validate_bodies: bool,
        include_predecessor: bool,
    ) -> Result<Listing, ReaderError> {
        let mut scan = self.dir.scan_catalogs()?;
        let mut segments = Vec::new();
        let finished = Arc::clone(&scan.finished);
        let predecessor = include_predecessor
            .then(|| {
                finished
                    .iter()
                    .filter(|unit| before_start(&range, unit.summary.max_ts))
                    .max_by_key(|unit| (unit.summary.max_ts, unit.address.id))
                    .map(|unit| unit.address.id)
            })
            .flatten();
        for unit in finished.iter().filter(|unit| {
            overlaps(&range, unit.summary.min_ts, unit.summary.max_ts)
                || predecessor == Some(unit.address.id)
        }) {
            if validate_bodies && !self.dir.validate_finished(&mut scan, unit)? {
                continue;
            }
            let file = self.dir.open_finished(unit)?;
            let catalog = read_catalog(&file)?;
            self.dir.validate_finished_file(&file, unit)?;
            let sections = sections_of(std::iter::once(&catalog)).into();
            segments.push(SegmentRef {
                source: SegmentSource::Finished(unit.clone()),
                provenance: Arc::clone(&self.provenance),
                segment_id: unit.address.id.get(),
                min_ts: unit.summary.min_ts,
                max_ts: unit.summary.max_ts,
                captured_bytes: unit.identity.len,
                sections,
            });
        }
        let active_id = scan.active.first().map(|part| part.segment_id.get());
        let finished_is_canonical = active_id.is_some_and(|active_id| {
            segments
                .iter()
                .any(|segment| segment.segment_id == active_id)
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
        if let Some((snapshot, min_ts, max_ts)) = active {
            let sections = sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
            segments.push(SegmentRef {
                segment_id: snapshot.segment_id().get(),
                source: SegmentSource::Active(snapshot),
                provenance: Arc::clone(&self.provenance),
                min_ts,
                max_ts,
                captured_bytes: scan.valid_len,
                sections,
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

#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
fn sections_of<'a>(catalogs: impl IntoIterator<Item = &'a Catalog>) -> Vec<SegmentSection> {
    let mut sections = std::collections::BTreeMap::<u32, SegmentSection>::new();
    for catalog in catalogs {
        for entry in &catalog.entries {
            let section = sections.entry(entry.type_id).or_insert(SegmentSection {
                type_id: entry.type_id,
                rows: 0,
                bytes: 0,
            });
            section.rows = section.rows.saturating_add(u64::from(entry.rows));
            section.bytes = section.bytes.saturating_add(entry.len);
        }
    }
    sections.into_values().collect()
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

fn before_start<R: RangeBounds<i64>>(range: &R, max_ts: i64) -> bool {
    match range.start_bound() {
        Bound::Unbounded => false,
        Bound::Included(start) => max_ts < *start,
        Bound::Excluded(start) => max_ts <= *start,
    }
}

#[cfg(test)]
mod tests;
