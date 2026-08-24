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

use std::cmp::Reverse;
use std::ops::{Bound, RangeBounds};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kronika_format::Catalog;
use kronika_registry::{ColumnClass, contract, logical_section_name, registry};
use kronika_store::{ActiveSnapshot, CatalogInventory, FinalUnit, LocalDir, read_catalog};

#[cfg(test)]
std::thread_local! {
    static SNAPSHOT_FINISHED_CATALOG_READS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

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

/// The real rows selected for one logical-section snapshot.
#[derive(Debug)]
pub struct SnapshotSelection {
    /// A segment contributing the latest section sample at or before the
    /// requested instant.
    pub anchor: Option<SegmentRef>,
    /// Other segments and physical layouts contributing the current sample,
    /// newest first.
    pub current_segments: Vec<SegmentRef>,
    /// Segments contributing the immediately preceding real section sample,
    /// newest first. A segment that carries both retained moments appears only
    /// in `current_segments`.
    pub predecessor_segments: Vec<SegmentRef>,
    /// Files the scan set aside, and why.
    pub warnings: Vec<StoreWarning>,
}

/// One catalog-only store scan whose full section catalogs remain unopened.
///
/// A caller can inspect all recorded time ranges, choose a window, and then
/// materialize references only for segments that overlap that window.
#[derive(Debug)]
pub struct CatalogDiscovery<'a> {
    reader: &'a Reader,
    scan: kronika_store::LocalScan,
}

impl CatalogDiscovery<'_> {
    /// Time bounds of every canonical segment found by the scan.
    pub fn ranges(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        let active_id = self.scan.active.first().map(|part| part.segment_id.get());
        let finished_is_canonical = active_id.is_some_and(|active_id| {
            self.scan
                .finished
                .iter()
                .any(|unit| unit.address.id.get() == active_id)
        });
        let active = (!finished_is_canonical)
            .then(|| active_bounds(&self.scan.active))
            .flatten();
        self.scan
            .finished
            .iter()
            .map(|unit| (unit.summary.min_ts, unit.summary.max_ts))
            .chain(active)
    }

    /// Open section catalogs only for segments overlapping `range`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when a selected segment changed or its catalog
    /// cannot be read safely.
    pub fn segments<R: RangeBounds<i64>>(self, range: R) -> Result<Listing, ReaderError> {
        self.list_segments(range, false, false)
    }

    fn list_segments<R: RangeBounds<i64>>(
        mut self,
        range: R,
        validate_bodies: bool,
        include_predecessor: bool,
    ) -> Result<Listing, ReaderError> {
        let mut segments = Vec::new();
        let finished = Arc::clone(&self.scan.finished);
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
            if validate_bodies && !self.reader.dir.validate_finished(&mut self.scan, unit)? {
                continue;
            }
            let file = self.reader.dir.open_finished(unit)?;
            let catalog = read_catalog(&file)?;
            self.reader.dir.validate_finished_file(&file, unit)?;
            let sections = sections_of(std::iter::once(&catalog)).into();
            segments.push(SegmentRef {
                source: SegmentSource::Finished(unit.clone()),
                provenance: Arc::clone(&self.reader.provenance),
                segment_id: unit.address.id.get(),
                min_ts: unit.summary.min_ts,
                max_ts: unit.summary.max_ts,
                captured_bytes: unit.identity.len,
                sections,
            });
        }
        let active_id = self.scan.active.first().map(|part| part.segment_id.get());
        let finished_is_canonical = active_id.is_some_and(|active_id| {
            segments
                .iter()
                .any(|segment| segment.segment_id == active_id)
        });
        let active = if finished_is_canonical {
            None
        } else if let Some((min_ts, max_ts)) = active_bounds(&self.scan.active)
            .filter(|&(min_ts, max_ts)| overlaps(&range, min_ts, max_ts))
        {
            self.reader
                .dir
                .open_active_snapshot(&self.scan)?
                .map(|snapshot| (snapshot, min_ts, max_ts))
        } else {
            None
        };
        if let Some((snapshot, min_ts, max_ts)) = active {
            let sections = sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
            segments.push(SegmentRef {
                segment_id: snapshot.segment_id().get(),
                source: SegmentSource::Active(snapshot),
                provenance: Arc::clone(&self.reader.provenance),
                min_ts,
                max_ts,
                captured_bytes: self.scan.valid_len,
                sections,
            });
        }
        segments.sort_by_key(|segment| segment.segment_id);
        Ok(Listing {
            segments,
            warnings: self.scan.warnings,
        })
    }
}

