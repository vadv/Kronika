//! The ranked top view over one window: entities of one section ranked by one
//! column, with per-interval cells.
//!
//! Two passes over the same rows, as the design prescribes. The first pass
//! keeps one small accumulator per entity — the whole-window ranking value and
//! the running column cell that feeds the totals band — so memory stays
//! proportional to the number of entities, not entities times columns. Only
//! the second pass allocates the top-K-by-columns result and fills its cells
//! and labels. The others band is the totals band minus the ranked rows.

use std::collections::HashMap;
use std::path::Path;

use kronika_reader::{Cell, Reader, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, logical_section_name, registry};
use serde_json::{Value, json};

use super::query::{Plan, chunk_dictionary, plans};
use super::render::{cell, record};
use super::{ApiError, CachePolicy, ResponseMeta};
use crate::route::{DataRequest, HeatmapRequest, SegmentRequest};

#[cfg(test)]
mod tests;

const ROW_CHUNK_ROWS: usize = 64;

pub(crate) struct PreparedHeatmap {
    reader: Reader,
    segments: Vec<SegmentRef>,
    request: HeatmapRequest,
    cumulative: bool,
}

pub(super) fn prepare(root: &Path, request: HeatmapRequest) -> Result<PreparedHeatmap, ApiError> {
    let cumulative =
        field_class(&request.section, &request.field, request.type_id)? == ColumnClass::Cumulative;
    let started = std::time::Instant::now();
    let reader = Reader::open(root)?;
    let stored = reader.catalog_segments(..)?;
    let mut segments: Vec<SegmentRef> = stored
        .segments
        .into_iter()
        .filter(|segment| segment.max_ts() >= request.from && segment.min_ts() <= request.to)
        .collect();
    segments.sort_by_key(SegmentRef::min_ts);
    super::catalog::log_open(segments.len(), &stored.warnings, started);
    Ok(PreparedHeatmap {
        reader,
        segments,
        request,
        cumulative,
    })
}

/// The registry decides upfront whether the field is a counter or a gauge; a
/// label or timestamp column cannot rank a heatmap. The registry, not the
/// stored hour, also answers whether the section and field exist at all.
fn field_class(section: &str, field: &str, wanted: Option<u32>) -> Result<ColumnClass, ApiError> {
    let mut section_seen = false;
    let mut found: Option<ColumnClass> = None;
    for contract in registry() {
        let type_id = contract.type_id.get();
        if logical_section_name(type_id) != Some(section)
            || wanted.is_some_and(|wanted| wanted != type_id)
        {
            continue;
        }
        section_seen = true;
        let Some(column) = contract.column(field) else {
            continue;
        };
        if column.class != ColumnClass::Cumulative && column.class != ColumnClass::Gauge {
            return Err(ApiError::NoSuchColumn(field.to_owned()));
        }
        if *found.get_or_insert(column.class) != column.class {
            return Err(ApiError::NoSuchColumn(field.to_owned()));
        }
    }
    if !section_seen {
        return Err(ApiError::NoSuchSection);
    }
    found.ok_or_else(|| ApiError::NoSuchColumn(field.to_owned()))
}

