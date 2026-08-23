//! Actual segment and physical-section inventory.

use std::ops::Bound::{Included, Unbounded};
use std::path::Path;

use kronika_reader::{Listing, Reader, SegmentKind, SegmentRef, StoreObject, StoreWarning};
use kronika_registry::{logical_section_name, section_implementation, section_name};
use serde_json::{Value, json};

use super::{ApiError, CachePolicy, ResponseMeta, log_warnings};
use crate::config::{SOURCE_FAMILIES, SOURCE_OS, SOURCE_POSTGRESQL};
use crate::route::Window;

pub(crate) struct PreparedCatalog {
    listing: Listing,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
    present_sources: Option<u32>,
    metric_sources: Option<u32>,
}

pub(super) fn prepare(
    root: &Path,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
) -> Result<PreparedCatalog, ApiError> {
    let started = std::time::Instant::now();
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments((
        window.from.map_or(Unbounded, Included),
        window.to.map_or(Unbounded, Included),
    ))?;
    log_open(listing.segments.len(), &listing.warnings, started);
    Ok(PreparedCatalog::from_listing(
        listing,
        window,
        configured_sources,
        synthetic_demo,
    ))
}

impl PreparedCatalog {
    pub(super) const fn from_listing(
        listing: Listing,
        window: Window,
        configured_sources: u32,
        synthetic_demo: bool,
    ) -> Self {
        Self {
            listing,
            window,
            configured_sources,
            synthetic_demo,
            present_sources: None,
            metric_sources: None,
        }
    }

    pub(super) const fn with_present_sources(
        mut self,
        present_sources: u32,
        metric_sources: u32,
    ) -> Self {
        self.present_sources = Some(present_sources);
        self.metric_sources = Some(metric_sources);
        self
    }

    pub(super) const fn meta() -> ResponseMeta {
        ResponseMeta::ok(CachePolicy::Revalidate)
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let present_sources = self.present_sources.unwrap_or_else(|| {
            self.listing
                .segments
                .iter()
                .flat_map(SegmentRef::sections)
                .filter_map(|section| source_bit(section.type_id))
                .fold(0_u32, |present, bit| present | bit)
        });
        let metric_sources = self.metric_sources.unwrap_or_else(|| {
            self.listing
                .segments
                .iter()
                .flat_map(SegmentRef::sections)
                .filter_map(|section| metric_source_bit(section.type_id))
                .fold(0_u32, |present, bit| present | bit)
        });
        if cancelled()
            || !emit(json!({
                "record": "catalog",
                "from": self.window.from.map(|value| value.to_string()),
                "to": self.window.to.map(|value| value.to_string()),
                "demo": self.synthetic_demo.then_some("synthetic"),
                "source_families": source_family_values(
                    self.configured_sources,
                    present_sources,
                    metric_sources,
                ),
            }))
        {
            return Ok(());
        }
        for segment in &self.listing.segments {
            if cancelled() {
                return Ok(());
            }
            let value = match segment.kind() {
                SegmentKind::Finished => finished(segment),
                SegmentKind::Active => active(segment)?,
            };
            if !emit(value) {
                return Ok(());
            }
        }
        for warning in &self.listing.warnings {
            if cancelled() || !emit(warning_value(warning)) {
                return Ok(());
            }
        }
        Ok(())
    }
}

pub(super) fn log_open(
    segment_count: usize,
    warnings: &[StoreWarning],
    started: std::time::Instant,
) {
    log_warnings(warnings);
    eprintln!(
        "kronika-web: catalog_open segments={segment_count} warnings={} elapsed_us={}",
        warnings.len(),
        started.elapsed().as_micros(),
    );
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
                "source_family": source_bit(section.type_id).and_then(source_name),
                "rows": section.rows.to_string(),
                "bytes": section.bytes.to_string(),
            })
        })
        .collect()
}

fn source_family_values(configured: u32, present: u32, metrics: u32) -> Vec<Value> {
    SOURCE_FAMILIES
        .iter()
        .map(|family| {
            json!({
                "name": family.name,
                "configured": configured & family.bit != 0,
                "present": present & family.bit != 0,
                "metrics_present": metrics & family.bit != 0,
            })
        })
        .collect()
}

pub(super) const fn source_bit(type_id: u32) -> Option<u32> {
    match type_id {
        1_001_001..=1_019_999 | 2_001_001..=2_199_999 => Some(SOURCE_POSTGRESQL),
        1_100_001..=1_299_999 => Some(SOURCE_OS),
        _ => None,
    }
}

pub(super) fn metric_source_bit(type_id: u32) -> Option<u32> {
    let bit = source_bit(type_id)?;
    let is_log = logical_section_name(type_id).is_some_and(|name| name.starts_with("pg_log_"));
    (!is_log).then_some(bit)
}

fn source_name(bit: u32) -> Option<&'static str> {
    SOURCE_FAMILIES
        .iter()
        .find(|family| family.bit == bit)
        .map(|family| family.name)
}

pub(super) fn warning_value(warning: &StoreWarning) -> Value {
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