fn select_snapshot(
    reader: &Reader,
    hour_start: i64,
    at: i64,
    logical_section: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<SnapshotSelection, ReaderError> {
    if cancelled() {
        return Err(cancelled_snapshot_read());
    }
    if hour_start > at {
        return Ok(empty_snapshot_selection(Vec::new()));
    }

    let type_ids = registry()
        .iter()
        .filter(|contract| logical_section_name(contract.type_id.get()) == Some(logical_section))
        .map(|contract| contract.type_id.get())
        .collect::<Vec<_>>();
    let mut inventory = reader.dir.catalog_inventory(cancelled)?;
    if type_ids.is_empty() {
        return Ok(empty_snapshot_selection(inventory.warnings));
    }

    let active_id = inventory.active.first().map(|part| part.segment_id.get());
    let finished_is_canonical = active_id.is_some_and(|active_id| {
        inventory
            .finished
            .iter()
            .any(|artifact| artifact.address.id.get() == active_id)
    });
    let mut retained = Vec::<SnapshotContribution>::new();
    let mut moments = SnapshotMoments::default();

    if !finished_is_canonical
        && active_bounds(&inventory.active).is_some_and(|(min_ts, _max_ts)| min_ts <= at)
        && let Some(reference) = active_inventory_reference(reader, &inventory)?
    {
        record_snapshot_contribution(
            reader,
            reference,
            logical_section,
            at,
            cancelled,
            &mut moments,
            &mut retained,
        )?;
    }

    for index in (0..inventory.finished.len()).rev() {
        if cancelled() {
            return Err(cancelled_snapshot_read());
        }
        let artifact = inventory.finished[index];
        note_snapshot_finished_catalog_read();
        let Some(unit) = reader
            .dir
            .read_finished_catalog_summary(&mut inventory, artifact)?
        else {
            continue;
        };
        if moments
            .previous
            .is_some_and(|previous| unit.summary.max_ts < previous)
        {
            break;
        }
        if unit.summary.min_ts > at || !unit.summary.may_contain_any_nonempty_type(&type_ids) {
            continue;
        }
        let reference = finished_snapshot_reference(reader, &unit)?;
        record_snapshot_contribution(
            reader,
            reference,
            logical_section,
            at,
            cancelled,
            &mut moments,
            &mut retained,
        )?;
    }

    finish_snapshot_selection(retained, moments, inventory.warnings)
}

fn empty_snapshot_selection(warnings: Vec<StoreWarning>) -> SnapshotSelection {
    SnapshotSelection {
        anchor: None,
        current_segments: Vec::new(),
        predecessor_segments: Vec::new(),
        warnings,
    }
}

fn finished_snapshot_reference(
    reader: &Reader,
    unit: &FinalUnit,
) -> Result<SegmentRef, ReaderError> {
    let file = reader.dir.open_finished(unit)?;
    let catalog = read_catalog(&file)?;
    reader.dir.validate_finished_file(&file, unit)?;
    Ok(SegmentRef {
        source: SegmentSource::Finished(unit.clone()),
        provenance: Arc::clone(&reader.provenance),
        segment_id: unit.address.id.get(),
        min_ts: unit.summary.min_ts,
        max_ts: unit.summary.max_ts,
        captured_bytes: unit.identity.len,
        sections: sections_of(std::iter::once(&catalog)).into(),
    })
}

fn active_inventory_reference(
    reader: &Reader,
    inventory: &CatalogInventory,
) -> Result<Option<SegmentRef>, ReaderError> {
    let Some(snapshot) = reader.dir.open_catalog_inventory_active(inventory)? else {
        return Ok(None);
    };
    let (min_ts, max_ts) = active_bounds(snapshot.parts()).unwrap_or((0, 0));
    let sections = sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
    Ok(Some(SegmentRef {
        segment_id: snapshot.segment_id().get(),
        source: SegmentSource::Active(snapshot),
        provenance: Arc::clone(&reader.provenance),
        min_ts,
        max_ts,
        captured_bytes: inventory.valid_len,
        sections,
    }))
}

#[expect(
    clippy::too_many_arguments,
    reason = "selection state and the cancellation callback stay explicit at the row boundary"
)]
fn record_snapshot_contribution(
    reader: &Reader,
    reference: SegmentRef,
    logical_section: &str,
    at: i64,
    cancelled: &impl Fn() -> bool,
    moments: &mut SnapshotMoments,
    retained: &mut Vec<SnapshotContribution>,
) -> Result<(), ReaderError> {
    if !reference
        .sections()
        .iter()
        .any(|section| logical_section_name(section.type_id) == Some(logical_section))
    {
        return Ok(());
    }
    let local = snapshot_moments(reader, &reference, logical_section, at, cancelled)?;
    if local.current.is_none() {
        return Ok(());
    }
    if let Some(current) = local.current {
        moments.record(current);
    }
    if let Some(previous) = local.previous {
        moments.record(previous);
    }
    retained.push(SnapshotContribution {
        reference,
        moments: local,
    });
    Ok(())
}