impl PreparedHeatmap {
    pub(super) fn meta(&self) -> ResponseMeta {
        let settled = self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished);
        ResponseMeta::ok(if settled {
            CachePolicy::Revalidate
        } else {
            CachePolicy::NoStore
        })
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let started = std::time::Instant::now();
        let Some((ranked, seen_rows)) = self.rank(cancelled)? else {
            return Ok(());
        };
        let Some((cells, labels)) = self.fill(&ranked, &seen_rows, cancelled)? else {
            return Ok(());
        };
        self.emit_all(&ranked, &cells, &labels, emit, cancelled)?;
        eprintln!(
            "kronika-web: heatmap section={} field={} segments={} entities={} out_of_order={} elapsed_us={}",
            self.request.section,
            self.request.field,
            self.segments.len(),
            ranked.entity_count,
            ranked.out_of_order,
            started.elapsed().as_micros(),
        );
        Ok(())
    }

    /// First pass: rank every entity over the whole window and fold the
    /// totals band. Returns the rows each plan contributed so the second pass
    /// reads exactly the same data even while an active segment grows.
    #[expect(clippy::type_complexity, reason = "one internal tuple, used once")]
    fn rank(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<(Ranked, HashMap<(i64, u32), u64>)>, ApiError> {
        let request = &self.request;
        let mut fold = Fold::new(request.from, request.to, request.columns, self.cumulative);
        let mut seen_rows: HashMap<(i64, u32), u64> = HashMap::new();
        for segment_ref in &self.segments {
            if cancelled() {
                return Ok(None);
            }
            let segment = self.reader.open_segment(segment_ref)?;
            let ranked_plans = match plans(&segment, &self.data_request(segment_ref, false), true) {
                Ok(plans) => plans,
                Err(ApiError::NoSuchSection | ApiError::NoSuchColumn(_)) => continue,
                Err(error) => return Err(error),
            };
            for plan in &ranked_plans {
                if plan
                    .fields
                    .first()
                    .is_none_or(|field| field.column.is_none())
                {
                    continue;
                }
                seen_rows.insert((segment_ref.id(), plan.type_id), plan.rows);
                let connected = visit_plan(
                    &segment,
                    plan,
                    plan.rows,
                    cancelled,
                    |ts, identity, value, _labels| {
                        fold.observe(plan.type_id, identity, ts, value);
                    },
                )?;
                if !connected {
                    return Ok(None);
                }
            }
        }
        Ok(Some((fold.finish(request.top), seen_rows)))
    }

    /// Second pass: the top-K-by-columns cells and last-seen labels.
    #[expect(clippy::type_complexity, reason = "one internal tuple, used once")]
    fn fill(
        &self,
        ranked: &Ranked,
        seen_rows: &HashMap<(i64, u32), u64>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<(Vec<Vec<Obs>>, Vec<Vec<(i64, Value)>>)>, ApiError> {
        let request = &self.request;
        let winners: HashMap<String, usize> = ranked
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| (row.key.clone(), index))
            .collect();
        let mut cells = vec![vec![Obs::default(); request.columns]; ranked.rows.len()];
        let mut labels =
            vec![vec![(i64::MIN, Value::Null); request.labels.len()]; ranked.rows.len()];
        for segment_ref in &self.segments {
            if cancelled() {
                return Ok(None);
            }
            let segment = self.reader.open_segment(segment_ref)?;
            let label_plans = match plans(&segment, &self.data_request(segment_ref, true), true) {
                Ok(plans) => plans,
                Err(ApiError::NoSuchSection | ApiError::NoSuchColumn(_)) => continue,
                Err(error) => return Err(error),
            };
            for plan in &label_plans {
                let Some(rows) = seen_rows.get(&(segment_ref.id(), plan.type_id)).copied() else {
                    continue;
                };
                let type_id = plan.type_id;
                let (from, to, columns) = (request.from, request.to, request.columns);
                let connected = visit_plan(
                    &segment,
                    plan,
                    rows,
                    cancelled,
                    |ts, identity, value, row_labels| {
                        let Some(index) = winners.get(&entity_key(type_id, identity)).copied()
                        else {
                            return;
                        };
                        if ts < from || ts > to {
                            return;
                        }
                        if let Some(value) = value {
                            cells[index][column_of(ts, from, to, columns)].observe(ts, value);
                        }
                        for (slot, stored) in labels[index].iter_mut().zip(row_labels) {
                            if !stored.is_null() && ts >= slot.0 {
                                *slot = (ts, stored.clone());
                            }
                        }
                    },
                )?;
                if !connected {
                    return Ok(None);
                }
            }
        }
        Ok(Some((cells, labels)))
    }

    fn emit_all(
        &self,
        ranked: &Ranked,
        cells: &[Vec<Obs>],
        labels: &[Vec<(i64, Value)>],
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let request = &self.request;
        let cumulative = self.cumulative;
        let mut winner_sums = vec![CellSum::default(); request.columns];
        let rendered_rows: Vec<Value> = ranked
            .rows
            .iter()
            .zip(cells)
            .zip(labels)
            .map(|((row, row_cells), row_labels)| {
                let rendered: Vec<Value> = row_cells
                    .iter()
                    .enumerate()
                    .map(|(index, observed)| {
                        let stored = observed.cell(cumulative);
                        if let Some(stored) = stored {
                            winner_sums[index].add(stored);
                        }
                        number(stored)
                    })
                    .collect();
                json!({
                    "record": "heatmap_row",
                    "type_id": row.type_id.to_string(),
                    "identity": row.identity,
                    "labels": row_labels.iter().map(|(_ts, value)| value.clone()).collect::<Vec<_>>(),
                    "total": number(row.total),
                    "cells": rendered,
                })
            })
            .collect();
        let header = json!({
            "record": "heatmap",
            "from": request.from.to_string(),
            "to": request.to.to_string(),
            "section": request.section,
            "field": request.field,
            "class": if cumulative { "cumulative" } else { "gauge" },
            "labels": request.labels,
            "top": ranked.rows.len(),
            "entity_count": ranked.entity_count,
            "others_count": ranked.entity_count.saturating_sub(ranked.rows.len()),
            "out_of_order": ranked.out_of_order.to_string(),
            "intervals": (0..request.columns).map(|index| json!({
                "start": interval_start(request.from, request.to, request.columns, index).to_string(),
                "end": interval_end(request.from, request.to, request.columns, index).to_string(),
            })).collect::<Vec<_>>(),
        });
        if cancelled() || !emit(record(header)?) {
            return Ok(());
        }
        for rendered in rendered_rows {
            if cancelled() || !emit(record(rendered)?) {
                return Ok(());
            }
        }
        let totals: Vec<Value> = ranked
            .totals
            .iter()
            .map(|sum| number(sum.value()))
            .collect();
        let others: Vec<Value> = ranked
            .totals
            .iter()
            .zip(&winner_sums)
            .map(|(total, winner)| number(total.minus(winner)))
            .collect();
        if !emit(record(json!({
            "record": "heatmap_band",
            "band": "totals",
            "total": number(ranked.totals_total),
            "cells": totals,
        }))?) {
            return Ok(());
        }
        if !emit(record(json!({
            "record": "heatmap_band",
            "band": "others",
            "total": number(ranked.others_total),
            "cells": others,
        }))?) {
            return Ok(());
        }
        Ok(())
    }

    fn data_request(&self, segment: &SegmentRef, with_labels: bool) -> DataRequest {
        let mut fields = vec![self.request.field.clone()];
        if with_labels {
            for label in &self.request.labels {
                if !fields.contains(label) {
                    fields.push(label.clone());
                }
            }
        }
        DataRequest {
            segment: SegmentRequest {
                segment_id: segment.id(),
                section: self.request.section.clone(),
            },
            fields,
            filters: Vec::new(),
            type_id: self.request.type_id,
            after: None,
        }
    }
}

