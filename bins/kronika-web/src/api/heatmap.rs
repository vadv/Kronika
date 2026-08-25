//! Ranked per-interval heatmaps.
//!
//! The ranking pass uses one accumulator per entity. Later passes allocate
//! cells only for bounded batches of ranked rows or groups. The others band is
//! the totals band minus the ranked rows.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use kronika_reader::{Cell, Dictionary, Reader, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, ColumnType, logical_section_name, registry};
use serde_json::{Value, json};

use super::query::{Plan, plans, resolved_dictionary};
use super::render::{cell, record};
use super::{ApiError, CachePolicy, ResponseMeta};
use crate::route::{DataRequest, HeatmapRequest, SegmentRequest};

#[cfg(test)]
mod tests;

const ROW_CHUNK_ROWS: usize = 64;
const UNGROUPED_CELL_BUDGET_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct PreparedHeatmap {
    reader: Reader,
    segments: Vec<SegmentRef>,
    request: HeatmapRequest,
    cumulative: bool,
    etag: Option<String>,
}

pub(crate) fn prepare(root: &Path, request: HeatmapRequest) -> Result<PreparedHeatmap, ApiError> {
    let cumulative = fields_class(&request.section, &request.fields, request.type_id)?
        == ColumnClass::Cumulative;
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
    let etag = stored
        .warnings
        .is_empty()
        .then(|| super::weak_etag("heatmap", &format!("{request:?}"), &segments))
        .flatten();
    Ok(PreparedHeatmap {
        reader,
        segments,
        request,
        cumulative,
        etag,
    })
}

/// Validates that every cut field exists and has one shared numeric class.
fn fields_class(
    section: &str,
    fields: &[String],
    wanted: Option<u32>,
) -> Result<ColumnClass, ApiError> {
    let mut section_seen = false;
    let mut found: Option<ColumnClass> = None;
    let mut matched = vec![false; fields.len()];
    for contract in registry() {
        let type_id = contract.type_id.get();
        if logical_section_name(type_id) != Some(section)
            || wanted.is_some_and(|wanted| wanted != type_id)
        {
            continue;
        }
        section_seen = true;
        for (index, field) in fields.iter().enumerate() {
            let Some(column) = contract.column(field) else {
                continue;
            };
            matched[index] = true;
            if column.class != ColumnClass::Cumulative && column.class != ColumnClass::Gauge {
                return Err(ApiError::NoSuchColumn(field.clone()));
            }
            if *found.get_or_insert(column.class) != column.class {
                return Err(ApiError::NoSuchColumn(field.clone()));
            }
        }
    }
    if !section_seen {
        return Err(ApiError::NoSuchSection);
    }
    if let Some(index) = matched.iter().position(|seen| !seen) {
        return Err(ApiError::NoSuchColumn(fields[index].clone()));
    }
    found.ok_or_else(|| ApiError::NoSuchColumn(fields.join("+")))
}

