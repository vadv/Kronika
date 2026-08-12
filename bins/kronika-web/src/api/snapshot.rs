//! Reads one snapshot and derives counter rates.

use std::collections::BTreeMap;
use std::path::Path;

use kronika_reader::{Cell, Reader, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::ColumnClass;
use serde_json::{Value, json};

use super::query::{Plan, plans};
use super::render::{cell, projected_layout, record, shorten};
use super::{ApiError, CachePolicy, ResponseMeta, explicit_segment};
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

type Readings = BTreeMap<Vec<String>, CounterReadings>;
type CounterReadings = BTreeMap<&'static str, Cell>;

struct SectionPlans {
    logical_name: String,
    plans: Vec<Plan>,
}

pub(super) fn prepare(root: &Path, request: SnapshotRequest) -> Result<PreparedSnapshot, ApiError> {
    let (reader, segment_ref) = explicit_segment(root, request.segment_id)?;
    let segment = reader.open_segment(&segment_ref)?;
    let earlier = preceding(&reader, &segment_ref)?;
    let mut sections = Vec::with_capacity(request.sections.len());
    for logical_name in request.sections {
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: request.segment_id,
                section: logical_name.clone(),
            },
            fields: request.fields.clone(),
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
            return self.emit_untimed(section, plan, emit, cancelled);
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
        let dictionary = source.dictionary()?;
        let elapsed = moments
            .current
            .checked_sub(before_at.unwrap_or(moments.current))
            .filter(|delta| *delta > 0);
        let mut failure = None;
        let mut rows = Vec::new();
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        source.visit_rows(
            plan.type_id,
            &plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                if row_timestamp(&row, timestamp) != Some(moments.current)
                    || !plan.matches(&row, &dictionary)
                {
                    return true;
                }
                let before = identity_of(plan, &row).and_then(|key| previous.get(&key));
                match Self::row_record(plan, &row, before, elapsed, ordinal, &dictionary, self.text)
                {
                    Ok(value) => rows.push(value),
                    Err(error) => failure = Some(error),
                }
                failure.is_none()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if let Some(by) = self
            .by
            .iter()
            .find_map(|name| available_field_index(&plan.fields, name))
        {
            rows.sort_by(|left, right| {
                sort_value(right, by)
                    .partial_cmp(&sort_value(left, by))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        if let Some(top) = self.top {
            rows.truncate(top);
        }
        for value in rows {
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
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
        section: &SectionPlans,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let _ = section;
        let dictionary = self.segment.dictionary()?;
        let mut failure = None;
        let mut connected = true;
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        self.segment.visit_rows(
            plan.type_id,
            &plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    connected = false;
                    return false;
                }
                if !plan.matches(&row, &dictionary) {
                    return true;
                }
                match Self::row_record(plan, &row, None, None, ordinal, &dictionary, self.text) {
                    Ok(value) => match record(&value) {
                        Ok(bytes) => connected = emit(bytes),
                        Err(error) => failure = Some(error),
                    },
                    Err(error) => failure = Some(error),
                }
                connected && failure.is_none()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(connected)
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
        segment.visit_rows(
            plan.type_id,
            &plan.projection,
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                if row_timestamp(&row, timestamp) != Some(at) {
                    return true;
                }
                let Some(key) = identity_of(plan, &row) else {
                    return true;
                };
                let mut stored = BTreeMap::new();
                for name in &counters {
                    if let Some(value) = row.get(name) {
                        stored.insert(*name, value.clone());
                    }
                }
                collected.insert(key, stored);
                true
            },
        )?;
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
        dictionary: &kronika_reader::Dictionary,
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

fn sort_value(row: &Value, field: usize) -> f64 {
    row.get("values")
        .and_then(|values| values.get(field))
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY)
}

fn available_field_index(fields: &[super::query::OutputField], name: &str) -> Option<usize> {
    fields
        .iter()
        .position(|field| field.name == name && field.column.is_some())
}

fn preceding(reader: &Reader, segment_ref: &SegmentRef) -> Result<Option<Segment>, ApiError> {
    let listing = reader.catalog_segments(..)?;
    let chosen = listing
        .segments
        .into_iter()
        .filter(|candidate| candidate.id() != segment_ref.id())
        .filter(|candidate| candidate.max_ts() <= segment_ref.min_ts())
        .max_by_key(SegmentRef::max_ts);
    chosen
        .map(|candidate| reader.open_segment(&candidate))
        .transpose()
        .map_err(ApiError::from)
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
    let value = delta / seconds;
    if value.is_finite() {
        json!(value)
    } else {
        Value::Null
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "integer counter deltas are converted only after exact subtraction"
)]
fn counter_delta(now: &Cell, earlier: &Cell) -> Option<f64> {
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
            return (delta >= 0.0 && delta.is_finite()).then_some(delta);
        }
        _ => return None,
    };
    (exact >= 0).then_some(exact as f64)
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

fn identity_of(plan: &Plan, row: &Row) -> Option<Vec<String>> {
    if plan.contract.identity.is_empty() {
        return Some(Vec::new());
    }
    plan.contract
        .identity
        .iter()
        .map(|name| row.get(name).map(identity_text))
        .collect()
}

fn identity_text(stored: &Cell) -> String {
    match stored {
        Cell::Null => String::new(),
        Cell::I16(value) => value.to_string(),
        Cell::I32(value) => value.to_string(),
        Cell::I64(value) | Cell::Ts(value) => value.to_string(),
        Cell::U32(value) => value.to_string(),
        Cell::U64(value) => value.to_string(),
        Cell::F64(value) => value.to_string(),
        Cell::Bool(value) => value.to_string(),
        Cell::StrId(value) => format!("s{value}"),
        Cell::ListI32(value) => format!("{value:?}"),
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
