//! Reads one snapshot and derives counter rates.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use kronika_reader::{Cell, Dictionary, Reader, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, contract};
use serde_json::{Value, json};

use super::query::{Plan, plans, resolved_dictionary};
use super::render::{cell, projected_layout, record, shorten};
use super::{ApiError, CachePolicy, ResponseMeta, explicit_segment_with_listing};
use crate::route::{DataRequest, SegmentRequest, SnapshotRequest};

pub(crate) struct PreparedSnapshot {
    segment: Segment,
    earlier: Option<Segment>,
    at: i64,
    sections: Vec<SectionPlans>,
    by: Vec<String>,
    top: Option<usize>,
    text: Option<usize>,
    row_ordinal: Option<u64>,
}

type Readings = BTreeMap<Vec<IdentityCell>, CounterReadings>;
type CounterReadings = BTreeMap<&'static str, Cell>;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum IdentityCell {
    Null,
    I16(i16),
    I32(i32),
    I64(i64),
    Ts(i64),
    U32(u32),
    U64(u64),
    F64(u64),
    Bool(bool),
    StrId(u64),
    ListI32(Vec<i32>),
}

struct StagedRow {
    ordinal: u64,
    row: Row,
    identity: Vec<IdentityCell>,
}

#[derive(Clone, Copy)]
struct RateContext<'a> {
    previous: Option<&'a Readings>,
    elapsed: Option<i64>,
}

struct SectionPlans {
    logical_name: String,
    plans: Vec<Plan>,
}

