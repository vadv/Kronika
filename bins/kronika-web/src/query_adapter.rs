//! Native captured-data adapter for shared query execution.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use kronika_query::{
    CapturedCatalog, DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject,
    OpaqueCapture, PredecessorSelection, QueryDataset, QueryError, SegmentSelection,
};
use kronika_reader::{CatalogDiscovery, Listing, Reader, Segment, SegmentRef, StoreObject};

/// One native reader retained for every opaque segment capture it produced.
#[derive(Debug)]
pub(crate) struct NativeDataset {
    reader: Reader,
    started: Instant,
}

impl NativeDataset {
    pub(crate) fn from_root(root: &Path) -> Result<Self, QueryError> {
        let started = Instant::now();
        Ok(Self {
            reader: Reader::open(root)?,
            started,
        })
    }

    fn descriptor(segment: &SegmentRef) -> DatasetSegment {
        DatasetSegment::new(
            OpaqueCapture::new(segment.clone()),
            segment.id(),
            segment.kind(),
            segment.min_ts(),
            segment.max_ts(),
            segment.active_position(),
            Arc::from(segment.sections()),
        )
    }

    fn listing(listing: Listing) -> DatasetListing {
        DatasetListing {
            segments: listing
                .segments
                .into_iter()
                .map(|segment| Self::descriptor(&segment))
                .collect(),
            warnings: listing.warnings.into_iter().map(warning).collect(),
        }
    }

    fn captured(segment: &DatasetSegment) -> Result<&SegmentRef, QueryError> {
        segment.capture().downcast_ref().ok_or_else(|| {
            QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "segment capture belongs to another data adapter",
            )))
        })
    }
}

#[derive(Debug)]
struct NativeCatalog<'a> {
    owner: &'a NativeDataset,
    discovery: CatalogDiscovery<'a>,
    ranges: Vec<(i64, i64)>,
}

impl CapturedCatalog for NativeCatalog<'_> {
    fn ranges(&self) -> &[(i64, i64)] {
        &self.ranges
    }

    fn segments(&self, selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        let bounds = (selection.bounds.start, selection.bounds.end);
        let listing = match selection.predecessor {
            PredecessorSelection::None => self.discovery.clone().segments(bounds),
            PredecessorSelection::Closest => {
                self.discovery.clone().segments_with_predecessor(bounds)
            }
            PredecessorSelection::ForLayouts(type_ids) => self
                .discovery
                .clone()
                .segments_with_predecessors_for(bounds, &type_ids),
        }?;
        log_catalog_open(listing.segments.len(), &listing, self.owner.started);
        Ok(NativeDataset::listing(listing))
    }
}

impl QueryDataset for NativeDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        let discovery = self.reader.catalog_discovery()?;
        let ranges = discovery.ranges().collect();
        Ok(Box::new(NativeCatalog {
            owner: self,
            discovery,
            ranges,
        }))
    }

    fn segment(&self, id: i64) -> Result<DatasetListing, QueryError> {
        let listing = self.reader.catalog_segment(id)?;
        log_warnings(&listing);
        if let Some(segment) = listing.segments.first() {
            eprintln!(
                "kronika-web: segment_open id={} kind={} sections={} elapsed_us={}",
                segment.id(),
                match segment.kind() {
                    kronika_reader::SegmentKind::Finished => "finished",
                    kronika_reader::SegmentKind::Active => "active",
                },
                segment.sections().len(),
                self.started.elapsed().as_micros(),
            );
        }
        Ok(Self::listing(listing))
    }

    fn open(&self, segment: &DatasetSegment) -> Result<Segment, QueryError> {
        self.reader
            .open_segment(Self::captured(segment)?)
            .map_err(QueryError::from)
    }

    fn at_active_position(
        &self,
        segment: &DatasetSegment,
        position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        let pinned = Self::captured(segment)?
            .at_active_position(position)
            .map_err(QueryError::from)?;
        Ok(Self::descriptor(&pinned))
    }
}

fn log_catalog_open(segment_count: usize, listing: &Listing, started: Instant) {
    log_warnings(listing);
    eprintln!(
        "kronika-web: catalog_open segments={segment_count} warnings={} elapsed_us={}",
        listing.warnings.len(),
        started.elapsed().as_micros(),
    );
}

fn log_warnings(listing: &Listing) {
    for warning in &listing.warnings {
        eprintln!("kronika-web: store warning code={}", warning.reason.code());
    }
}

const fn warning(warning: kronika_reader::StoreWarning) -> DatasetWarning {
    let subject = match warning.affected {
        StoreObject::Segment(address) => DatasetWarningSubject::Segment(address.id.get()),
        StoreObject::ActiveJournal => DatasetWarningSubject::ActiveJournal,
        StoreObject::Foreign(path) => DatasetWarningSubject::ForeignEntry {
            name_hash: path.name_hash,
            name_len: path.name_len,
        },
        _ => DatasetWarningSubject::Other,
    };
    DatasetWarning {
        subject,
        code: warning.reason.code(),
    }
}