/// Walk one plan's first `rows` physical rows in chunks and hand each row's
/// timestamp, decoded identity, numeric value and decoded non-field outputs to
/// the callback.
fn visit_plan(
    segment: &Segment,
    plan: &Plan,
    rows: u64,
    cancelled: &impl Fn() -> bool,
    mut observe: impl FnMut(i64, &[Value], Option<f64>, &[Value]),
) -> Result<bool, ApiError> {
    let take = usize::try_from(rows).unwrap_or(usize::MAX);
    let mut chunk: Vec<(u64, Row)> = Vec::with_capacity(ROW_CHUNK_ROWS);
    let mut connected = true;
    let mut failure: Option<ApiError> = None;
    let timestamp = plan.timestamp;
    let field = plan.fields.first().and_then(|output| output.column);
    let label_columns: Vec<Option<&'static str>> = plan
        .fields
        .iter()
        .skip(1)
        .map(|output| output.column)
        .collect();
    let mut flush = |chunk: &mut Vec<(u64, Row)>| -> Result<bool, ApiError> {
        if cancelled() {
            return Ok(false);
        }
        let dictionary = chunk_dictionary(segment, chunk)?;
        for (_ordinal, row) in chunk.drain(..) {
            let Some(Cell::Ts(ts)) = timestamp.and_then(|column| row.get(column)) else {
                continue;
            };
            let ts = *ts;
            let identity = plan
                .contract
                .identity
                .iter()
                .map(|name| {
                    row.get(name)
                        .map_or(Ok(Value::Null), |stored| cell(stored, &dictionary))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let value = field.and_then(|column| row.get(column)).and_then(numeric);
            let labels = label_columns
                .iter()
                .map(|column| {
                    column
                        .and_then(|name| row.get(name))
                        .map_or(Ok(Value::Null), |stored| cell(stored, &dictionary))
                })
                .collect::<Result<Vec<_>, _>>()?;
            observe(ts, &identity, value, &labels);
        }
        Ok(true)
    };
    segment.visit_rows(plan.type_id, &plan.projection, 0, take, |ordinal, row| {
        if cancelled() {
            connected = false;
            return false;
        }
        chunk.push((ordinal, row));
        if chunk.len() < ROW_CHUNK_ROWS {
            return true;
        }
        match flush(&mut chunk) {
            Ok(still_connected) => connected = still_connected,
            Err(error) => failure = Some(error),
        }
        connected && failure.is_none()
    })?;
    if failure.is_none() && connected && !chunk.is_empty() {
        match flush(&mut chunk) {
            Ok(still_connected) => connected = still_connected,
            Err(error) => failure = Some(error),
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(connected)
}

fn numeric(stored: &Cell) -> Option<f64> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "counters below 2^53 are exact and rates are approximate by nature"
    )]
    match stored {
        Cell::I16(value) => Some(f64::from(*value)),
        Cell::I32(value) => Some(f64::from(*value)),
        Cell::I64(value) | Cell::Ts(value) => Some(*value as f64),
        Cell::U32(value) => Some(f64::from(*value)),
        Cell::U64(value) => Some(*value as f64),
        Cell::F64(value) => value.is_finite().then_some(*value),
        Cell::Bool(_) | Cell::StrId(_) | Cell::ListI32(_) | Cell::Null => None,
    }
}

fn number(stored: Option<f64>) -> Value {
    stored
        .and_then(serde_json::Number::from_f64)
        .map_or(Value::Null, Value::Number)
}

pub(super) fn entity_key(type_id: u32, identity: &[Value]) -> String {
    let mut key = type_id.to_string();
    for value in identity {
        key.push('\u{1f}');
        key.push_str(&value.to_string());
    }
    key
}

pub(super) fn interval_start(from: i64, to: i64, columns: usize, index: usize) -> i64 {
    let span = i128::from(to) - i128::from(from) + 1;
    let offset = span * to_i128(index) / to_i128(columns.max(1));
    from.saturating_add(clamped(offset))
}

pub(super) fn interval_end(from: i64, to: i64, columns: usize, index: usize) -> i64 {
    interval_start(from, to, columns, index + 1).saturating_sub(1)
}

pub(super) fn column_of(ts: i64, from: i64, to: i64, columns: usize) -> usize {
    let span = (i128::from(to) - i128::from(from) + 1).max(1);
    let offset = i128::from(ts) - i128::from(from);
    let column = (offset * to_i128(columns.max(1)) / span).max(0);
    usize::try_from(column)
        .unwrap_or(columns - 1)
        .min(columns.saturating_sub(1))
}

fn to_i128(value: usize) -> i128 {
    i128::try_from(value).unwrap_or(i128::MAX)
}

fn clamped(offset: i128) -> i64 {
    i64::try_from(offset).unwrap_or(i64::MAX)
}

/// First/last observation of one entity inside one span of time.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Obs {
    count: u32,
    first_ts: i64,
    first_value: f64,
    last_ts: i64,
    last_value: f64,
    max_value: f64,
}

