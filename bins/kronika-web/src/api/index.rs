//! One logical indexed series selected from per-layout IDX blocks.

use std::path::Path;

use hyper::StatusCode;
use kronika_index::{
    DERIVED_HEALTH_TYPE_ID, INSTANCE_METADATA_TYPE_ID, OS_PSI_TYPE_ID, ResourceIndex, resource,
};
use kronika_reader::{SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, contract, logical_section_name};
use serde_json::{Value, json};

use super::render::{identity, layout, observation, record};
use super::{ApiError, CachePolicy, Prepared, ResponseMeta, explicit_segment};
use crate::route::SegmentRequest;

pub(crate) struct PreparedIndex {
    meta: ResponseMeta,
    segment: SegmentRef,
    logical_name: String,
    resource: ResourceIndex,
}

pub(super) fn prepare(
    root: &Path,
    sources: u32,
    request: SegmentRequest,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let (reader, segment) = explicit_segment(root, request.segment_id)?;
    let exists = request.section == "health"
        || segment.sections().iter().any(|section| {
            contract(section.type_id).is_some()
                && logical_section_name(section.type_id).is_some_and(|name| name == request.section)
        });
    if !exists {
        return Err(ApiError::NoSuchSection);
    }
    let started = std::time::Instant::now();
    let resource = resource(root, &reader, &segment, sources, &request.section)?;
    let objects = resource
        .index
        .sections
        .iter()
        .map(|section| section.objects.len())
        .sum::<usize>();
    eprintln!(
        "kronika-web: index_resource segment_id={} logical_name={} persisted={} sections={} objects={} elapsed_us={}",
        segment.id(),
        request.section,
        resource.persisted,
        resource.index.sections.len(),
        objects,
        started.elapsed().as_micros(),
    );
    let meta = resource_meta(segment.kind(), resource.index.checksum)?;
    if meta
        .etag
        .as_deref()
        .zip(if_none_match)
        .is_some_and(|(current, offered)| etag_matches(offered, current))
    {
        return Ok(Prepared::Empty(ResponseMeta {
            status: StatusCode::NOT_MODIFIED,
            ..meta
        }));
    }
    Ok(Prepared::Index(PreparedIndex {
        meta,
        segment,
        logical_name: request.section,
        resource,
    }))
}

impl PreparedIndex {
    pub(super) fn meta(&self) -> ResponseMeta {
        self.meta.clone()
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        if cancelled()
            || !emit(record(json!({
                "record": "index",
                "segment": segment_value(&self.segment),
                "logical_name": self.logical_name,
                "sources": self.resource.index.sources.to_string(),
                "checksum": self.resource.index.checksum.map(|value| format!("{value:08x}")),
            }))?)
        {
            return Ok(());
        }
        for section in self.resource.index.sections {
            if cancelled()
                || !emit(record(json!({
                    "record": "layout",
                    "layout": section_layout(&self.logical_name, section.type_id)?,
                }))?)
            {
                return Ok(());
            }
            for object in section.objects {
                if cancelled()
                    || !emit(record(json!({
                        "record": "object",
                        "type_id": section.type_id.to_string(),
                        "identity": object.identity.iter().map(identity).collect::<Vec<_>>(),
                        "observations": object
                            .observations
                            .into_iter()
                            .map(observation)
                            .collect::<Vec<_>>(),
                    }))?)
                {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

fn segment_value(segment: &SegmentRef) -> Value {
    let cursor = segment.active_position().map(|wal_position| {
        json!({
            "segment_id": segment.id().to_string(),
            "wal_position": wal_position.to_string(),
        })
    });
    json!({
        "id": segment.id().to_string(),
        "kind": match segment.kind() {
            SegmentKind::Finished => "finished",
            SegmentKind::Active => "active",
        },
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "cursor": cursor,
    })
}

fn resource_meta(kind: SegmentKind, checksum: Option<u32>) -> Result<ResponseMeta, ApiError> {
    match kind {
        SegmentKind::Finished => {
            let checksum = checksum.ok_or_else(|| {
                ApiError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "finished index has no validated checksum",
                )))
            })?;
            Ok(ResponseMeta {
                status: StatusCode::OK,
                cache: CachePolicy::Revalidate,
                etag: Some(format!("\"{checksum:08x}\"")),
            })
        }
        SegmentKind::Active => Ok(ResponseMeta::ok(CachePolicy::NoStore)),
    }
}

pub(super) fn section_layout(logical_name: &str, type_id: u32) -> Result<Value, ApiError> {
    if type_id == DERIVED_HEALTH_TYPE_ID {
        return Ok(json!({
            "logical_name": "health",
            "physical_name": "derived_health",
            "type_id": DERIVED_HEALTH_TYPE_ID.to_string(),
            "implementation": "kronika",
            "identity": [],
            "columns": [{
                "name": "health",
                "type": "u32",
                "class": "gauge",
                "unit": "percent",
                "nullable": true,
            }],
            "provenance": {
                "inputs": [
                    INSTANCE_METADATA_TYPE_ID.to_string(),
                    OS_PSI_TYPE_ID.to_string(),
                ],
            },
        }));
    }
    let contract = contract(type_id).ok_or(ApiError::NoSuchSection)?;
    let fields: Vec<&str> = contract
        .columns
        .iter()
        .filter(|column| {
            contract.identity.contains(&column.name)
                || matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge)
        })
        .map(|column| column.name)
        .collect();
    Ok(layout(logical_name, contract, &fields))
}

fn etag_matches(offered: &str, current: &str) -> bool {
    offered.split(',').any(|candidate| {
        let candidate = candidate.trim();
        if candidate == "*" {
            return true;
        }
        candidate.strip_prefix("W/").unwrap_or(candidate).trim() == current
    })
}

#[cfg(test)]
mod tests;
