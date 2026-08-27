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

/// Recorded event row keyed by field name, with the exact locator accepted by
/// `kronika_get_row_detail`.
pub(crate) struct EventRowOut {
    pub(crate) segment_id: i64,
    pub(crate) type_id: u32,
    pub(crate) row_ordinal: u64,
    pub(crate) at: i64,
    pub(crate) fields: BTreeMap<String, Value>,
}

/// Bounded rows for one logical section; `has_more` is section-local.
pub(crate) struct SectionEvents<'a> {
    pub(crate) section: &'a str,
    pub(crate) rows: Vec<EventRowOut>,
    pub(crate) has_more: bool,
}

/// An [`EventRowOut`] ordered by `(at, segment_id, type_id, row_ordinal)`,
/// so [`BoundedRows`]'s max-heap can evict the latest row it holds.
struct OrderedRow(EventRowOut);

impl OrderedRow {
    const fn key(&self) -> (i64, i64, u32, u64) {
        (
            self.0.at,
            self.0.segment_id,
            self.0.type_id,
            self.0.row_ordinal,
        )
    }
}

impl PartialEq for OrderedRow {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for OrderedRow {}

impl PartialOrd for OrderedRow {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedRow {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

/// The `limit` earliest matching rows seen so far for one section: a
/// max-heap that evicts its latest row on overflow, remembering that it
/// overflowed. Stored row order within a segment follows the section's
/// registry sort key, which for several event sections leads with
/// something other than `ts` — so "keep the first `limit` physical rows"
/// would drop an earlier event that merely appears later in the file.
struct BoundedRows {
    bound: usize,
    heap: std::collections::BinaryHeap<OrderedRow>,
    overflowed: bool,
}

impl BoundedRows {
    fn new(bound: usize) -> Self {
        Self {
            bound: bound.max(1),
            heap: std::collections::BinaryHeap::new(),
            overflowed: false,
        }
    }

    fn push(&mut self, row: EventRowOut) {
        self.heap.push(OrderedRow(row));
        if self.heap.len() > self.bound {
            self.heap.pop();
            self.overflowed = true;
        }
    }

    /// The latest `at` this collector still holds, once full: any row at or
    /// past it may still evict one, any row before it must be considered.
    fn worst_at(&self) -> Option<i64> {
        if self.heap.len() < self.bound {
            return None;
        }
        self.heap.peek().map(|row| row.0.at)
    }

    /// Whether a segment whose rows all sit at or after `min_ts` can be
    /// skipped for this section: only when the collector is full and even
    /// the earliest possible row of that segment is later than everything
    /// held.
    fn satisfied_before(&self, min_ts: i64) -> bool {
        self.worst_at().is_some_and(|worst| min_ts > worst)
    }

    /// Records that a matching row was dropped — the `has_more` the
    /// caller reports.
    const fn note_more(&mut self) {
        self.overflowed = true;
    }

