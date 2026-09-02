//! Actual segment and physical-section inventory.

use kronika_reader::{SegmentKind, SegmentSection};
use kronika_registry::{logical_section_name, section_implementation, section_name};
use serde_json::{Value, json};

use crate::dataset::{
    DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject, QueryDataset,
    SegmentBounds, SegmentSelection,
};
use crate::render::record;
use crate::{CatalogRequest, QueryError, QuerySink, SOURCE_OS, SOURCE_POSTGRESQL, Window};

pub(crate) struct PreparedCatalog {
    listing: DatasetListing,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
}

impl PreparedCatalog {
    pub(crate) fn prepare(
        dataset: &dyn QueryDataset,
        request: CatalogRequest,
        configured_sources: u32,
        synthetic_demo: bool,
    ) -> Result<Self, QueryError> {
        let catalog = dataset.catalog()?;
        let listing = catalog.segments(SegmentSelection::new(SegmentBounds::inclusive(
            request.window.from,
            request.window.to,
        )))?;
        Ok(Self {
            listing,
            window: request.window,
            configured_sources,
            synthetic_demo,
        })
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        let present_sources = self
            .listing
            .segments
            .iter()
            .flat_map(DatasetSegment::sections)
            .filter_map(|section| source_bit(section.type_id))
            .fold(0_u32, |present, bit| present | bit);
        let metric_sources = self
            .listing
            .segments
            .iter()
            .flat_map(DatasetSegment::sections)
            .filter_map(|section| metric_source_bit(section.type_id))
            .fold(0_u32, |present, bit| present | bit);
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "catalog",
                "from": self.window.from.map(|value| value.to_string()),
                "to": self.window.to.map(|value| value.to_string()),
                "demo": self.synthetic_demo.then_some("synthetic"),
                "source_families": source_family_values(
                    self.configured_sources,
                    present_sources,
                    metric_sources,
                ),
            }))?)
        {
            return Ok(());
        }
        for segment in &self.listing.segments {
            if sink.cancelled() {
                return Ok(());
            }
            let value = match segment.kind() {
                SegmentKind::Finished => finished(segment),
                SegmentKind::Active => active(segment)?,
            };
            if !sink.record(record(value)?) {
                return Ok(());
            }
        }
        for warning in &self.listing.warnings {
            if sink.cancelled() || !sink.record(record(warning_value(*warning))?) {
                return Ok(());
            }
        }
        Ok(())
    }
}

fn finished(segment: &DatasetSegment) -> Value {
    json!({
        "record": "finished_segment",
        "id": segment.id().to_string(),
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "sections": section_values(segment.sections()),
    })
}

fn active(segment: &DatasetSegment) -> Result<Value, QueryError> {
    let position = segment.active_position().ok_or_else(|| {
        QueryError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "active segment has no committed WAL position",
        )))
    })?;
    Ok(json!({
        "record": "active_segment",
        "id": segment.id().to_string(),
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "cursor": {
            "segment_id": segment.id().to_string(),
            "wal_position": position.to_string(),
        },
        "sections": section_values(segment.sections()),
    }))
}

fn section_values(sections: &[SegmentSection]) -> Vec<Value> {
    sections
        .iter()
        .map(|section| {
            json!({
                "logical_name": logical_section_name(section.type_id),
                "physical_name": section_name(section.type_id),
                "type_id": section.type_id.to_string(),
                "implementation": section_implementation(section.type_id),
                "source_family": source_bit(section.type_id).and_then(source_name),
                "rows": section.rows.to_string(),
                "bytes": section.bytes.to_string(),
            })
        })
        .collect()
}

fn source_family_values(configured: u32, present: u32, metrics: u32) -> Vec<Value> {
    [("os", SOURCE_OS), ("postgresql", SOURCE_POSTGRESQL)]
        .into_iter()
        .map(|(name, bit)| {
            json!({
                "name": name,
                "configured": configured & bit != 0,
                "present": present & bit != 0,
                "metrics_present": metrics & bit != 0,
            })
        })
        .collect()
}

const fn source_bit(type_id: u32) -> Option<u32> {
    match type_id {
        1_001_001..=1_019_999 | 2_001_001..=2_199_999 => Some(SOURCE_POSTGRESQL),
        1_100_001..=1_299_999 => Some(SOURCE_OS),
        _ => None,
    }
}

fn metric_source_bit(type_id: u32) -> Option<u32> {
    let bit = source_bit(type_id)?;
    let is_log = logical_section_name(type_id).is_some_and(|name| name.starts_with("pg_log_"));
    (!is_log).then_some(bit)
}

const fn source_name(bit: u32) -> Option<&'static str> {
    match bit {
        SOURCE_OS => Some("os"),
        SOURCE_POSTGRESQL => Some("postgresql"),
        _ => None,
    }
}

fn warning_value(warning: DatasetWarning) -> Value {
    let affected = match warning.subject {
        DatasetWarningSubject::Segment(id) => json!({
            "kind": "segment",
            "id": id.to_string(),
        }),
        DatasetWarningSubject::ActiveJournal => json!({ "kind": "active_journal" }),
        DatasetWarningSubject::ForeignEntry {
            name_hash,
            name_len,
        } => json!({
            "kind": "foreign_entry",
            "name_hash": name_hash.to_string(),
            "name_len": name_len.to_string(),
        }),
        DatasetWarningSubject::Other => json!({ "kind": "store_object" }),
    };
    json!({
        "record": "warning",
        "code": warning.code,
        "affected": affected,
    })
}