impl Obs {
    pub(super) fn observe(&mut self, ts: i64, value: f64) {
        if self.count == 0 {
            *self = Self {
                count: 1,
                first_ts: ts,
                first_value: value,
                last_ts: ts,
                last_value: value,
                max_value: value,
            };
            return;
        }
        self.count = self.count.saturating_add(1);
        if ts < self.first_ts {
            self.first_ts = ts;
            self.first_value = value;
        }
        if ts >= self.last_ts {
            self.last_ts = ts;
            self.last_value = value;
        }
        if value > self.max_value {
            self.max_value = value;
        }
    }

    /// The design's cell rule: a counter cell is last minus first over the
    /// observed elapsed time, null on a reset or fewer than two observations;
    /// a gauge cell is the last sample.
    pub(super) fn cell(&self, cumulative: bool) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        if !cumulative {
            return Some(self.last_value);
        }
        if self.count < 2 || self.last_ts <= self.first_ts {
            return None;
        }
        let delta = self.last_value - self.first_value;
        if delta < 0.0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "an interval of 2^52 microseconds is 142 years"
        )]
        let seconds = (self.last_ts - self.first_ts) as f64 / 1_000_000.0;
        Some(delta / seconds)
    }

    /// The whole-window ranking value: absolute counter delta or gauge maximum.
    pub(super) fn total(&self, cumulative: bool) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        if !cumulative {
            return Some(self.max_value);
        }
        if self.count < 2 || self.last_ts <= self.first_ts {
            return None;
        }
        let delta = self.last_value - self.first_value;
        (delta >= 0.0).then_some(delta)
    }
}

