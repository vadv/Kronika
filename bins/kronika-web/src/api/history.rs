//! Projected full-resolution history, streamed per physical layout and identity.

use std::path::Path;

use kronika_reader::{Row, Segment, SegmentKind};
use serde_json::json;

use super::query::{Plan, chunk_dictionary, plans};
use super::render::{cell, projected_layout, record};
use super::{ApiError, CachePolicy, ResponseMeta, explicit_segment};
use crate::route::DataRequest;

const ROW_CHUNK_ROWS: usize = 256;

pub(crate) struct PreparedHistory {
    segment: Segment,
    logical_name: String,
    plans: Vec<Plan>,
}

pub(super) fn prepare(root: &Path, request: DataRequest) -> Result<PreparedHistory, ApiError> {
    let (reader, segment_ref) = explicit_segment(root, request.segment.segment_id)?;
    let segment = reader.open_segment(&segment_ref)?;
    let plans = plans(&segment, &request)?;
    Ok(PreparedHistory {
        segment,
        logical_name: request.segment.section,
        plans,
    })
}

impl PreparedHistory {
    pub(super) const fn meta(&self) -> ResponseMeta {
        ResponseMeta::ok(match self.segment.kind() {
            SegmentKind::Finished => CachePolicy::Immutable,
            SegmentKind::Active => CachePolicy::NoStore,
        })
    }

    pub(super) fn stream(self, emit: &mut impl FnMut(Vec<u8>) -> bool) -> Result<(), ApiError> {
        if !emit(record(json!({
            "record": "history",
            "segment": {
                "id": self.segment.id().to_string(),
                "kind": match self.segment.kind() {
                    SegmentKind::Finished => "finished",
                    SegmentKind::Active => "active",
                },
                "active_position": self.segment.active_position().map(|value| value.to_string()),
            },
            "logical_name": self.logical_name,
            "order": "physical_asc",
        }))?) {
            return Ok(());
        }
        for plan in &self.plans {
            let fields = plan
                .fields
                .iter()
                .map(|field| {
                    (
                        field.name.as_str(),
                        field.column.and_then(|name| plan.contract.column(name)),
                    )
                })
                .collect::<Vec<_>>();
            if !emit(record(json!({
                "record": "layout",
                "layout": projected_layout(&self.logical_name, plan.contract, &fields),
            }))?) {
                return Ok(());
            }
            let mut failure = None;
            let mut connected = true;
            let mut chunk = Vec::with_capacity(ROW_CHUNK_ROWS);
            self.segment.visit_rows(
                plan.type_id,
                &plan.projection,
                0,
                usize::MAX,
                |ordinal, row| {
                    chunk.push((ordinal, row));
                    if chunk.len() < ROW_CHUNK_ROWS {
                        return true;
                    }
                    match emit_chunk(&self.segment, plan, &mut chunk, emit) {
                        Ok(still_connected) => connected = still_connected,
                        Err(error) => failure = Some(error),
                    }
                    connected && failure.is_none()
                },
            )?;
            if failure.is_none() && connected && !chunk.is_empty() {
                match emit_chunk(&self.segment, plan, &mut chunk, emit) {
                    Ok(still_connected) => connected = still_connected,
                    Err(error) => failure = Some(error),
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            if !connected {
                return Ok(());
            }
        }
        Ok(())
    }
}

fn emit_chunk(
    segment: &Segment,
    plan: &Plan,
    rows: &mut Vec<(u64, Row)>,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
) -> Result<bool, ApiError> {
    let dictionary = chunk_dictionary(segment, rows)?;
    for (ordinal, row) in rows.drain(..) {
        if !plan.matches(&row, &dictionary) {
            continue;
        }
        let identity = plan
            .contract
            .identity
            .iter()
            .map(|name| {
                row.get(name).map_or(Ok(serde_json::Value::Null), |value| {
                    cell(value, &dictionary)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let timestamp = plan
            .timestamp
            .and_then(|name| row.get(name))
            .map_or(Ok(serde_json::Value::Null), |value| {
                cell(value, &dictionary)
            })?;
        let values = plan
            .fields
            .iter()
            .map(|field| {
                field
                    .column
                    .and_then(|name| row.get(name))
                    .map_or(Ok(serde_json::Value::Null), |value| {
                        cell(value, &dictionary)
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !emit(record(json!({
            "record": "row",
            "type_id": plan.type_id.to_string(),
            "ordinal": ordinal.to_string(),
            "timestamp": timestamp,
            "identity": identity,
            "values": values,
        }))?) {
            return Ok(false);
        }
    }
    Ok(true)
}