fn note_snapshot_finished_catalog_read() {
    #[cfg(test)]
    SNAPSHOT_FINISHED_CATALOG_READS.with(|reads| reads.set(reads.get().saturating_add(1)));
}

#[cfg(test)]
fn reset_snapshot_finished_catalog_reads() {
    SNAPSHOT_FINISHED_CATALOG_READS.with(|reads| reads.set(0));
}

#[cfg(test)]
fn snapshot_finished_catalog_reads() -> usize {
    SNAPSHOT_FINISHED_CATALOG_READS.with(std::cell::Cell::get)
}

struct SnapshotContribution {
    reference: SegmentRef,
    moments: SnapshotMoments,
}

#[derive(Clone, Copy, Default)]
struct SnapshotMoments {
    current: Option<i64>,
    previous: Option<i64>,
}

impl SnapshotMoments {
    fn record(&mut self, at: i64) {
        match self.current {
            None => self.current = Some(at),
            Some(current) if at > current => {
                self.previous = Some(current);
                self.current = Some(at);
            }
            Some(current) if at < current => {
                self.previous = Some(self.previous.map_or(at, |previous| previous.max(at)));
            }
            Some(_equal) => {}
        }
    }

    fn contains(self, at: Option<i64>) -> bool {
        at.is_some_and(|at| self.current == Some(at) || self.previous == Some(at))
    }
}

fn finish_snapshot_selection(
    mut retained: Vec<SnapshotContribution>,
    moments: SnapshotMoments,
    warnings: Vec<StoreWarning>,
) -> Result<SnapshotSelection, ReaderError> {
    let Some(current) = moments.current else {
        return Ok(empty_snapshot_selection(warnings));
    };
    retained.retain(|contribution| {
        contribution.moments.contains(Some(current))
            || contribution.moments.contains(moments.previous)
    });
    let anchor_index = retained
        .iter()
        .enumerate()
        .filter(|(_index, contribution)| contribution.moments.contains(Some(current)))
        .max_by_key(|(_index, contribution)| contribution.reference.id())
        .map(|(index, _contribution)| index)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "snapshot selection lost its current section contributor",
            )
        })?;
    let anchor = retained.swap_remove(anchor_index).reference;
    let (current, predecessors): (Vec<_>, Vec<_>) = retained
        .into_iter()
        .partition(|contribution| contribution.moments.contains(Some(current)));
    let mut current_segments = current
        .into_iter()
        .map(|contribution| contribution.reference)
        .collect::<Vec<_>>();
    let mut predecessor_segments = predecessors
        .into_iter()
        .map(|contribution| contribution.reference)
        .collect::<Vec<_>>();
    current_segments.sort_unstable_by_key(|segment| Reverse(segment.id()));
    predecessor_segments.sort_unstable_by_key(|segment| Reverse(segment.id()));
    Ok(SnapshotSelection {
        anchor: Some(anchor),
        current_segments,
        predecessor_segments,
        warnings,
    })
}