/// A null-aware sum of finished cells.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CellSum {
    sum: f64,
    contributors: u32,
}

impl CellSum {
    pub(super) fn add(&mut self, value: f64) {
        self.sum += value;
        self.contributors = self.contributors.saturating_add(1);
    }

    pub(super) fn value(&self) -> Option<f64> {
        (self.contributors > 0).then_some(self.sum)
    }

    pub(super) fn minus(&self, winners: &Self) -> Option<f64> {
        let contributors = self.contributors.saturating_sub(winners.contributors);
        (contributors > 0).then_some(self.sum - winners.sum)
    }
}

struct EntityState {
    type_id: u32,
    identity: Vec<Value>,
    window: Obs,
    column: usize,
    current: Obs,
}

pub(super) struct RankedRow {
    pub(super) key: String,
    pub(super) type_id: u32,
    pub(super) identity: Vec<Value>,
    pub(super) total: Option<f64>,
}

pub(super) struct Ranked {
    pub(super) rows: Vec<RankedRow>,
    pub(super) totals: Vec<CellSum>,
    pub(super) totals_total: Option<f64>,
    pub(super) others_total: Option<f64>,
    pub(super) entity_count: usize,
    pub(super) out_of_order: u64,
}

/// The first pass: one accumulator per entity. Rows arrive in recording
/// order, so an entity's finished column folds into the totals band the
/// moment a later column starts; a sample for an already-finished column is
/// counted and skipped rather than folded wrongly.
pub(super) struct Fold {
    from: i64,
    to: i64,
    columns: usize,
    cumulative: bool,
    entities: HashMap<String, EntityState>,
    totals: Vec<CellSum>,
    out_of_order: u64,
}

impl Fold {
    pub(super) fn new(from: i64, to: i64, columns: usize, cumulative: bool) -> Self {
        Self {
            from,
            to,
            columns,
            cumulative,
            entities: HashMap::new(),
            totals: vec![CellSum::default(); columns],
            out_of_order: 0,
        }
    }

    pub(super) fn observe(
        &mut self,
        type_id: u32,
        identity: &[Value],
        ts: i64,
        value: Option<f64>,
    ) {
        if ts < self.from || ts > self.to {
            return;
        }
        let Some(value) = value else {
            return;
        };
        let key = entity_key(type_id, identity);
        let column = column_of(ts, self.from, self.to, self.columns);
        let state = self.entities.entry(key).or_insert_with(|| EntityState {
            type_id,
            identity: identity.to_vec(),
            window: Obs::default(),
            column,
            current: Obs::default(),
        });
        state.window.observe(ts, value);
        if column < state.column {
            self.out_of_order = self.out_of_order.saturating_add(1);
            return;
        }
        if column > state.column {
            if let Some(finished) = state.current.cell(self.cumulative) {
                self.totals[state.column].add(finished);
            }
            state.column = column;
            state.current = Obs::default();
        }
        state.current.observe(ts, value);
    }

    pub(super) fn finish(self, top: usize) -> Ranked {
        let cumulative = self.cumulative;
        let mut totals = self.totals;
        let mut rows: Vec<RankedRow> = Vec::with_capacity(self.entities.len());
        let mut totals_total: Option<f64> = None;
        for (key, state) in self.entities {
            if let Some(finished) = state.current.cell(cumulative) {
                totals[state.column].add(finished);
            }
            let total = state.window.total(cumulative);
            if let Some(total) = total {
                totals_total = Some(match totals_total {
                    Some(current) if !cumulative => current.max(total),
                    Some(current) => current + total,
                    None => total,
                });
            }
            rows.push(RankedRow {
                key,
                type_id: state.type_id,
                identity: state.identity,
                total,
            });
        }
        rows.sort_by(|left, right| match (left.total, right.total) {
            (Some(left_total), Some(right_total)) => right_total
                .partial_cmp(&left_total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.key.cmp(&right.key)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.key.cmp(&right.key),
        });
        let entity_count = rows.len();
        let others_total = rows.iter().skip(top).filter_map(|row| row.total).fold(
            None,
            |current: Option<f64>, total| {
                Some(match current {
                    Some(current) if !cumulative => current.max(total),
                    Some(current) => current + total,
                    None => total,
                })
            },
        );
        rows.truncate(top);
        Ranked {
            rows,
            totals,
            totals_total,
            others_total,
            entity_count,
            out_of_order: self.out_of_order,
        }
    }
}