    fn finish(self) -> (Vec<EventRowOut>, bool) {
        let rows = self
            .heap
            .into_sorted_vec()
            .into_iter()
            .map(|row| row.0)
            .collect();
        (rows, self.overflowed)
    }
}

/// Whether a segment's catalog lists any rows for `section` — readable
/// without opening the segment, which is the point: the skip path must
/// know whether it is leaving rows behind.
fn segment_carries(segment_ref: &SegmentRef, section: &str) -> bool {
    segment_ref.sections().iter().any(|stored| {
        stored.rows > 0 && kronika_registry::logical_section_name(stored.type_id) == Some(section)
    })
}

/// The catalog has no per-section time range, so a full collector still
/// scans a carried in-window segment — only to count dropped matches into
/// `has_more`, never to push.
fn skippable(
    segment_ref: &SegmentRef,
    section: &str,
    section_rows: &BoundedRows,
    window: Window,
) -> bool {
    section_rows.satisfied_before(segment_ref.min_ts())
        && (!segment_carries(segment_ref, section)
            || window.to.is_some_and(|to| segment_ref.min_ts() > to))
}

/// Bounded top-N read of one or more logical sections across `segments`,
/// keyed by field name for MCP consumers instead of `stream_plans`'s
/// positional wire format. Each segment is opened at most once and its
/// catalog shared across every requested section, rather than reopening the
/// same physical file once per section — `kronika_find_events` reads up to
/// eight sections over the same window, and `Segment::open` is a real file
/// open plus catalog parse, not a cheap wrap.
///
/// Each section collects through a [`BoundedRows`] heap rather than
/// stopping at the first `limit` physical rows: stored row order follows
/// the section's registry sort key, not `ts`, so the earliest events of a
/// window can sit anywhere in a segment's data. `segments` is scanned in
/// the order given (callers pass them sorted by `min_ts`). A full
/// collector skips opening a segment only when nothing in it could change
/// the answer: the section is absent, or every row of the segment starts
/// past the window's `to`. A segment that might still hold matching rows
/// is scanned so `has_more` reports dropped matches, never a guess from
/// segment bounds. Each section keeps its own bound and its own
/// `has_more`, exactly as if fetched independently; only the segment
/// opens are shared.
pub(crate) fn fetch_bounded_events<'a>(
    reader: &Reader,
    segments: &[SegmentRef],
    sections: &[&'a str],
    window: Window,
    limit: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<SectionEvents<'a>>, ApiError> {
    let mut rows: Vec<BoundedRows> = sections.iter().map(|_| BoundedRows::new(limit)).collect();
    'segments: for segment_ref in segments {
        if cancelled() {
            break;
        }
        if sections
            .iter()
            .zip(rows.iter())
            .all(|(section, section_rows)| skippable(segment_ref, section, section_rows, window))
        {
            continue;
        }
        let segment = reader.open_segment(segment_ref)?;
        for (section, section_rows) in sections.iter().zip(rows.iter_mut()) {
            if cancelled() {
                break 'segments;
            }
            if skippable(segment_ref, section, section_rows, window) {
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
                if cancelled() {
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
                    section_rows,
                    cancelled,
                )?;
            }
        }
    }
    Ok(sections
        .iter()
        .zip(rows)
        .map(|(section, collected)| {
            let (rows, has_more) = collected.finish();
            SectionEvents {
                section,
                rows,
                has_more,
            }
        })
        .collect())
}

/// Scans one plan's rows for [`fetch_bounded_events`]. Every row in the
/// window is visited — stored order is the registry sort key, not `ts`, so
/// no physical prefix is a valid cut — but a row later than everything a
/// full collector holds is dropped before it costs a chunk slot or a
/// dictionary decode.
fn collect_bounded_rows(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    window: Window,
    rows: &mut BoundedRows,
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
            if cancelled() {
                return false;
            }
            let at = match row.get(timestamp_column) {
                Some(Cell::Ts(at)) if window.contains(*at) => *at,
                _ => return true,
            };
            if rows.worst_at().is_some_and(|worst| at > worst) {
                rows.note_more();
                return true;
            }
            chunk.push((ordinal, row));
            if chunk.len() < ROW_CHUNK_ROWS {
                return true;
            }
            if let Err(error) = append_chunk(
                segment,
                segment_id,
                plan,
                timestamp_column,
                &mut chunk,
                rows,
            ) {
                failure = Some(error);
                return false;
            }
            true
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if !chunk.is_empty() {
        append_chunk(
            segment,
            segment_id,
            plan,
            timestamp_column,
            &mut chunk,
            rows,
        )?;
    }
    Ok(())
}

/// Resolves one chunk's dictionary and renders keyed [`EventRowOut`] values.
fn append_chunk(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    timestamp_column: &str,
    chunk: &mut Vec<(u64, Row)>,
    rows: &mut BoundedRows,
) -> Result<(), ApiError> {
    let dictionary = streaming_chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        validate_row_dictionary(&row, &dictionary)?;
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

#[cfg(test)]
mod bounded_rows_tests {
    use super::{BoundedRows, EventRowOut};

    fn row(at: i64, row_ordinal: u64) -> EventRowOut {
        EventRowOut {
            segment_id: 1,
            type_id: 1,
            row_ordinal,
            at,
            fields: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn keeps_the_earliest_rows_regardless_of_arrival_order() {
        // Stored order follows the registry sort key, so an early event can
        // arrive after the collector already looks full.
        let mut collected = BoundedRows::new(2);
        collected.push(row(300, 0));
        collected.push(row(100, 1));
        collected.push(row(200, 2));
        let (rows, has_more) = collected.finish();
        assert_eq!(
            rows.iter().map(|row| row.at).collect::<Vec<_>>(),
            vec![100, 200]
        );
        assert!(has_more);
    }

    #[test]
    fn under_the_bound_nothing_is_dropped() {
        let mut collected = BoundedRows::new(3);
        collected.push(row(300, 0));
        collected.push(row(100, 1));
        let (rows, has_more) = collected.finish();
        assert_eq!(
            rows.iter().map(|row| row.at).collect::<Vec<_>>(),
            vec![100, 300]
        );
        assert!(!has_more);
    }

    #[test]
    fn a_full_collector_skips_segments_starting_after_its_worst_row() {
        let mut collected = BoundedRows::new(2);
        collected.push(row(100, 0));
        collected.push(row(200, 1));
        assert!(collected.satisfied_before(201));
        assert!(!collected.satisfied_before(200));
        let mut sparse = BoundedRows::new(2);
        sparse.push(row(100, 0));
        assert!(!sparse.satisfied_before(500));
    }
}