fn snapshot_moments(
    reader: &Reader,
    reference: &SegmentRef,
    logical_section: &str,
    at: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<SnapshotMoments, ReaderError> {
    let segment = reader.open_segment(reference)?;
    let mut moments = SnapshotMoments::default();
    for section in reference
        .sections()
        .iter()
        .filter(|section| logical_section_name(section.type_id) == Some(logical_section))
    {
        if cancelled() {
            return Err(cancelled_snapshot_read());
        }
        let Some(timestamp) = contract(section.type_id).and_then(|contract| {
            contract
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name)
        }) else {
            continue;
        };
        segment.visit_rows(
            section.type_id,
            &[timestamp],
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                if let Some(Cell::Ts(stored)) = row.get(timestamp)
                    && *stored <= at
                {
                    moments.record(*stored);
                }
                true
            },
        )?;
        if cancelled() {
            return Err(cancelled_snapshot_read());
        }
    }
    Ok(moments)
}

fn cancelled_snapshot_read() -> ReaderError {
    std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "snapshot selection was cancelled",
    )
    .into()
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

    /// Scan compact catalog summaries before choosing which full catalogs to
    /// open.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory cannot be walked or a compact
    /// catalog summary cannot be read safely.
    pub fn catalog_discovery(&self) -> Result<CatalogDiscovery<'_>, ReaderError> {
        Ok(CatalogDiscovery {
            reader: self,
            scan: self.dir.scan_catalogs()?,
        })
    }

    /// Select the latest real sample and its immediate predecessor for one
    /// logical section at `at`.
    ///
    /// Finished artifacts are inspected newest first. Their compact summaries,
    /// section catalogs, and bodies are opened only until no older segment can
    /// contribute either retained moment. Every segment and physical layout
    /// contributing an equal retained timestamp is preserved.
    ///
    /// `hour_start` identifies the UTC-hour window chosen by the caller. The
    /// selector rejects an inverted window and may walk into older retention
    /// for the section-aware predecessor.
    ///
    /// # Errors
    ///
    /// Returns an I/O or decode error when a selected source cannot be read,
    /// or an interrupted error when `cancelled` requests cancellation.
    pub fn snapshot_selection(
        &self,
        hour_start: i64,
        at: i64,
        logical_section: &str,
        cancelled: &impl Fn() -> bool,
    ) -> Result<SnapshotSelection, ReaderError> {
        select_snapshot(self, hour_start, at, logical_section, cancelled)
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

    /// Find one segment by its stable id without opening unrelated catalogs.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory or the selected segment catalog
    /// cannot be read safely.
    pub fn catalog_segment(&self, id: i64) -> Result<Listing, ReaderError> {
        let scan = self.dir.scan_catalogs()?;
        let mut segments = Vec::with_capacity(1);
        if let Some(unit) = scan
            .finished
            .iter()
            .find(|unit| unit.address.id.get() == id)
        {
            let file = self.dir.open_finished(unit)?;
            let catalog = read_catalog(&file)?;
            self.dir.validate_finished_file(&file, unit)?;
            segments.push(SegmentRef {
                source: SegmentSource::Finished(unit.clone()),
                provenance: Arc::clone(&self.provenance),
                segment_id: unit.address.id.get(),
                min_ts: unit.summary.min_ts,
                max_ts: unit.summary.max_ts,
                captured_bytes: unit.identity.len,
                sections: sections_of(std::iter::once(&catalog)).into(),
            });
        } else if scan
            .active
            .first()
            .is_some_and(|part| part.segment_id.get() == id)
            && let Some(snapshot) = self.dir.open_active_snapshot(&scan)?
        {
            let (min_ts, max_ts) = active_bounds(snapshot.parts()).unwrap_or((0, 0));
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
        Ok(Listing {
            segments,
            warnings: scan.warnings,
        })
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
        self.catalog_discovery()?
            .list_segments(range, validate_bodies, include_predecessor)
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
