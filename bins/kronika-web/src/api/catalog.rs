//! Actual segment and physical-section inventory.

use std::ops::Bound::{Included, Unbounded};
use std::path::Path;

use kronika_reader::{Listing, Reader, SegmentKind, SegmentRef, StoreObject, StoreWarning};
use kronika_registry::{logical_section_name, section_implementation, section_name};
use serde_json::{Value, json};

use super::render::record;
use super::{ApiError, CachePolicy, ResponseMeta, log_warnings};
use crate::route::Window;

pub(crate) struct PreparedCatalog {
    listing: Listing,
    window: Window,
}

pub(super) fn prepare(root: &Path, window: Window) -> Result<PreparedCatalog, ApiError> {
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments((
        window.from.map_or(Unbounded, Included),
        window.to.map_or(Unbounded, Included),
    ))?;
    log_warnings(&listing.warnings);
    Ok(PreparedCatalog { listing, window })
}

impl PreparedCatalog {
    pub(super) const fn meta() -> ResponseMeta {
        ResponseMeta::ok(CachePolicy::Revalidate)
    }

    pub(super) fn stream(self, emit: &mut impl FnMut(Vec<u8>) -> bool) -> Result<(), ApiError> {
        if !emit(record(json!({
            "record": "catalog",
            "from": self.window.from.map(|value| value.to_string()),
            "to": self.window.to.map(|value| value.to_string()),
        }))?) {
            return Ok(());
        }
        for segment in &self.listing.segments {
            let value = match segment.kind() {
                SegmentKind::Finished => finished(segment),
                SegmentKind::Active => active(segment)?,
            };
            if !emit(record(value)?) {
                return Ok(());
            }
        }
        for warning in &self.listing.warnings {
            if !emit(record(warning_value(warning))?) {
                return Ok(());
            }
        }
        Ok(())
    }
}

fn finished(segment: &SegmentRef) -> Value {
    json!({
        "record": "finished_segment",
        "id": segment.id().to_string(),
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "sections": sections(segment),
    })
}

fn active(segment: &SegmentRef) -> Result<Value, ApiError> {
    let position = segment.active_position().ok_or_else(|| {
        ApiError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "active segment has no committed WAL position",
        )))
    })?;
    Ok(json!({
        "record": "active_segment",
        "id": segment.id().to_string(),
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "cursor": cursor_value(segment.id(), position),
        "sections": sections(segment),
    }))
}

fn cursor_value(segment_id: i64, wal_position: u64) -> Value {
    json!({
        "segment_id": segment_id.to_string(),
        "wal_position": wal_position.to_string(),
    })
}

fn sections(segment: &SegmentRef) -> Vec<Value> {
    section_values(segment.sections())
}

fn section_values(sections: &[kronika_reader::SegmentSection]) -> Vec<Value> {
    sections
        .iter()
        .map(|section| {
            json!({
                "logical_name": logical_section_name(section.type_id),
                "physical_name": section_name(section.type_id),
                "type_id": section.type_id.to_string(),
                "implementation": section_implementation(section.type_id),
                "rows": section.rows.to_string(),
                "bytes": section.bytes.to_string(),
            })
        })
        .collect()
}

fn warning_value(warning: &StoreWarning) -> Value {
    let affected = match warning.affected {
        StoreObject::Segment(address) => json!({
            "kind": "segment",
            "id": address.id.get().to_string(),
        }),
        StoreObject::ActiveJournal => json!({ "kind": "active_journal" }),
        StoreObject::Foreign(path) => json!({
            "kind": "foreign_entry",
            "name_hash": path.name_hash.to_string(),
            "name_len": path.name_len.to_string(),
        }),
        _ => json!({ "kind": "store_object" }),
    };
    json!({
        "record": "warning",
        "code": warning.reason.code(),
        "affected": affected,
    })
}

#[cfg(test)]
mod tests;
