//! Projected full-resolution history, streamed per physical layout and identity.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use kronika_index::{OS_PSI_TYPE_ID, visit_health_points};
use kronika_reader::{Cell, Reader, Row, Segment, SegmentKind, SegmentRef};
use serde_json::{Value, json};

use super::query::{Plan, apply_tail, plans, streaming_chunk_dictionary, validate_row_dictionary};
use super::render::{cell, projected_layout, record};
use super::{ApiError, CachePolicy, ResponseMeta, active_tail, explicit_segment};
use crate::route::{ActiveCursor, DataRequest, SegmentRequest, Window};

const ROW_CHUNK_ROWS: usize = 512;

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
            None,
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
                    if let Some(Cell::Ts(timestamp)) = row.get("ts") {
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
                let value = point.value.map_or(Value::Null, |value| json!(value));
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

fn health_plan(request: &DataRequest, tail: Option<&SegmentRef>) -> Result<HealthPlan, ApiError> {
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

/// One event-log row bounded by [`fetch_bounded_events`], keyed by column
/// name instead of `stream_plans`'s positional `values` array.
/// `segment_id`/`type_id`/`row_ordinal`/`at` are the same
/// `kronika_get_row_detail` locator `ProcessRowOut`/`PlainRowOut`
/// (`api/snapshot.rs`) carry, so a caller can chain straight into that tool.
pub(crate) struct EventRowOut {
    pub(crate) segment_id: i64,
    pub(crate) type_id: u32,
    pub(crate) row_ordinal: u64,
    pub(crate) at: i64,
    pub(crate) fields: BTreeMap<String, Value>,
}

/// One logical section's bounded rows from [`fetch_bounded_events`], plus
/// whether more rows matched than were returned for that section alone.
pub(crate) struct SectionEvents<'a> {
    pub(crate) section: &'a str,
    pub(crate) rows: Vec<EventRowOut>,
    pub(crate) has_more: bool,
}

/// Bounded top-N read of one or more logical sections across `segments`,
/// keyed by field name for MCP consumers instead of `stream_plans`'s
/// positional wire format. Each segment is opened at most once and its
/// catalog shared across every requested section, rather than reopening the
/// same physical file once per section — `kronika_find_events` reads up to
/// seven sections over the same window, and `Segment::open` is a real file
/// open plus catalog parse, not a cheap wrap.
///
/// Physical row order within a segment is append order, which for a
/// log-derived event section is already chronological, and `segments` is
/// scanned in the order given (callers pass them sorted by `min_ts`, the
/// same order `PreparedHour` streams). That makes "collect the first
/// `limit` matching rows and stop" a correct timestamp-ascending bound on
/// its own — unlike `PreparedSnapshot`'s `PageRows`, nothing here ranks by
/// an arbitrary sort column, so no heap is needed. Each section keeps its
/// own bound and its own `has_more`, exactly as if fetched independently;
/// only the segment opens are shared.
pub(crate) fn fetch_bounded_events<'a>(
    reader: &Reader,
    segments: &[SegmentRef],
    sections: &[&'a str],
    window: Window,
    limit: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<SectionEvents<'a>>, ApiError> {
    let bound = limit.saturating_add(1);
    let mut rows: Vec<Vec<EventRowOut>> = sections.iter().map(|_| Vec::new()).collect();
    'segments: for segment_ref in segments {
        if cancelled() || rows.iter().all(|section_rows| section_rows.len() >= bound) {
            break;
        }
        let segment = reader.open_segment(segment_ref)?;
        for (section, section_rows) in sections.iter().zip(rows.iter_mut()) {
            if cancelled() {
                break 'segments;
            }
            if section_rows.len() >= bound {
                continue;
            }
            let request = DataRequest {
                segment: SegmentRequest {
                    segment_id: segment_ref.id(),
                    section: (*section).to_owned(),
                },
                fields: Vec::new(),
                filters: Vec::new(),
                type_id: None,
                after: None,
            };
            let section_plans = match plans(&segment, &request, true) {
                Ok(section_plans) => section_plans,
                Err(ApiError::NoSuchSection) => continue,
                Err(error) => return Err(error),
            };
            for plan in &section_plans {
                if cancelled() || section_rows.len() >= bound {
                    break;
                }
                if !plan.applies() {
                    continue;
                }
                collect_bounded_rows(
                    &segment,
                    segment_ref.id(),
                    plan,
                    window,
                    bound,
                    section_rows,
                    cancelled,
                )?;
            }
        }
    }
    Ok(sections
        .iter()
        .zip(rows)
        .map(|(section, mut rows)| {
            let has_more = rows.len() > limit;
            rows.truncate(limit);
            SectionEvents {
                section,
                rows,
                has_more,
            }
        })
        .collect())
}

