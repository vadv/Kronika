//! One moment of several sections, with counters already turned into rates.
//!
//! A table shows a moment, not an hour, and it shows rates, not the running
//! totals a counter carries. Doing both here is one request where the client
//! would otherwise make one per section and then subtract for itself.

use std::collections::BTreeMap;
use std::path::Path;

use kronika_reader::{Cell, Row, Segment, SegmentKind};
use kronika_registry::ColumnClass;
use serde_json::{Value, json};

use super::query::{Plan, plans};
use super::render::{cell, projected_layout, record};
use super::{ApiError, CachePolicy, ResponseMeta, explicit_segment};
use crate::route::{DataRequest, SegmentRequest, SnapshotRequest};

pub(crate) struct PreparedSnapshot {
    segment: Segment,
    at: i64,
    sections: Vec<SectionPlans>,
    by: Option<String>,
    top: Option<usize>,
}

struct SectionPlans {
    logical_name: String,
    plans: Vec<Plan>,
}

pub(super) fn prepare(root: &Path, request: SnapshotRequest) -> Result<PreparedSnapshot, ApiError> {
    let (reader, segment_ref) = explicit_segment(root, request.segment_id)?;
    let segment = reader.open_segment(&segment_ref)?;
    let mut sections = Vec::with_capacity(request.sections.len());
    for logical_name in request.sections {
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: request.segment_id,
                section: logical_name.clone(),
            },
            fields: request.fields.clone(),
            filters: Vec::new(),
            after: None,
        };
        // A section reaches an active segment only with its first sample, so
        // a young one is missing most of them. An absent section is an empty
        // table, not a failed request for every other section beside it.
        // A projection asked for by name still has to carry the timestamp and the
        // identity: this reads a moment and subtracts the one before it.
        match plans(&segment, &data, true) {
            Ok(plans) => sections.push(SectionPlans {
                logical_name,
                plans,
            }),
            Err(ApiError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(PreparedSnapshot {
        segment,
        at: request.at,
        sections,
        by: request.by,
        top: request.top,
    })
}

impl PreparedSnapshot {
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
            "layout": projected_layout(&section.logical_name, plan.contract, &fields),
            "rates": rate_columns(plan),
        }))?) {
            return Ok(false);
        }
        if !plan.applies() {
            return Ok(true);
        }
        let Some(timestamp) = plan.timestamp else {
            return self.emit_untimed(section, plan, emit, cancelled);
        };
        let Some(moments) = self.moments(plan, timestamp, cancelled)? else {
            return Ok(true);
        };
        let previous = self.collect(plan, timestamp, moments.previous, cancelled)?;
        let dictionary = self.segment.dictionary()?;
        let elapsed = moments
            .current
            .checked_sub(moments.previous.unwrap_or(moments.current))
            .filter(|gap| *gap > 0);
        let mut failure = None;
        let mut rows = Vec::new();
        self.segment.visit_rows(
            plan.type_id,
            &plan.projection,
            0,
            usize::MAX,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                if row_timestamp(&row, timestamp) != Some(moments.current) {
                    return true;
                }
                let before = identity_of(plan, &row).and_then(|key| previous.get(&key));
                match Self::row_record(plan, &row, before, elapsed, ordinal, &dictionary) {
                    Ok(value) => rows.push(value),
                    Err(error) => failure = Some(error),
                }
                failure.is_none()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if let Some(by) = &self.by {
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

    /// Sections without a timestamp hold one state, so the snapshot is the
    /// whole of them and nothing is a rate.
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
        self.segment.visit_rows(
            plan.type_id,
            &plan.projection,
            0,
            usize::MAX,
            |ordinal, row| {
                if cancelled() {
                    connected = false;
                    return false;
                }
                match Self::row_record(plan, &row, None, None, ordinal, &dictionary) {
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

    /// The stored moment at or before `at`, and the one before that.
    fn moments(
        &self,
        plan: &Plan,
        timestamp: &'static str,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Moments>, ApiError> {
        let mut current: Option<i64> = None;
        let mut previous: Option<i64> = None;
        self.segment.visit_rows(
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
                if stored > self.at {
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

    /// The preceding moment's rows, keyed by identity, for the subtraction.
    fn collect(
        &self,
        plan: &Plan,
        timestamp: &'static str,
        at: Option<i64>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<BTreeMap<Vec<String>, BTreeMap<&'static str, f64>>, ApiError> {
        let mut collected = BTreeMap::new();
        let Some(at) = at else {
            return Ok(collected);
        };
        let counters = rate_columns(plan);
        if counters.is_empty() {
            return Ok(collected);
        }
        self.segment.visit_rows(
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
                    if let Some(number) = row.get(name).and_then(numeric) {
                        stored.insert(*name, number);
                    }
                }
                collected.insert(key, stored);
                true
            },
        )?;
        Ok(collected)
    }

    fn row_record(
        plan: &Plan,
        row: &Row,
        before: Option<&BTreeMap<&'static str, f64>>,
        elapsed: Option<i64>,
        ordinal: u64,
        dictionary: &kronika_reader::Dictionary,
    ) -> Result<Value, ApiError> {
        let stamped = plan.timestamp.and_then(|column| row_timestamp(row, column));
        let mut values = serde_json::Map::new();
        for field in &plan.fields {
            let Some(column) = field.column else {
                values.insert(field.name.clone(), Value::Null);
                continue;
            };
            let stored = row.get(column);
            let is_rate = plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
            if is_rate {
                values.insert(field.name.clone(), rate(stored, before, column, elapsed));
                continue;
            }
            values.insert(
                field.name.clone(),
                match stored {
                    Some(stored) => cell(stored, dictionary)?,
                    None => Value::Null,
                },
            );
        }
        Ok(json!({
            "record": "row",
            "type_id": plan.type_id.to_string(),
            "ordinal": ordinal.to_string(),
            "timestamp": stamped.map(|stored| stored.to_string()),
            "values": Value::Object(values),
        }))
    }
}

/// Absent and non-numeric sort last, so an ordered table starts with the rows
/// that have something to say.
fn sort_value(row: &Value, column: &str) -> f64 {
    row.get("values")
        .and_then(|values| values.get(column))
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY)
}

struct Moments {
    current: i64,
    previous: Option<i64>,
}

/// Per second, against the stored moment before this one. Absent without a
/// predecessor, and absent rather than negative when a counter went backwards.
fn rate(
    stored: Option<&Cell>,
    before: Option<&BTreeMap<&'static str, f64>>,
    column: &'static str,
    elapsed: Option<i64>,
) -> Value {
    let (Some(now), Some(before), Some(elapsed)) = (stored.and_then(numeric), before, elapsed)
    else {
        return Value::Null;
    };
    let Some(earlier) = before.get(column) else {
        return Value::Null;
    };
    let delta = now - earlier;
    if delta < 0.0 {
        return Value::Null;
    }
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

#[expect(
    clippy::cast_precision_loss,
    reason = "no counter reaches 2^53 between two snapshots"
)]
fn numeric(stored: &Cell) -> Option<f64> {
    match stored {
        Cell::I16(value) => Some(f64::from(*value)),
        Cell::I32(value) => Some(f64::from(*value)),
        Cell::I64(value) | Cell::Ts(value) => Some(*value as f64),
        Cell::U32(value) => Some(f64::from(*value)),
        Cell::U64(value) => Some(*value as f64),
        Cell::F64(value) => Some(*value),
        _other => None,
    }
}
