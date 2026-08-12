//! Projected full-resolution history, streamed per physical layout and identity.

use std::collections::BTreeSet;
use std::path::Path;

use kronika_index::{OS_PSI_TYPE_ID, visit_health_points};
use kronika_reader::{Row, Segment, SegmentKind};
use serde_json::json;

use super::query::{Plan, apply_tail, chunk_dictionary, plans};
use super::render::{cell, projected_layout, record};
use super::{ApiError, CachePolicy, ResponseMeta, active_tail, explicit_segment};
use crate::route::{ActiveCursor, DataRequest};

const ROW_CHUNK_ROWS: usize = 16;

pub(crate) struct PreparedHistory {
    segment: Segment,
    logical_name: String,
    plans: Vec<Plan>,
    after: Option<ActiveCursor>,
    health: Option<HealthPlan>,
}

#[derive(Debug, Clone, Copy)]
struct HealthPlan {
    field_count: usize,
    psi_start_row: u64,
}

pub(super) fn prepare(root: &Path, request: DataRequest) -> Result<PreparedHistory, ApiError> {
    let (reader, segment_ref) = explicit_segment(root, request.segment.segment_id)?;
    let tail = active_tail(&segment_ref, request.after)?;
    let segment = reader.open_segment(&segment_ref)?;
    let (plans, health) = if request.segment.section == "health" {
        (Vec::new(), Some(health_plan(&request, tail.as_ref())?))
    } else {
        let mut plans = plans(&segment, &request, true)?;
        apply_tail(&mut plans, tail.as_ref())?;
        (plans, None)
    };
    Ok(PreparedHistory {
        segment,
        logical_name: request.segment.section,
        plans,
        after: request.after,
        health,
    })
}

impl PreparedHistory {
    pub(super) const fn meta(&self) -> ResponseMeta {
        ResponseMeta::ok(match self.segment.kind() {
            SegmentKind::Finished => CachePolicy::Immutable,
            SegmentKind::Active => CachePolicy::NoStore,
        })
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        if cancelled()
            || !emit(record(json!({
                "record": "history",
                "segment": {
                    "id": self.segment.id().to_string(),
                    "kind": match self.segment.kind() {
                        SegmentKind::Finished => "finished",
                        SegmentKind::Active => "active",
                    },
                    "cursor": self.segment.active_position().map(|wal_position| json!({
                        "segment_id": self.segment.id().to_string(),
                        "wal_position": wal_position.to_string(),
                    })),
                },
                "logical_name": self.logical_name,
                "order": "physical_asc",
                "after": self.after.map(|cursor| json!({
                    "segment_id": cursor.segment_id.to_string(),
                    "wal_position": cursor.wal_position.to_string(),
                })),
            }))?)
        {
            return Ok(());
        }
        if let Some(health) = self.health {
            return self.stream_health(health, emit, cancelled);
        }
        stream_plans(
            &self.segment,
            &self.logical_name,
            &self.plans,
            emit,
            cancelled,
        )
        .map(|_connected| ())
    }

    fn stream_health(
        &self,
        plan: HealthPlan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        if cancelled()
            || !emit(record(json!({
                "record": "layout",
                "layout": super::index::section_layout("health", 0)?,
            }))?)
        {
            return Ok(());
        }
        let mut connected = true;
        let tail_timestamps = if self.after.is_some() {
            let mut timestamps = BTreeSet::new();
            self.segment.visit_rows(
                OS_PSI_TYPE_ID,
                &["ts"],
                plan.psi_start_row,
                usize::MAX,
                |_ordinal, row| {
                    if cancelled() {
                        connected = false;
                        return false;
                    }
                    if let Some(kronika_reader::Cell::Ts(timestamp)) = row.get("ts") {
                        timestamps.insert(*timestamp);
                    }
                    true
                },
            )?;
            Some(timestamps)
        } else {
            None
        };
        if !connected {
            return Ok(());
        }
        let mut ordinal = 0_u64;
        let mut failure = None;
        visit_health_points(
            &self.segment,
            || !cancelled(),
            |point| {
                let point_ordinal = ordinal;
                ordinal = ordinal.saturating_add(1);
                if tail_timestamps
                    .as_ref()
                    .is_some_and(|timestamps| !timestamps.contains(&point.timestamp))
                {
                    return true;
                }
                let value = point
                    .value
                    .map_or(serde_json::Value::Null, |value| json!(value));
                let values = vec![value; plan.field_count];
                match record(json!({
                    "record": "row",
                    "type_id": "0",
                    "ordinal": point_ordinal.to_string(),
                    "timestamp": point.timestamp.to_string(),
                    "identity": [],
                    "values": values,
                })) {
                    Ok(bytes) => connected = emit(bytes),
                    Err(error) => {
                        failure = Some(error);
                        connected = false;
                    }
                }
                connected
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }
}

fn health_plan(
    request: &DataRequest,
    tail: Option<&kronika_reader::SegmentRef>,
) -> Result<HealthPlan, ApiError> {
    for field in &request.fields {
        if field != "health" {
            return Err(ApiError::NoSuchColumn(field.clone()));
        }
    }
    if let Some(filter) = request.filters.first() {
        return Err(ApiError::BadFilter(filter.column.clone()));
    }
    let psi_start_row = tail
        .and_then(|segment| {
            segment
                .sections()
                .iter()
                .find(|section| section.type_id == OS_PSI_TYPE_ID)
        })
        .map_or(0, |section| section.rows);
    Ok(HealthPlan {
        field_count: request.fields.len().max(1),
        psi_start_row,
    })
}

fn emit_chunk(
    segment: &Segment,
    plan: &Plan,
    rows: &mut Vec<(u64, Row)>,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    if cancelled() {
        return Ok(false);
    }
    let dictionary = chunk_dictionary(segment, rows)?;
    for (ordinal, row) in rows.drain(..) {
        if cancelled() {
            return Ok(false);
        }
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

pub(super) fn stream_plans(
    segment: &Segment,
    logical_name: &str,
    plans: &[Plan],
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    for plan in plans {
        if cancelled() {
            return Ok(false);
        }
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
            "layout": projected_layout(logical_name, plan.contract, &fields),
        }))?) {
            return Ok(false);
        }
        if !plan.applies() {
            continue;
        }
        let mut failure = None;
        let mut connected = true;
        let mut chunk = Vec::with_capacity(ROW_CHUNK_ROWS);
        segment.visit_rows(
            plan.type_id,
            &plan.projection,
            plan.start_row,
            usize::MAX,
            |ordinal, row| {
                if cancelled() {
                    connected = false;
                    return false;
                }
                chunk.push((ordinal, row));
                if chunk.len() < ROW_CHUNK_ROWS {
                    return true;
                }
                match emit_chunk(segment, plan, &mut chunk, emit, cancelled) {
                    Ok(still_connected) => connected = still_connected,
                    Err(error) => failure = Some(error),
                }
                connected && failure.is_none()
            },
        )?;
        if failure.is_none() && connected && !chunk.is_empty() {
            match emit_chunk(segment, plan, &mut chunk, emit, cancelled) {
                Ok(still_connected) => connected = still_connected,
                Err(error) => failure = Some(error),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if !connected {
            return Ok(false);
        }
    }
    Ok(true)
}