impl PreparedHeatmap {
    pub(super) fn meta(&self) -> ResponseMeta {
        let settled = self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished);
        ResponseMeta::ok_with_etag(
            if self.etag.is_some() {
                CachePolicy::Immutable
            } else if settled {
                CachePolicy::Revalidate
            } else {
                CachePolicy::NoStore
            },
            self.etag.clone(),
        )
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let started = std::time::Instant::now();
        let Some((fold, seen_rows)) = self.rank(cancelled)? else {
            return Ok(());
        };
        let (entities, out_of_order);
        if self.request.group.is_empty() {
            let ranked = fold.finish(self.request.top);
            entities = ranked.entity_count;
            out_of_order = ranked.out_of_order;
            self.emit_ungrouped(&ranked, &seen_rows, emit, cancelled)?;
        } else {
            let mut grouped = fold.finish_grouped(self.request.top);
            entities = grouped.group_count;
            out_of_order = grouped.out_of_order;
            let Some(filled) = self.fill_grouped(&grouped, &seen_rows, cancelled)? else {
                return Ok(());
            };
            for (row, cells) in grouped.rows.iter_mut().zip(filled.rows) {
                row.cells = cells.into_iter().map(|sum| sum.value()).collect();
            }
            grouped.others = filled.others;
            if !self.cumulative {
                grouped.others_total = band_peak(&grouped.others);
            }
            self.emit_grouped(&grouped, emit, cancelled)?;
        }
        eprintln!(
            "kronika-web: heatmap section={} field={} segments={} entities={} out_of_order={} elapsed_us={}",
            self.request.section,
            self.request.fields.join("+"),
            self.segments.len(),
            entities,
            out_of_order,
            started.elapsed().as_micros(),
        );
        Ok(())
    }

    /// Ranks entities over the full requested window, without the per-column
    /// cell grid `stream` renders for the HTTP grid view. Shared computation
    /// for `kronika_overview`.
    ///
    /// For a gauge cut, a band total is not the sum of each entity's own
    /// window maximum: `stream` corrects `totals_total` with `band_peak`
    /// over the per-column sums already folded during ranking, and corrects
    /// `others_total` from the winners-only cells `fill`/`fill_grouped`
    /// compute (a band cell holds each entity's *last* value in the column,
    /// not its maximum, so the "everyone else" band cannot be recovered
    /// from the per-entity maxima ranking already has). This mirrors that
    /// correction without building the rest of the per-column grid.
    pub(crate) fn rank_only(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<HeatmapRanking>, ApiError> {
        let Some((fold, seen_rows)) = self.rank(cancelled)? else {
            return Ok(None);
        };
        if self.request.group.is_empty() {
            let ranked = fold.finish(self.request.top);
            let totals_total = if self.cumulative {
                ranked.totals_total
            } else {
                band_peak(&ranked.totals)
            };
            let others_total = if self.cumulative {
                ranked.others_total
            } else {
                let Some((cells, _labels)) = self.fill(&ranked.rows, &seen_rows, cancelled)? else {
                    return Ok(None);
                };
                let mut winner_sums = vec![CellSum::default(); self.request.columns];
                for row_cells in &cells {
                    for (column, observed) in row_cells.iter().enumerate() {
                        if let Some(value) = observed.cell(self.cumulative) {
                            winner_sums[column].add(value);
                        }
                    }
                }
                ranked
                    .totals
                    .iter()
                    .zip(&winner_sums)
                    .filter_map(|(total, winner)| total.minus(winner))
                    .fold(None, |current: Option<f64>, value| {
                        Some(current.map_or(value, |stored| stored.max(value)))
                    })
            };
            Ok(Some(HeatmapRanking {
                entities: ranked
                    .rows
                    .into_iter()
                    .map(|row| RankedEntity {
                        key: row.key,
                        total: row.total,
                    })
                    .collect(),
                totals_total,
                others_total,
                entity_count: ranked.entity_count,
            }))
        } else {
            let grouped = fold.finish_grouped(self.request.top);
            let others_total = if self.cumulative {
                grouped.others_total
            } else {
                let Some(filled) = self.fill_grouped(&grouped, &seen_rows, cancelled)? else {
                    return Ok(None);
                };
                band_peak(&filled.others)
            };
            Ok(Some(HeatmapRanking {
                entities: grouped
                    .rows
                    .into_iter()
                    .map(|row| RankedEntity {
                        key: row.key,
                        total: row.total,
                    })
                    .collect(),
                totals_total: grouped.totals_total,
                others_total,
                entity_count: grouped.group_count,
            }))
        }
    }

    /// Ranks entities and folds totals over a fixed row prefix per plan.
    #[expect(clippy::type_complexity, reason = "one internal tuple, used once")]
    fn rank(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<(Fold, HashMap<(i64, u32), u64>)>, ApiError> {
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
                let cut = cut_columns(plan, self.request.fields.len());
                if cut.is_empty() {
                    continue;
                }
                let group_columns: Vec<Option<&'static str>> = self
                    .request
                    .group
                    .iter()
                    .map(|group| {
                        plan.fields
                            .iter()
                            .find(|output| &output.name == group)
                            .and_then(|output| output.column)
                    })
                    .collect();
                seen_rows.insert((segment_ref.id(), plan.type_id), plan.rows);
                let Some(cache) = RenderCache::for_plan(&segment, plan, plan.rows, cancelled)?
                else {
                    return Ok(None);
                };
                let mut identity: Vec<Value> = Vec::with_capacity(plan.contract.identity.len());
                let connected = pump_rows(&segment, plan, plan.rows, cancelled, |chunk| {
                    for (_ordinal, row) in chunk.drain(..) {
                        let Some(Cell::Ts(ts)) = plan.timestamp.and_then(|column| row.get(column))
                        else {
                            continue;
                        };
                        let ts = *ts;
                        if ts < request.from || ts > request.to {
                            continue;
                        }
                        identity.clear();
                        for name in plan.contract.identity {
                            identity.push(match row.get(name) {
                                Some(stored) => cache.value(stored)?,
                                None => Value::Null,
                            });
                        }
                        let group = if group_columns.is_empty() {
                            None
                        } else {
                            let mut values = Vec::with_capacity(group_columns.len());
                            for column in &group_columns {
                                values.push(match column.and_then(|name| row.get(name)) {
                                    Some(stored) => cache.value(stored)?,
                                    None => Value::Null,
                                });
                            }
                            Some(values)
                        };
                        let value = summed(&row, &cut);
                        fold.observe(plan.type_id, &identity, group, ts, value);
                    }
                    Ok(true)
                })?;
                if !connected {
                    return Ok(None);
                }
            }
        }
        Ok(Some((fold, seen_rows)))
    }

    /// Loads cells and last-seen labels for ranked entities.
    #[expect(clippy::type_complexity, reason = "one internal tuple, used once")]
    #[expect(
        clippy::too_many_lines,
        reason = "the scan keeps row decoding, identity, cell, and label state together"
    )]
    fn fill(
        &self,
        rows: &[RankedRow],
        seen_rows: &HashMap<(i64, u32), u64>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<(Vec<Vec<Obs>>, Vec<Vec<(i64, Value)>>)>, ApiError> {
        let request = &self.request;
        let winners: HashMap<&str, usize> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| (row.key.as_str(), index))
            .collect();
        let winner_types: HashSet<u32> = rows.iter().map(|row| row.type_id).collect();
        let cumulative = self.cumulative;
        let mut cells = vec![vec![Obs::default(); request.columns]; rows.len()];
        let mut carries: Vec<Option<(i64, f64)>> = vec![None; rows.len()];
        let mut labels = vec![vec![(i64::MIN, Value::Null); request.labels.len()]; rows.len()];
        if rows.is_empty() {
            return Ok(Some((cells, labels)));
        }
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
                if !winner_types.contains(&plan.type_id) {
                    continue;
                }
                let cut = cut_columns(plan, self.request.fields.len());
                // Positional against the request: data_request dedups a label
                // already present among the cut fields, so plan.fields cannot
                // be sliced by offset.
                let label_columns: Vec<Option<&'static str>> = self
                    .request
                    .labels
                    .iter()
                    .map(|label| {
                        plan.fields
                            .iter()
                            .find(|output| &output.name == label)
                            .and_then(|output| output.column)
                    })
                    .collect();
                let (from, to, columns) = (request.from, request.to, request.columns);
                let Some(cache) = RenderCache::for_plan(&segment, plan, rows, cancelled)? else {
                    return Ok(None);
                };
                let mut identity: Vec<Value> = Vec::with_capacity(plan.contract.identity.len());
                let mut key = String::new();
                let connected = pump_rows(&segment, plan, rows, cancelled, |chunk| {
                    for (_ordinal, row) in chunk.drain(..) {
                        let Some(Cell::Ts(ts)) = plan.timestamp.and_then(|column| row.get(column))
                        else {
                            continue;
                        };
                        let ts = *ts;
                        if ts < from || ts > to {
                            continue;
                        }
                        identity.clear();
                        for name in plan.contract.identity {
                            identity.push(match row.get(name) {
                                Some(stored) => cache.value(stored)?,
                                None => Value::Null,
                            });
                        }
                        entity_key_into(&mut key, plan.type_id, &identity);
                        let Some(index) = winners.get(key.as_str()).copied() else {
                            continue;
                        };
                        if let Some(value) = summed(&row, &cut) {
                            let previous_ts = carries[index].map(|(carry_ts, _value)| carry_ts);
                            let column = column_of_span(previous_ts, ts, from, to, columns);
                            if cumulative
                                && cells[index][column].count == 0
                                && let Some((carry_ts, carry_value)) = carries[index]
                                && carry_ts < ts
                            {
                                cells[index][column].observe(carry_ts, carry_value);
                            }
                            cells[index][column].observe(ts, value);
                            if carries[index].is_none_or(|(carry_ts, _value)| carry_ts <= ts) {
                                carries[index] = Some((ts, value));
                            }
                        }
                        for (slot, column) in labels[index].iter_mut().zip(&label_columns) {
                            if ts < slot.0 {
                                continue;
                            }
                            let Some(stored) = column.and_then(|name| row.get(name)) else {
                                continue;
                            };
                            let rendered = cache.value(stored)?;
                            if !rendered.is_null() {
                                *slot = (ts, rendered);
                            }
                        }
                    }
                    Ok(true)
                })?;
                if !connected {
                    return Ok(None);
                }
            }
        }
        Ok(Some((cells, labels)))
    }

    /// Loads ranked group cells and the Others band without retaining all
    /// groups by all columns. Each identity keeps its first observed group.
    fn fill_grouped(
        &self,
        grouped: &Grouped,
        seen_rows: &HashMap<(i64, u32), u64>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<GroupedCells>, ApiError> {
        if grouped.rows.is_empty() {
            return Ok(Some(GroupedCells {
                rows: Vec::new(),
                others: grouped.totals.clone(),
            }));
        }
        let request = &self.request;
        let mut fold = GroupedFill::new(
            request.from,
            request.to,
            request.columns,
            self.cumulative,
            &grouped.rows,
        );
        for segment_ref in &self.segments {
            if cancelled() {
                return Ok(None);
            }
            let segment = self.reader.open_segment(segment_ref)?;
            let grouped_plans = match plans(&segment, &self.data_request(segment_ref, false), true)
            {
                Ok(plans) => plans,
                Err(ApiError::NoSuchSection | ApiError::NoSuchColumn(_)) => continue,
                Err(error) => return Err(error),
            };
            for plan in &grouped_plans {
                let Some(rows) = seen_rows.get(&(segment_ref.id(), plan.type_id)).copied() else {
                    continue;
                };
                let cut = cut_columns(plan, request.fields.len());
                if cut.is_empty() {
                    continue;
                }
                let group_columns: Vec<Option<&'static str>> = request
                    .group
                    .iter()
                    .map(|group| {
                        plan.fields
                            .iter()
                            .find(|output| &output.name == group)
                            .and_then(|output| output.column)
                    })
                    .collect();
                let Some(cache) = RenderCache::for_plan(&segment, plan, rows, cancelled)? else {
                    return Ok(None);
                };
                let mut identity = Vec::with_capacity(plan.contract.identity.len());
                let mut group = Vec::with_capacity(group_columns.len());
                let connected = pump_rows(&segment, plan, rows, cancelled, |chunk| {
                    for (_ordinal, row) in chunk.drain(..) {
                        let Some(Cell::Ts(ts)) = plan.timestamp.and_then(|column| row.get(column))
                        else {
                            continue;
                        };
                        identity.clear();
                        for name in plan.contract.identity {
                            identity.push(match row.get(name) {
                                Some(stored) => cache.value(stored)?,
                                None => Value::Null,
                            });
                        }
                        group.clear();
                        for column in &group_columns {
                            group.push(match column.and_then(|name| row.get(name)) {
                                Some(stored) => cache.value(stored)?,
                                None => Value::Null,
                            });
                        }
                        fold.observe(plan.type_id, &identity, &group, *ts, summed(&row, &cut));
                    }
                    Ok(true)
                })?;
                if !connected {
                    return Ok(None);
                }
            }
        }
        Ok(Some(fold.finish()))
    }

    fn emit_ungrouped(
        &self,
        ranked: &Ranked,
        seen_rows: &HashMap<(i64, u32), u64>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let request = &self.request;
        let cumulative = self.cumulative;
        let header = json!({
            "record": "heatmap",
            "from": request.from.to_string(),
            "to": request.to.to_string(),
            "section": request.section,
            "fields": request.fields,
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
        let mut winner_sums = vec![CellSum::default(); request.columns];
        let batch_rows = ungrouped_batch_rows(request.columns, request.labels.len());
        for rows in ranked.rows.chunks(batch_rows) {
            let Some((cells, labels)) = self.fill(rows, seen_rows, cancelled)? else {
                return Ok(());
            };
            for ((row, row_cells), row_labels) in rows.iter().zip(cells).zip(labels) {
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
                let rendered = json!({
                    "record": "heatmap_row",
                    "type_id": row.type_id.to_string(),
                    "identity": row.identity,
                    "labels": row_labels.into_iter().map(|(_ts, value)| value).collect::<Vec<_>>(),
                    "total": number(row.total),
                    "cells": rendered,
                });
                if cancelled() || !emit(record(rendered)?) {
                    return Ok(());
                }
            }
        }
        let totals: Vec<Value> = ranked
            .totals
            .iter()
            .map(|sum| number(sum.value()))
            .collect();
        let other_values: Vec<Option<f64>> = ranked
            .totals
            .iter()
            .zip(&winner_sums)
            .map(|(total, winner)| total.minus(winner))
            .collect();
        // A gauge band's hour value is the peak of the summed strip drawn
        // beside it; one member's window maximum would understate the band.
        let totals_total = if cumulative {
            ranked.totals_total
        } else {
            band_peak(&ranked.totals)
        };
        let others_total = if cumulative {
            ranked.others_total
        } else {
            other_values
                .iter()
                .flatten()
                .fold(None, |current: Option<f64>, value| {
                    Some(current.map_or(*value, |stored| stored.max(*value)))
                })
        };
        if !emit(record(json!({
            "record": "heatmap_band",
            "band": "totals",
            "total": number(totals_total),
            "cells": totals,
        }))?) {
            return Ok(());
        }
        if !emit(record(json!({
            "record": "heatmap_band",
            "band": "others",
            "total": number(others_total),
            "cells": other_values.into_iter().map(number).collect::<Vec<_>>(),
        }))?) {
            return Ok(());
        }
        Ok(())
    }

    fn emit_grouped(
        &self,
        grouped: &Grouped,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let request = &self.request;
        let cumulative = self.cumulative;
        let header = json!({
            "record": "heatmap",
            "from": request.from.to_string(),
            "to": request.to.to_string(),
            "section": request.section,
            "fields": request.fields,
            "class": if cumulative { "cumulative" } else { "gauge" },
            "labels": request.labels,
            "group": request.group,
            "top": grouped.rows.len(),
            "entity_count": grouped.group_count,
            "others_count": grouped.group_count.saturating_sub(grouped.rows.len()),
            "out_of_order": grouped.out_of_order.to_string(),
            "intervals": (0..request.columns).map(|index| json!({
                "start": interval_start(request.from, request.to, request.columns, index).to_string(),
                "end": interval_end(request.from, request.to, request.columns, index).to_string(),
            })).collect::<Vec<_>>(),
        });
        if cancelled() || !emit(record(header)?) {
            return Ok(());
        }
        for row in &grouped.rows {
            if cancelled()
                || !emit(record(json!({
                    "record": "heatmap_row",
                    "type_id": "0",
                    "identity": row.values,
                    "labels": [],
                    "members": row.members,
                    "total": number(row.total),
                    "cells": row.cells.iter().map(|cell| number(*cell)).collect::<Vec<_>>(),
                }))?)
            {
                return Ok(());
            }
        }
        if !emit(record(json!({
            "record": "heatmap_band",
            "band": "totals",
            "total": number(grouped.totals_total),
            "cells": grouped.totals.iter().map(|sum| number(sum.value())).collect::<Vec<_>>(),
        }))?) {
            return Ok(());
        }
        if !emit(record(json!({
            "record": "heatmap_band",
            "band": "others",
            "total": number(grouped.others_total),
            "cells": grouped.others.iter().map(|sum| number(sum.value())).collect::<Vec<_>>(),
        }))?) {
            return Ok(());
        }
        Ok(())
    }

    fn data_request(&self, segment: &SegmentRef, with_labels: bool) -> DataRequest {
        let mut fields = self.request.fields.clone();
        for group in &self.request.group {
            if !fields.contains(group) {
                fields.push(group.clone());
            }
        }
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

/// Feeds one plan's first `rows` physical rows to `flush` in bounded chunks.
fn pump_rows(
    segment: &Segment,
    plan: &Plan,
    rows: u64,
    cancelled: &impl Fn() -> bool,
    mut flush: impl FnMut(&mut Vec<(u64, Row)>) -> Result<bool, ApiError>,
) -> Result<bool, ApiError> {
    let take = usize::try_from(rows).unwrap_or(usize::MAX);
    let mut chunk: Vec<(u64, Row)> = Vec::with_capacity(ROW_CHUNK_ROWS);
    let mut connected = true;
    let mut failure: Option<ApiError> = None;
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

/// Caps each ungrouped batch by observation and label storage. Larger results
/// use additional passes over the fixed row prefix.
fn ungrouped_batch_rows(columns: usize, labels: usize) -> usize {
    let cell_bytes = columns.saturating_mul(size_of::<Obs>());
    let label_bytes = labels.saturating_mul(size_of::<(i64, Value)>());
    let row_bytes = cell_bytes
        .saturating_add(label_bytes)
        .saturating_add(size_of::<Vec<Obs>>())
        .saturating_add(size_of::<Vec<(i64, Value)>>())
        .saturating_add(size_of::<Option<(i64, f64)>>())
        .max(1);
    (UNGROUPED_CELL_BUDGET_BYTES / row_bytes).max(1)
}

/// Resolves every distinct dictionary id in a plan projection once.
struct RenderCache {
    rendered: HashMap<u64, Value>,
    empty: Dictionary,
}

impl RenderCache {
    fn for_plan(
        segment: &Segment,
        plan: &Plan,
        rows: u64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Self>, ApiError> {
        let mut ids = HashSet::new();
        if plan.projection.iter().any(|name| {
            plan.contract
                .column(name)
                .is_some_and(|column| column.ty == ColumnType::StrId)
        }) {
            let take = usize::try_from(rows).unwrap_or(usize::MAX);
            let mut connected = true;
            segment.visit_rows(plan.type_id, &plan.projection, 0, take, |_ordinal, row| {
                if cancelled() {
                    connected = false;
                    return false;
                }
                ids.extend(row.iter().filter_map(|(_name, stored)| match stored {
                    Cell::StrId(id) => Some(*id),
                    _ => None,
                }));
                true
            })?;
            if !connected {
                return Ok(None);
            }
        }
        let dictionary = resolved_dictionary(segment, &ids)?;
        let mut rendered = HashMap::with_capacity(ids.len());
        for id in ids {
            let value = cell(&Cell::StrId(id), &dictionary)?;
            rendered.insert(id, value);
        }
        Ok(Some(Self {
            rendered,
            empty: Dictionary::default(),
        }))
    }

    fn value(&self, stored: &Cell) -> Result<Value, ApiError> {
        if let Cell::StrId(id) = stored {
            return self.rendered.get(id).cloned().ok_or_else(|| {
                ApiError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unresolved dictionary id {id}"),
                )))
            });
        }
        cell(stored, &self.empty)
    }
}

fn cut_columns(plan: &Plan, count: usize) -> Vec<&'static str> {
    plan.fields
        .iter()
        .take(count)
        .filter_map(|output| output.column)
        .collect()
}

fn summed(row: &Row, columns: &[&'static str]) -> Option<f64> {
    let mut sum: Option<f64> = None;
    for column in columns {
        if let Some(value) = row.get(column).and_then(numeric) {
            sum = Some(sum.unwrap_or(0.0) + value);
        }
    }
    sum
}

fn numeric(stored: &Cell) -> Option<f64> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "counters below 2^53 are exact; floating-point division makes rates approximate"
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

/// Uses distinct separators for identity parts and nulls.
pub(super) fn entity_key_into(key: &mut String, type_id: u32, identity: &[Value]) {
    use std::fmt::Write as _;
    key.clear();
    let _ = write!(key, "{type_id}");
    for value in identity {
        key.push('\u{1f}');
        match value {
            Value::String(text) => key.push_str(text),
            Value::Null => key.push('\u{0}'),
            other => {
                let _ = write!(key, "{other}");
            }
        }
    }
}

pub(super) fn interval_start(from: i64, to: i64, columns: usize, index: usize) -> i64 {
    let span = i128::from(to) - i128::from(from) + 1;
    let offset = span * to_i128(index) / to_i128(columns.max(1));
    from.saturating_add(clamped(offset))
}

pub(super) fn interval_end(from: i64, to: i64, columns: usize, index: usize) -> i64 {
    interval_start(from, to, columns, index + 1).saturating_sub(1)
}

/// The column a sample is drawn in: the one holding the middle of the span it
/// measures, which runs from the previous sample of the same entity to this
/// one. A section collected once per column width would otherwise leave a
/// column empty whenever collection drifted across its boundary, and every
/// reading would be drawn a column later than the work it describes.
pub(super) fn column_of_span(
    previous_ts: Option<i64>,
    ts: i64,
    from: i64,
    to: i64,
    columns: usize,
) -> usize {
    let middle = match previous_ts {
        Some(previous) if previous < ts => previous + (ts - previous) / 2,
        _ => ts,
    };
    column_of(middle.max(from), from, to, columns)
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

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Obs {
    pub(super) count: u32,
    first_ts: i64,
    first_value: f64,
    pub(super) last_ts: i64,
    pub(super) last_value: f64,
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

    /// Counter cells require two samples and no reset; gauge cells use the
    /// last sample.
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
    carry: Option<(i64, f64)>,
    group: Option<usize>,
}

#[derive(Clone, Copy)]
enum GroupDestination {
    Winner(usize),
    Others,
}

struct GroupFillState {
    destination: GroupDestination,
    column: usize,
    current: Obs,
    carry: Option<(i64, f64)>,
}

impl GroupFillState {
    /// The last sample seen for this entity. Under a cumulative cut the open
    /// column starts seeded with it, so `current` answers whenever it is open.
    fn previous_ts(&self) -> Option<i64> {
        if self.current.count > 0 {
            return Some(self.current.last_ts);
        }
        self.carry.map(|(carry_ts, _value)| carry_ts)
    }

    fn observe(
        &mut self,
        column: usize,
        ts: i64,
        value: f64,
        cumulative: bool,
    ) -> Option<(GroupDestination, usize, f64)> {
        if column < self.column {
            return None;
        }
        let finished = if column > self.column {
            let finished = self
                .current
                .cell(cumulative)
                .map(|value| (self.destination, self.column, value));
            if self.current.count > 0 {
                self.carry = Some((self.current.last_ts, self.current.last_value));
            }
            self.column = column;
            self.current = Obs::default();
            if cumulative && let Some((carry_ts, carry_value)) = self.carry {
                self.current.observe(carry_ts, carry_value);
            }
            finished
        } else {
            None
        };
        self.current.observe(ts, value);
        finished
    }
}

struct GroupedFill {
    from: i64,
    to: i64,
    columns: usize,
    cumulative: bool,
    winners: HashMap<String, usize>,
    entities: HashMap<String, GroupFillState>,
    cells: Vec<Vec<CellSum>>,
    others: Vec<CellSum>,
    entity_key: String,
    group_key: String,
}

struct GroupedCells {
    rows: Vec<Vec<CellSum>>,
    others: Vec<CellSum>,
}

impl GroupedFill {
    fn new(from: i64, to: i64, columns: usize, cumulative: bool, rows: &[GroupedRow]) -> Self {
        Self {
            from,
            to,
            columns,
            cumulative,
            winners: rows
                .iter()
                .enumerate()
                .map(|(index, row)| (row.key.clone(), index))
                .collect(),
            entities: HashMap::new(),
            cells: vec![vec![CellSum::default(); columns]; rows.len()],
            others: vec![CellSum::default(); columns],
            entity_key: String::new(),
            group_key: String::new(),
        }
    }

    fn observe(
        &mut self,
        type_id: u32,
        identity: &[Value],
        group: &[Value],
        ts: i64,
        value: Option<f64>,
    ) {
        if ts < self.from || ts > self.to {
            return;
        }
        let Some(value) = value else {
            return;
        };
        entity_key_into(&mut self.entity_key, type_id, identity);
        let previous_ts = self
            .entities
            .get(self.entity_key.as_str())
            .and_then(GroupFillState::previous_ts);
        let column = column_of_span(previous_ts, ts, self.from, self.to, self.columns);
        let state = if let Some(state) = self.entities.get_mut(self.entity_key.as_str()) {
            state
        } else {
            entity_key_into(&mut self.group_key, 0, group);
            let destination = self
                .winners
                .get(self.group_key.as_str())
                .copied()
                .map_or(GroupDestination::Others, GroupDestination::Winner);
            self.entities
                .entry(self.entity_key.clone())
                .or_insert_with(|| GroupFillState {
                    destination,
                    column,
                    current: Obs::default(),
                    carry: None,
                })
        };
        if let Some((destination, finished_column, finished)) =
            state.observe(column, ts, value, self.cumulative)
        {
            Self::add(
                &mut self.cells,
                &mut self.others,
                destination,
                finished_column,
                finished,
            );
        }
    }

    fn finish(mut self) -> GroupedCells {
        for (_key, state) in self.entities {
            if let Some(finished) = state.current.cell(self.cumulative) {
                Self::add(
                    &mut self.cells,
                    &mut self.others,
                    state.destination,
                    state.column,
                    finished,
                );
            }
        }
        GroupedCells {
            rows: self.cells,
            others: self.others,
        }
    }

    fn add(
        cells: &mut [Vec<CellSum>],
        others: &mut [CellSum],
        destination: GroupDestination,
        column: usize,
        value: f64,
    ) {
        match destination {
            GroupDestination::Winner(index) => cells[index][column].add(value),
            GroupDestination::Others => others[column].add(value),
        }
    }
}

struct GroupState {
    values: Vec<Value>,
    members: u32,
}

pub(super) struct GroupedRow {
    key: String,
    pub(super) values: Vec<Value>,
    pub(super) members: u32,
    pub(super) total: Option<f64>,
    pub(super) cells: Vec<Option<f64>>,
}

pub(super) struct Grouped {
    pub(super) rows: Vec<GroupedRow>,
    pub(super) totals: Vec<CellSum>,
    pub(super) others: Vec<CellSum>,
    pub(super) totals_total: Option<f64>,
    pub(super) others_total: Option<f64>,
    pub(super) group_count: usize,
    pub(super) out_of_order: u64,
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

/// One ranked entity's total over the requested window — the shape
/// `kronika_overview` needs, independent of the HTTP heatmap's NDJSON wire
/// format.
pub(crate) struct RankedEntity {
    pub(crate) key: String,
    pub(crate) total: Option<f64>,
}

/// Whole-window ranking result, shared by the HTTP heatmap stream and the
/// `kronika_overview` MCP tool.
pub(crate) struct HeatmapRanking {
    pub(crate) entities: Vec<RankedEntity>,
    pub(crate) totals_total: Option<f64>,
    pub(crate) others_total: Option<f64>,
    pub(crate) entity_count: usize,
}

/// One ranking accumulator per entity. Finished columns fold into totals when
/// the next column starts; late samples are counted but not folded again.
pub(super) struct Fold {
    from: i64,
    to: i64,
    columns: usize,
    cumulative: bool,
    entities: HashMap<String, EntityState>,
    totals: Vec<CellSum>,
    groups: Vec<GroupState>,
    group_index: HashMap<String, usize>,
    key: String,
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
            groups: Vec::new(),
            group_index: HashMap::new(),
            key: String::new(),
            out_of_order: 0,
        }
    }

    pub(super) fn observe(
        &mut self,
        type_id: u32,
        identity: &[Value],
        group: Option<Vec<Value>>,
        ts: i64,
        value: Option<f64>,
    ) {
        if ts < self.from || ts > self.to {
            return;
        }
        let Some(value) = value else {
            return;
        };
        let mut key = std::mem::take(&mut self.key);
        entity_key_into(&mut key, type_id, identity);
        let previous_ts = self
            .entities
            .get(key.as_str())
            .and_then(|state| (state.window.count > 0).then_some(state.window.last_ts));
        let column = column_of_span(previous_ts, ts, self.from, self.to, self.columns);
        let state = if let Some(state) = self.entities.get_mut(key.as_str()) {
            self.key = key;
            state
        } else {
            // The first sighting fixes the entity's group: a process that
            // execs into a new command stays under the name it started with.
            let group = group.map(|values| {
                let mut group_key = String::new();
                entity_key_into(&mut group_key, 0, &values);
                *self.group_index.entry(group_key).or_insert_with(|| {
                    self.groups.push(GroupState { values, members: 0 });
                    self.groups.len() - 1
                })
            });
            if let Some(index) = group {
                self.groups[index].members = self.groups[index].members.saturating_add(1);
            }
            let owned = key.clone();
            self.key = key;
            self.entities.entry(owned).or_insert_with(|| EntityState {
                type_id,
                identity: identity.to_vec(),
                window: Obs::default(),
                column,
                current: Obs::default(),
                carry: None,
                group,
            })
        };
        state.window.observe(ts, value);
        if column < state.column {
            self.out_of_order = self.out_of_order.saturating_add(1);
            return;
        }
        if column > state.column {
            if let Some(finished) = state.current.cell(self.cumulative) {
                self.totals[state.column].add(finished);
            }
            if state.current.count > 0 {
                state.carry = Some((state.current.last_ts, state.current.last_value));
            }
            state.column = column;
            state.current = Obs::default();
            // A counter cell measures from the latest observation at or
            // before the interval start, so one in-interval sample is enough.
            if self.cumulative
                && let Some((carry_ts, carry_value)) = state.carry
            {
                state.current.observe(carry_ts, carry_value);
            }
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

    /// Sums completed per-identity cells and ranking totals by group value.
    pub(super) fn finish_grouped(self, top: usize) -> Grouped {
        let cumulative = self.cumulative;
        let out_of_order = self.out_of_order;
        let mut totals = self.totals;
        let mut groups = self.groups;
        let mut group_totals: Vec<Option<f64>> = vec![None; groups.len()];
        for (_key, state) in self.entities {
            if let Some(finished) = state.current.cell(cumulative) {
                totals[state.column].add(finished);
            }
            if let (Some(index), Some(total)) = (state.group, state.window.total(cumulative)) {
                group_totals[index] =
                    Some(group_totals[index].map_or(total, |current| current + total));
            }
        }
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by(
            |left, right| match (group_totals[*left], group_totals[*right]) {
                (Some(left_total), Some(right_total)) => right_total
                    .partial_cmp(&left_total)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.cmp(right)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left.cmp(right),
            },
        );
        let group_count = groups.len();
        let mut others_total: Option<f64> = None;
        for index in order.iter().skip(top) {
            if let Some(total) = group_totals[*index] {
                others_total = Some(others_total.unwrap_or(0.0) + total);
            }
        }
        let totals_total = if cumulative {
            group_totals
                .iter()
                .flatten()
                .fold(None, |current: Option<f64>, total| {
                    Some(current.unwrap_or(0.0) + total)
                })
        } else {
            band_peak(&totals)
        };
        let others_total = if cumulative { others_total } else { None };
        let mut rows = Vec::with_capacity(top.min(order.len()));
        for index in order.into_iter().take(top) {
            let group = std::mem::replace(
                &mut groups[index],
                GroupState {
                    values: Vec::new(),
                    members: 0,
                },
            );
            let mut key = String::new();
            entity_key_into(&mut key, 0, &group.values);
            rows.push(GroupedRow {
                key,
                values: group.values,
                members: group.members,
                total: group_totals[index],
                cells: Vec::new(),
            });
        }
        Grouped {
            rows,
            totals,
            others: vec![CellSum::default(); self.columns],
            totals_total,
            others_total,
            group_count,
            out_of_order,
        }
    }
}

fn band_peak(cells: &[CellSum]) -> Option<f64> {
    cells
        .iter()
        .filter_map(CellSum::value)
        .fold(None, |current: Option<f64>, value| {
            Some(current.map_or(value, |stored| stored.max(value)))
        })
}