pub(super) fn prepare(root: &Path, request: SnapshotRequest) -> Result<PreparedSnapshot, ApiError> {
    let (reader, segment_ref, segments) = explicit_segment_with_listing(root, request.segment_id)?;
    let segment = reader.open_segment(&segment_ref)?;
    let earlier = preceding(&reader, &segment_ref, segments)?;
    let shared_projection = request.sections.len() > 1 && !request.fields.is_empty();
    if shared_projection {
        validate_shared_projection(&segment, &request.sections, &request.fields)?;
    }
    let mut sections = Vec::with_capacity(request.sections.len());
    for logical_name in request.sections {
        let fields = if shared_projection {
            section_projection(&segment, &logical_name, &request.fields)
        } else {
            request.fields.clone()
        };
        if shared_projection && fields.is_empty() {
            continue;
        }
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: request.segment_id,
                section: logical_name.clone(),
            },
            fields,
            filters: request.filters.clone(),
            type_id: request.type_id,
            after: None,
        };
        // Missing sections are empty so one source cannot fail the snapshot.
        match plans(&segment, &data, true) {
            Ok(plans) => sections.push(SectionPlans {
                logical_name,
                plans,
            }),
            Err(ApiError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    if let Some(ordinal) = request.row_ordinal {
        if segment.kind() != SegmentKind::Finished {
            return Err(ApiError::BadCursor);
        }
        let [section] = sections.as_slice() else {
            return Err(ApiError::BadCursor);
        };
        let [plan] = section.plans.as_slice() else {
            return Err(ApiError::BadCursor);
        };
        let Some(timestamp) = plan.timestamp else {
            return Err(ApiError::BadCursor);
        };
        if ordinal >= plan.rows {
            return Err(ApiError::BadCursor);
        }
        let mut exact = false;
        segment.visit_rows(plan.type_id, &[timestamp], ordinal, 1, |_stored, row| {
            exact = row_timestamp(&row, timestamp) == Some(request.at);
            false
        })?;
        if !exact {
            return Err(ApiError::BadCursor);
        }
    }
    Ok(PreparedSnapshot {
        segment,
        earlier,
        at: request.at,
        sections,
        by: request.by,
        top: request.top,
        text: request.text,
        row_ordinal: request.row_ordinal,
    })
}

impl PreparedSnapshot {
    pub(super) const fn meta(&self) -> ResponseMeta {
        ResponseMeta::ok(match self.segment.kind() {
            SegmentKind::Finished => CachePolicy::Revalidate,
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
                "record": "snapshot",
                "segment": { "id": self.segment.id().to_string() },
                "at": self.at.to_string(),
            }))?)
        {
            return Ok(());
        }
        for section in &self.sections {
            for plan in &section.plans {
                if cancelled() {
                    return Ok(());
                }
                if !self.emit_section(section, plan, emit, cancelled)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn emit_section(
        &self,
        section: &SectionPlans,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        if !Self::emit_layout(section, plan, emit)? {
            return Ok(false);
        }
        if !plan.applies() {
            return Ok(true);
        }
        let Some(timestamp) = plan.timestamp else {
            return self.emit_untimed(plan, emit, cancelled);
        };
        // A section's latest sample may be in the preceding segment.
        let here = Self::moments(&self.segment, plan, timestamp, self.at, cancelled)?;
        let own = here.is_some();
        let source = if own {
            &self.segment
        } else {
            let Some(earlier) = self.earlier.as_ref() else {
                return Ok(true);
            };
            earlier
        };
        let Some(moments) = (if own {
            here
        } else {
            Self::moments(source, plan, timestamp, self.at, cancelled)?
        }) else {
            return Ok(true);
        };
        let (previous, before_at) = match moments.previous {
            Some(previous) => (
                Self::collect(source, plan, timestamp, previous, cancelled)?,
                Some(previous),
            ),
            None if own => self.earlier_moment(plan, timestamp, cancelled)?,
            None => (Readings::new(), None),
        };
        let elapsed = moments
            .current
            .checked_sub(before_at.unwrap_or(moments.current))
            .filter(|delta| *delta > 0);
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let mut rows = Vec::new();
        source.visit_rows(
            plan.type_id,
            &plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                if row_timestamp(&row, timestamp) != Some(moments.current) {
                    return true;
                }
                rows.push((ordinal, row));
                true
            },
        )?;
        if cancelled() {
            return Ok(false);
        }
        self.emit_rows(
            source,
            plan,
            rows,
            RateContext {
                previous: Some(&previous),
                elapsed,
            },
            emit,
            cancelled,
        )
    }

    fn order_and_truncate(&self, plan: &Plan, rates: RateContext<'_>, rows: &mut Vec<StagedRow>) {
        if let Some(column) = self.by.iter().find_map(|name| {
            available_field_index(&plan.fields, name).and_then(|index| plan.fields[index].column)
        }) {
            rows.sort_by(|left, right| compare_staged(plan, column, right, left, rates));
        }
        if let Some(top) = self.top {
            rows.truncate(top);
        }
    }

    fn emit_layout(
        section: &SectionPlans,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
    ) -> Result<bool, ApiError> {
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
        Ok(emit(record(json!({
            "record": "layout",
            "layout": projected_layout(&section.logical_name, plan.contract, &fields),
            "rates": rate_columns(plan),
        }))?))
    }

    fn emit_untimed(
        &self,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let mut rows = Vec::new();
        self.segment.visit_rows(
            plan.type_id,
            &plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                rows.push((ordinal, row));
                true
            },
        )?;
        if cancelled() {
            return Ok(false);
        }
        self.emit_rows(
            &self.segment,
            plan,
            rows,
            RateContext {
                previous: None,
                elapsed: None,
            },
            emit,
            cancelled,
        )
    }

    fn emit_rows(
        &self,
        source: &Segment,
        plan: &Plan,
        rows: Vec<(u64, Row)>,
        rates: RateContext<'_>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let selection_dictionary = plan.selection_dictionary(source, &rows)?;
        let mut staged = Vec::with_capacity(rows.len());
        for (ordinal, row) in rows {
            if !plan.matches(&row, &selection_dictionary) {
                continue;
            }
            let Some(identity) = identity_of(plan, &row) else {
                continue;
            };
            staged.push(StagedRow {
                ordinal,
                row,
                identity,
            });
        }
        self.order_and_truncate(plan, rates, &mut staged);
        let dictionary = retained_dictionary(source, &staged)?;
        for staged in staged {
            let before = rates
                .previous
                .and_then(|previous| previous.get(&staged.identity));
            let value = Self::row_record(
                plan,
                &staged.row,
                before,
                rates.elapsed,
                staged.ordinal,
                &dictionary,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn moments(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        at: i64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Moments>, ApiError> {
        let mut current: Option<i64> = None;
        let mut previous: Option<i64> = None;
        segment.visit_rows(
            plan.type_id,
            &[timestamp],
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                let Some(stored) = row_timestamp(&row, timestamp) else {
                    return true;
                };
                if stored > at {
                    return true;
                }
                match current {
                    Some(chosen) if stored == chosen => {}
                    Some(chosen) if stored > chosen => {
                        previous = Some(chosen);
                        current = Some(stored);
                    }
                    Some(chosen)
                        if previous.is_none_or(|before| stored > before) && stored < chosen =>
                    {
                        previous = Some(stored);
                    }
                    Some(_) => {}
                    None => current = Some(stored),
                }
                true
            },
        )?;
        Ok(current.map(|current| Moments { current, previous }))
    }

    fn collect(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        at: i64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Readings, ApiError> {
        let mut collected = BTreeMap::new();
        let counters = rate_columns(plan);
        if counters.is_empty() {
            return Ok(collected);
        }
        let mut projection = counters.clone();
        projection.extend(plan.contract.identity.iter().copied());
        projection.push(timestamp);
        projection.sort_unstable();
        projection.dedup();
        let mut rows = Vec::new();
        segment.visit_rows(plan.type_id, &projection, 0, usize::MAX, |ordinal, row| {
            if cancelled() {
                return false;
            }
            if row_timestamp(&row, timestamp) != Some(at) {
                return true;
            }
            rows.push((ordinal, row));
            true
        })?;
        if cancelled() {
            return Ok(collected);
        }
        for (_ordinal, row) in rows {
            let Some(key) = identity_of(plan, &row) else {
                continue;
            };
            let mut stored = BTreeMap::new();
            for name in &counters {
                if let Some(value) = row.get(name) {
                    stored.insert(*name, value.clone());
                }
            }
            collected.insert(key, stored);
        }
        Ok(collected)
    }

    fn earlier_moment(
        &self,
        plan: &Plan,
        timestamp: &'static str,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(Readings, Option<i64>), ApiError> {
        let Some(earlier) = self.earlier.as_ref() else {
            return Ok((BTreeMap::new(), None));
        };
        let mut last: Option<i64> = None;
        earlier.visit_rows(
            plan.type_id,
            &[timestamp],
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                if let Some(stored) = row_timestamp(&row, timestamp)
                    && last.is_none_or(|chosen| stored > chosen)
                {
                    last = Some(stored);
                }
                true
            },
        )?;
        let Some(at) = last else {
            return Ok((BTreeMap::new(), None));
        };
        Ok((
            Self::collect(earlier, plan, timestamp, at, cancelled)?,
            Some(at),
        ))
    }

    fn row_record(
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        ordinal: u64,
        dictionary: &Dictionary,
        text_limit: Option<usize>,
    ) -> Result<Value, ApiError> {
        let stamped = plan.timestamp.and_then(|column| row_timestamp(row, column));
        let mut values = Vec::with_capacity(plan.fields.len());
        for field in &plan.fields {
            let Some(column) = field.column else {
                values.push(Value::Null);
                continue;
            };
            let stored = row.get(column);
            let is_rate = plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
            if is_rate {
                values.push(rate(stored, before, column, elapsed));
                continue;
            }
            let rendered = match stored {
                Some(stored) => cell(stored, dictionary)?,
                None => Value::Null,
            };
            values.push(match text_limit {
                Some(limit) => shorten(rendered, limit),
                None => rendered,
            });
        }
        Ok(json!({
            "record": "row",
            "type_id": plan.type_id.to_string(),
            "ordinal": ordinal.to_string(),
            "timestamp": stamped.map(|stored| stored.to_string()),
            "values": values,
        }))
    }
}

fn compare_staged(
    plan: &Plan,
    column: &'static str,
    left: &StagedRow,
    right: &StagedRow,
    rates: RateContext<'_>,
) -> Ordering {
    let value = |staged: &StagedRow| {
        let stored = staged.row.get(column)?;
        let cumulative = plan
            .contract
            .column(column)
            .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
        if cumulative {
            rates.elapsed?;
            let earlier = rates.previous?.get(&staged.identity)?.get(column)?;
            counter_delta(stored, earlier)
        } else {
            ordered_cell(stored)
        }
    };
    compare_ordered(value(left), value(right))
}

fn compare_ordered(left: Option<OrderedNumber>, right: Option<OrderedNumber>) -> Ordering {
    match (left, right) {
        (Some(OrderedNumber::Integer(left)), Some(OrderedNumber::Integer(right))) => {
            left.cmp(&right)
        }
        (Some(OrderedNumber::Float(left)), Some(OrderedNumber::Float(right))) => {
            left.partial_cmp(&right).unwrap_or(Ordering::Equal)
        }
        (Some(_), Some(_)) | (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
    }
}

fn ordered_cell(cell: &Cell) -> Option<OrderedNumber> {
    match cell {
        Cell::I16(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::I32(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::I64(value) | Cell::Ts(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::U32(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::U64(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::F64(value) if value.is_finite() => Some(OrderedNumber::Float(*value)),
        Cell::Bool(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::F64(_) | Cell::StrId(_) | Cell::ListI32(_) | Cell::Null => None,
    }
}

fn retained_dictionary(segment: &Segment, rows: &[StagedRow]) -> Result<Dictionary, ApiError> {
    let ids: HashSet<u64> = rows
        .iter()
        .flat_map(|staged| staged.row.iter())
        .filter_map(|(_name, cell)| match cell {
            Cell::StrId(id) => Some(*id),
            _ => None,
        })
        .collect();
    resolved_dictionary(segment, &ids)
}

fn available_field_index(fields: &[super::query::OutputField], name: &str) -> Option<usize> {
    fields
        .iter()
        .position(|field| field.name == name && field.column.is_some())
}

fn preceding(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segments: Vec<SegmentRef>,
) -> Result<Option<Segment>, ApiError> {
    let chosen = segments
        .into_iter()
        .filter(|candidate| candidate.max_ts() <= segment_ref.min_ts())
        .max_by_key(SegmentRef::max_ts);
    chosen
        .map(|candidate| reader.open_segment(&candidate))
        .transpose()
        .map_err(ApiError::from)
}

fn validate_shared_projection(
    segment: &Segment,
    sections: &[String],
    fields: &[String],
) -> Result<(), ApiError> {
    for field in fields {
        let known = sections.iter().any(|section| {
            segment
                .layouts(section)
                .filter_map(|(type_id, _section)| contract(type_id))
                .any(|layout| layout.column(field).is_some())
        });
        if !known {
            return Err(ApiError::NoSuchColumn(field.clone()));
        }
    }
    Ok(())
}

fn section_projection(segment: &Segment, logical_name: &str, fields: &[String]) -> Vec<String> {
    let columns = segment
        .layouts(logical_name)
        .filter_map(|(type_id, _section)| contract(type_id))
        .flat_map(|layout| layout.columns.iter().map(|column| column.name))
        .collect::<HashSet<_>>();
    fields
        .iter()
        .filter(|field| columns.contains(field.as_str()))
        .cloned()
        .collect()
}

struct Moments {
    current: i64,
    previous: Option<i64>,
}

/// Returns null without a valid nondecreasing predecessor.
fn rate(
    stored: Option<&Cell>,
    before: Option<&CounterReadings>,
    column: &'static str,
    elapsed: Option<i64>,
) -> Value {
    let (Some(now), Some(before), Some(elapsed)) = (stored, before, elapsed) else {
        return Value::Null;
    };
    let Some(earlier) = before.get(column) else {
        return Value::Null;
    };
    let Some(delta) = counter_delta(now, earlier) else {
        return Value::Null;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval of 2^52 microseconds is 142 years"
    )]
    let seconds = elapsed as f64 / 1_000_000.0;
    let value = delta.as_f64() / seconds;
    if value.is_finite() {
        json!(value)
    } else {
        Value::Null
    }
}

#[derive(Clone, Copy)]
enum OrderedNumber {
    Integer(i128),
    Float(f64),
}

impl OrderedNumber {
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer counter deltas are converted only after exact subtraction"
    )]
    const fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

fn counter_delta(now: &Cell, earlier: &Cell) -> Option<OrderedNumber> {
    let exact = match (now, earlier) {
        (Cell::I16(now), Cell::I16(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::I32(now), Cell::I32(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::I64(now) | Cell::Ts(now), Cell::I64(earlier) | Cell::Ts(earlier)) => {
            i128::from(*now) - i128::from(*earlier)
        }
        (Cell::U32(now), Cell::U32(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::U64(now), Cell::U64(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::F64(now), Cell::F64(earlier)) => {
            let delta = now - earlier;
            return (delta >= 0.0 && delta.is_finite()).then_some(OrderedNumber::Float(delta));
        }
        _ => return None,
    };
    (exact >= 0).then_some(OrderedNumber::Integer(exact))
}

fn rate_columns(plan: &Plan) -> Vec<&'static str> {
    plan.fields
        .iter()
        .filter_map(|field| field.column)
        .filter(|column| {
            plan.contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
        })
        .collect()
}

fn identity_of(plan: &Plan, row: &Row) -> Option<Vec<IdentityCell>> {
    if plan.contract.identity.is_empty() {
        return Some(Vec::new());
    }
    let mut identity = Vec::with_capacity(plan.contract.identity.len());
    for name in plan.contract.identity {
        let stored = row.get(name)?;
        identity.push(identity_cell(stored));
    }
    Some(identity)
}

fn identity_cell(stored: &Cell) -> IdentityCell {
    match stored {
        Cell::Null => IdentityCell::Null,
        Cell::I16(value) => IdentityCell::I16(*value),
        Cell::I32(value) => IdentityCell::I32(*value),
        Cell::I64(value) => IdentityCell::I64(*value),
        Cell::Ts(value) => IdentityCell::Ts(*value),
        Cell::U32(value) => IdentityCell::U32(*value),
        Cell::U64(value) => IdentityCell::U64(*value),
        Cell::F64(value) => IdentityCell::F64(value.to_bits()),
        Cell::Bool(value) => IdentityCell::Bool(*value),
        Cell::ListI32(value) => IdentityCell::ListI32(value.clone()),
        Cell::StrId(id) => IdentityCell::StrId(*id),
    }
}

fn row_timestamp(row: &Row, column: &'static str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::Ts(stored)) => Some(*stored),
        _other => None,
    }
}

#[cfg(test)]
mod tests;