/// Scans one plan's rows for [`fetch_bounded_events`], stopping as soon as
/// `rows` reaches `bound` (`limit + 1`, so the caller can tell `has_more`
/// apart from "exactly `limit` rows exist"). Flushes the pending chunk
/// early, before `ROW_CHUNK_ROWS`, once it alone would satisfy `bound` —
/// `emit_chunk`'s streaming caller always wants every row, so it only ever
/// flushes at the full batch size, but a bounded fetch must not decode
/// hundreds of rows it is about to throw away just to fill one batch.
fn collect_bounded_rows(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    window: Window,
    bound: usize,
    rows: &mut Vec<EventRowOut>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let Some(timestamp_column) = plan.timestamp else {
        return Ok(());
    };
    let mut failure = None;
    let mut chunk: Vec<(u64, Row)> = Vec::with_capacity(ROW_CHUNK_ROWS);
    segment.visit_rows(
        plan.type_id,
        &plan.projection,
        plan.start_row,
        usize::MAX,
        |ordinal, row| {
            if cancelled() || rows.len() >= bound {
                return false;
            }
            if !matches!(row.get(timestamp_column), Some(Cell::Ts(at)) if window.contains(*at)) {
                return true;
            }
            chunk.push((ordinal, row));
            if chunk.len() < ROW_CHUNK_ROWS && rows.len() + chunk.len() < bound {
                return true;
            }
            if let Err(error) = append_chunk(
                segment,
                segment_id,
                plan,
                timestamp_column,
                &mut chunk,
                rows,
                bound,
            ) {
                failure = Some(error);
                return false;
            }
            rows.len() < bound
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if !chunk.is_empty() && rows.len() < bound {
        append_chunk(
            segment,
            segment_id,
            plan,
            timestamp_column,
            &mut chunk,
            rows,
            bound,
        )?;
    }
    Ok(())
}

/// Renders one drained chunk into [`EventRowOut`]s, the same
/// dictionary-per-chunk resolution `emit_chunk` uses, reshaped to a keyed
/// map through the same [`cell`] renderer instead of a positional array.
fn append_chunk(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    timestamp_column: &str,
    chunk: &mut Vec<(u64, Row)>,
    rows: &mut Vec<EventRowOut>,
    bound: usize,
) -> Result<(), ApiError> {
    let dictionary = streaming_chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        validate_row_dictionary(&row, &dictionary)?;
        if rows.len() >= bound {
            break;
        }
        let Some(Cell::Ts(at)) = row.get(timestamp_column) else {
            continue;
        };
        let mut fields = BTreeMap::new();
        for field in &plan.fields {
            let value = field
                .column
                .and_then(|name| row.get(name))
                .map_or(Ok(Value::Null), |value| cell(value, &dictionary))?;
            fields.insert(field.name.clone(), value);
        }
        rows.push(EventRowOut {
            segment_id,
            type_id: plan.type_id,
            row_ordinal: ordinal,
            at: *at,
            fields,
        });
    }
    Ok(())
}
