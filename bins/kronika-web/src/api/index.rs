//! HTTP representation of one allowlisted per-segment index series.

use std::path::Path;

use hyper::StatusCode;
use kronika_index::{
    DERIVED_HEALTH_TYPE_ID, FindingBlock, FindingKind, INSTANCE_METADATA_TYPE_ID,
    INSTANCE_METADATA_V1_TYPE_ID, OS_PSI_TYPE_ID, ResourceIndex, SeriesBlock, resource,
    series_keys,
};
use kronika_reader::{SegmentKind, SegmentRef};
use kronika_registry::{contract, section_implementation};
use serde_json::{Value, json};

use super::render::record;
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
    request: SegmentRequest,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let (reader, segment) = explicit_segment(root, request.segment_id)?;
    if series_keys(&segment, &request.section).is_empty() {
        return Err(ApiError::NoSuchSection);
    }
    let started = std::time::Instant::now();
    let resource = resource(root, &reader, &segment, &request.section)?;
    let point_count = resource.index.blocks.iter().map(block_len).sum::<usize>();
    eprintln!(
        "kronika-web: index_resource segment_id={} logical_name={} persisted={} blocks={} points={} elapsed_us={}",
        segment.id(),
        request.section,
        resource.persisted,
        resource.index.blocks.len(),
        point_count,
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
                "checksum": self.resource.index.checksum.map(|value| format!("{value:08x}")),
            }))?)
        {
            return Ok(());
        }
        for block in self.resource.index.blocks {
            if let SeriesBlock::Findings(block) = block {
                if !stream_findings(block, emit, cancelled)? {
                    return Ok(());
                }
                continue;
            }
            if cancelled()
                || !emit(record(json!({
                    "record": "layout",
                    "layout": block_layout(&self.logical_name, &block)?,
                }))?)
            {
                return Ok(());
            }
            let health_series = match &block {
                SeriesBlock::OsHealth(_) => Some("os_health"),
                SeriesBlock::OverallHealth(_) => Some("overall_health"),
                SeriesBlock::PostgresHealth(_) => Some("postgres_health"),
                _ => None,
            };
            match block {
                SeriesBlock::OsHealth(points)
                | SeriesBlock::OverallHealth(points)
                | SeriesBlock::PostgresHealth(points) => {
                    let series = health_series.expect("health block has a series name");
                    for point in points {
                        if cancelled()
                            || !emit(record(json!({
                                "record": "point",
                                "series": series,
                                "type_id": DERIVED_HEALTH_TYPE_ID.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": {},
                                "value": point.value,
                            }))?)
                        {
                            return Ok(());
                        }
                    }
                }
                SeriesBlock::PgTransactions { type_id, points } => {
                    for point in points {
                        if cancelled()
                            || !emit(record(json!({
                                "record": "point",
                                "series": "transactions_per_second",
                                "type_id": type_id.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": { "datid": point.datid },
                                "value": point.value,
                            }))?)
                        {
                            return Ok(());
                        }
                    }
                }
                SeriesBlock::PgActiveBackends { type_id, points } => {
                    for point in points {
                        if cancelled()
                            || !emit(record(json!({
                                "record": "point",
                                "series": "active_backends",
                                "type_id": type_id.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": {},
                                "value": point.count,
                            }))?)
                        {
                            return Ok(());
                        }
                    }
                }
                SeriesBlock::Findings(_) => return Err(ApiError::NoSuchSection),
            }
        }
        Ok(())
    }
}

fn stream_findings(
    block: FindingBlock,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    if cancelled()
        || !emit(record(json!({
            "record": "findings",
            "type_id": block.type_id.to_string(),
            "total_hits": block.total_hits,
            "truncated": block.truncated,
        }))?)
    {
        return Ok(false);
    }
    for finding in block.findings {
        if cancelled()
            || !emit(record(json!({
                "record": "finding",
                "kind": finding_kind(finding.kind),
                "type_id": block.type_id.to_string(),
                "field_ordinal": finding.field_ordinal,
                "row_ordinal": finding.row_ordinal,
                "ts": finding.timestamp.to_string(),
            }))?)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

const fn block_len(block: &SeriesBlock) -> usize {
    match block {
        SeriesBlock::OsHealth(points)
        | SeriesBlock::OverallHealth(points)
        | SeriesBlock::PostgresHealth(points) => points.len(),
        SeriesBlock::PgTransactions { points, .. } => points.len(),
        SeriesBlock::PgActiveBackends { points, .. } => points.len(),
        SeriesBlock::Findings(block) => block.findings.len(),
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

fn block_layout(logical_name: &str, block: &SeriesBlock) -> Result<Value, ApiError> {
    match block {
        SeriesBlock::OsHealth(_) => Ok(health_layout("os_health")),
        SeriesBlock::OverallHealth(_) => Ok(health_layout("overall_health")),
        SeriesBlock::PostgresHealth(_) => Ok(health_layout("postgres_health")),
        SeriesBlock::PgTransactions { type_id, .. }
        | SeriesBlock::PgActiveBackends { type_id, .. } => section_layout(logical_name, *type_id),
        SeriesBlock::Findings(_) => Err(ApiError::NoSuchSection),
    }
}

const fn finding_kind(kind: FindingKind) -> &'static str {
    match kind {
        FindingKind::KnownBad => "known_bad",
        FindingKind::Spike => "spike",
        FindingKind::Event => "event",
    }
}

pub(super) fn section_layout(logical_name: &str, type_id: u32) -> Result<Value, ApiError> {
    if type_id == DERIVED_HEALTH_TYPE_ID {
        return Ok(health_layout("os_health"));
    }
    let contract = contract(type_id).ok_or(ApiError::NoSuchSection)?;
    let (identity, columns) = match logical_name {
        "pg_stat_database" => (
            json!(["datid"]),
            json!([{
                "name": "transactions_per_second",
                "type": "f64",
                "class": "gauge",
                "unit": "per_second",
                "nullable": true,
            }]),
        ),
        "pg_stat_activity" => (
            json!([]),
            json!([{
                "name": "active_backends",
                "type": "u32",
                "class": "gauge",
                "unit": "count",
                "nullable": false,
            }]),
        ),
        _ => return Err(ApiError::NoSuchSection),
    };
    Ok(json!({
        "logical_name": logical_name,
        "physical_name": contract.name,
        "type_id": type_id.to_string(),
        "implementation": section_implementation(type_id),
        "identity": identity,
        "columns": columns,
    }))
}

fn health_layout(series: &str) -> Value {
    json!({
        "logical_name": "health",
        "physical_name": format!("derived_{series}"),
        "type_id": DERIVED_HEALTH_TYPE_ID.to_string(),
        "implementation": "kronika",
        "identity": [],
        "columns": [{
            "name": series,
            "type": "u8",
            "class": "gauge",
            "unit": "percent",
            "nullable": true,
        }],
        "provenance": {
            "inputs": [
                INSTANCE_METADATA_TYPE_ID.to_string(),
                INSTANCE_METADATA_V1_TYPE_ID.to_string(),
                OS_PSI_TYPE_ID.to_string(),
                "1001001",
                "1001002",
                "1001003",
            ],
        },
    })
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
