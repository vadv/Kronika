//! Projected full-resolution history, streamed per physical layout and identity.

use kronika_reader::{Cell, Row, Segment};
use serde_json::{Value, json};

use super::ApiError;
use super::query::{Plan, streaming_chunk_dictionary, validate_row_dictionary};
use super::render::{cell, projected_layout, record};
use crate::route::Window;

const ROW_CHUNK_ROWS: usize = 512;

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
    let dictionary = streaming_chunk_dictionary(segment, rows)?;
    for (ordinal, row) in rows.drain(..) {
        if cancelled() {
            return Ok(false);
        }
        validate_row_dictionary(&row, &dictionary)?;
        if !plan.matches(&row, &dictionary) {
            continue;
        }
        let identity = plan
            .contract
            .identity
            .iter()
            .map(|name| {
                row.get(name)
                    .map_or(Ok(Value::Null), |value| cell(value, &dictionary))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let timestamp = plan
            .timestamp
            .and_then(|name| row.get(name))
            .map_or(Ok(Value::Null), |value| cell(value, &dictionary))?;
        let values = plan
            .fields
            .iter()
            .map(|field| {
                field
                    .column
                    .and_then(|name| row.get(name))
                    .map_or(Ok(Value::Null), |value| cell(value, &dictionary))
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
    window: Option<Window>,
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
                if window.is_some_and(|window| {
                    !plan
                        .timestamp
                        .and_then(|column| row.get(column))
                        .is_some_and(|cell| matches!(cell, Cell::Ts(ts) if window.contains(*ts)))
                }) {
                    return true;
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
