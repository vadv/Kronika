//! One-pass Heatmap planning, execution, and transport-independent folding.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use kronika_reader::{Cell, Dictionary, Reader, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, Unit, contract, logical_section_name, registry};
use serde_json::{Value, json};

use super::query::{
    HeatmapBatchQuery, HeatmapItemQuery, HeatmapRequest, HeatmapView, MAX_FIELDS, MAX_TOP,
};
use super::result::{
    CoverageState, HeatmapBand, HeatmapBatchResult, HeatmapCoverage, HeatmapEntity, HeatmapGrid,
    HeatmapGroup, HeatmapInterval, HeatmapItemResult, NamedValues,
};
use crate::api::render::{cell, record};
use crate::api::{ApiError, CachePolicy, ResponseMeta};

const WORKING_SET_MAX_BYTES: usize = 8 * 1024 * 1024;
const RESULT_MAX_BYTES: usize = 8 * 1024 * 1024;
const IDENTITY_ALIASES: [(&str, &str); 2] = [("queryid", "query_id"), ("planid", "plan_id")];

#[derive(Debug)]
pub(crate) struct HeatmapError {
    ranking_index: usize,
    message: String,
    invalid: bool,
}

impl HeatmapError {
    fn invalid(ranking_index: usize, message: impl Into<String>) -> Self {
        Self {
            ranking_index,
            message: message.into(),
            invalid: true,
        }
    }

    fn storage(ranking_index: usize, error: impl std::fmt::Display) -> Self {
        Self {
            ranking_index,
            message: error.to_string(),
            invalid: false,
        }
    }

    pub(crate) const fn ranking_index(&self) -> usize {
        self.ranking_index
    }

    pub(crate) fn into_api(self) -> ApiError {
        if self.invalid {
            ApiError::BadFilter(self.to_string())
        } else {
            ApiError::Unreadable(Box::new(self))
        }
    }
}

impl std::fmt::Display for HeatmapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rankings[{}]: {}", self.ranking_index, self.message)
    }
}

impl std::error::Error for HeatmapError {}

pub(crate) struct PreparedHeatmap {
    batch: PreparedHeatmapBatch,
}

pub(crate) struct PreparedHeatmapBatch {
    reader: Reader,
    segments: Vec<SegmentRef>,
    recorded: Option<(i64, i64)>,
    query: HeatmapBatchQuery,
    unique: Vec<ItemSpec>,
    original_to_unique: Vec<usize>,
    etag: Option<String>,
    #[cfg(test)]
    executions: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    row_visits: std::sync::atomic::AtomicU64,
}

#[derive(Clone)]
struct ItemSpec {
    query: HeatmapItemQuery,
    class: ColumnClass,
    unit: Option<Unit>,
    labels: Vec<String>,
    first_index: usize,
}

pub(crate) fn prepare(root: &Path, request: HeatmapRequest) -> Result<PreparedHeatmap, ApiError> {
    let query = request
        .normalize()
        .map_err(|error| ApiError::BadFilter(error.to_string()))?;
    prepare_batch(root, query)
        .map(|batch| PreparedHeatmap { batch })
        .map_err(HeatmapError::into_api)
}

pub(crate) fn prepare_batch(
    root: &Path,
    query: HeatmapBatchQuery,
) -> Result<PreparedHeatmapBatch, HeatmapError> {
    if query.items.is_empty() {
        return Err(HeatmapError::invalid(0, "rankings must not be empty"));
    }
    let (unique, original_to_unique) = normalize_items(&query.items)?;
    let started = std::time::Instant::now();
    let reader = Reader::open(root).map_err(|error| HeatmapError::storage(0, error))?;
    let stored = reader
        .catalog_segments(..)
        .map_err(|error| HeatmapError::storage(0, error))?;
    let recorded = stored
        .segments
        .iter()
        .map(|segment| (segment.min_ts(), segment.max_ts()))
        .reduce(|(first, last), (min, max)| (first.min(min), last.max(max)));
    let mut segments: Vec<SegmentRef> = stored
        .segments
        .into_iter()
        .filter(|segment| {
            segment.max_ts() >= query.range.from && segment.min_ts() < query.range.to_exclusive
        })
        .collect();
    segments.sort_by_key(SegmentRef::min_ts);
    super::super::catalog::log_open(segments.len(), &stored.warnings, started);
    let etag = stored
        .warnings
        .is_empty()
        .then(|| super::super::weak_etag("heatmap", &format!("{query:?}"), &segments))
        .flatten();
    Ok(PreparedHeatmapBatch {
        reader,
        segments,
        recorded,
        query,
        unique,
        original_to_unique,
        etag,
        #[cfg(test)]
        executions: std::sync::atomic::AtomicU64::new(0),
        #[cfg(test)]
        row_visits: std::sync::atomic::AtomicU64::new(0),
    })
}

impl PreparedHeatmap {
    pub(crate) fn meta(&self) -> ResponseMeta {
        self.batch.meta()
    }

    pub(crate) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let range = self.batch.query.range;
        let result = self
            .batch
            .execute(cancelled)
            .map_err(HeatmapError::into_api)?;
        let Some(item) = result.results.first() else {
            return Ok(());
        };
        emit_http(item, range.from, range.to_exclusive - 1, emit, cancelled)
    }
}

impl PreparedHeatmapBatch {
    pub(crate) fn meta(&self) -> ResponseMeta {
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

    pub(crate) fn execute(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<HeatmapBatchResult, HeatmapError> {
        #[cfg(test)]
        self.executions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut budget = WorkingBudget::default();
        let mut accumulators: Vec<Accumulator> = self
            .unique
            .iter()
            .map(|spec| Accumulator::new(spec, self.query.range))
            .collect();
        budget.reserve(
            accumulators.len().saturating_mul(size_of::<Accumulator>()),
            0,
        )?;
        let mut opened_segments = Vec::with_capacity(self.segments.len());
        budget.reserve(self.segments.len().saturating_mul(size_of::<Segment>()), 0)?;
        let mut rendered_ids: HashMap<u64, Value> = HashMap::new();

        for segment_ref in &self.segments {
            if cancelled() {
                return Err(HeatmapError::storage(0, "request cancelled"));
            }
            let segment = self
                .reader
                .open_segment(segment_ref)
                .map_err(|error| HeatmapError::storage(0, error))?;
            let plans = physical_plans(&segment, &self.unique);
            for plan in plans {
                scan_plan(
                    &segment,
                    &plan,
                    self.query.range,
                    &mut accumulators,
                    &mut budget,
                    cancelled,
                    #[cfg(test)]
                    &self.row_visits,
                )?;
            }
            opened_segments.push(segment);
        }

        let mut retained_ids = HashSet::new();
        for accumulator in &accumulators {
            accumulator.collect_ids(&mut retained_ids, &mut budget)?;
        }
        if !retained_ids.is_empty() {
            let index = self
                .unique
                .iter()
                .map(|spec| spec.first_index)
                .min()
                .unwrap_or(0);
            for segment in &opened_segments {
                let dictionary = segment
                    .dictionary_once_for(&retained_ids)
                    .map_err(|error| HeatmapError::storage(index, error))?;
                for id in &retained_ids {
                    if rendered_ids.contains_key(id) {
                        continue;
                    }
                    let Some(resolved) = dictionary.resolve(*id) else {
                        continue;
                    };
                    let value = cell(&Cell::StrId(*id), &dictionary)
                        .map_err(|error| HeatmapError::storage(index, error))?;
                    let bytes = serde_json::to_vec(&value)
                        .map_err(|error| HeatmapError::storage(index, error))?
                        .len()
                        .saturating_add(size_of_val(&resolved))
                        .saturating_add(size_of::<(u64, Value)>() * 2);
                    budget.reserve(bytes, index)?;
                    rendered_ids.insert(*id, value);
                }
            }
        }

        let mut unique_results = Vec::with_capacity(accumulators.len());
        for (spec, accumulator) in self.unique.iter().zip(accumulators) {
            unique_results.push(accumulator.finish(spec, self.recorded, &rendered_ids)?);
        }
        let results = self
            .original_to_unique
            .iter()
            .map(|index| unique_results[*index].clone())
            .collect();
        let result = HeatmapBatchResult { results };
        check_result_budget(&result)?;
        Ok(result)
    }
}

fn normalize_items(
    items: &[HeatmapItemQuery],
) -> Result<(Vec<ItemSpec>, Vec<usize>), HeatmapError> {
    let mut unique = Vec::new();
    let mut positions = HashMap::new();
    let mut original_to_unique = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(position) = positions.get(item).copied() {
            original_to_unique.push(position);
            continue;
        }
        let (class, unit, labels) = validate_item(item, index)?;
        let position = unique.len();
        positions.insert(item.clone(), position);
        original_to_unique.push(position);
        unique.push(ItemSpec {
            query: item.clone(),
            class,
            unit,
            labels,
            first_index: index,
        });
    }
    Ok((unique, original_to_unique))
}

#[expect(
    clippy::too_many_lines,
    reason = "validation keeps every indexed ranking error in request order"
)]
fn validate_item(
    item: &HeatmapItemQuery,
    index: usize,
) -> Result<(ColumnClass, Option<Unit>, Vec<String>), HeatmapError> {
    let ranking = &item.ranking;
    if ranking.section.is_empty() || ranking.section.len() > 128 {
        return Err(HeatmapError::invalid(
            index,
            "section must contain 1 to 128 UTF-8 bytes",
        ));
    }
    if !(1..=MAX_FIELDS).contains(&ranking.fields.len()) {
        return Err(HeatmapError::invalid(
            index,
            format!("fields must contain 1 to {MAX_FIELDS} names"),
        ));
    }
    let mut seen = HashSet::new();
    for field in &ranking.fields {
        if field.is_empty() || !seen.insert(field) {
            return Err(HeatmapError::invalid(
                index,
                format!("field {field:?} is empty or repeated"),
            ));
        }
    }
    if !(1..=MAX_TOP).contains(&ranking.top) {
        return Err(HeatmapError::invalid(
            index,
            format!("top must be between 1 and {MAX_TOP}, got {}", ranking.top),
        ));
    }

    let wanted_type = item.view.type_id();
    let contracts: Vec<_> = registry()
        .iter()
        .filter(|contract| {
            logical_section_name(contract.type_id.get()) == Some(ranking.section.as_str())
                && wanted_type.is_none_or(|wanted| wanted == contract.type_id.get())
        })
        .collect();
    if contracts.is_empty() {
        return Err(HeatmapError::invalid(index, "no such logical section"));
    }
    let mut class = None;
    let mut unit = None;
    for field in &ranking.fields {
        let columns: Vec<_> = contracts
            .iter()
            .filter_map(|contract| contract.column(field))
            .collect();
        if columns.is_empty() {
            return Err(HeatmapError::invalid(
                index,
                format!("no such column {field:?}"),
            ));
        }
        for column in columns {
            if !matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge) {
                return Err(HeatmapError::invalid(
                    index,
                    format!("column {field:?} is not numeric"),
                ));
            }
            if class
                .replace(column.class)
                .is_some_and(|seen| seen != column.class)
            {
                return Err(HeatmapError::invalid(
                    index,
                    format!(
                        "fields carry different classes: {}",
                        ranking.fields.join("+")
                    ),
                ));
            }
            if unit
                .replace(column.unit)
                .is_some_and(|seen| seen != column.unit)
            {
                return Err(HeatmapError::invalid(
                    index,
                    format!("fields carry different units: {}", ranking.fields.join("+")),
                ));
            }
        }
    }
    for group in item.view.groups() {
        if group.is_empty()
            || !contracts
                .iter()
                .any(|contract| contract.column(group).is_some())
        {
            return Err(HeatmapError::invalid(
                index,
                format!("no such column {group:?}"),
            ));
        }
    }
    let mut labels = Vec::new();
    let mut label_seen = HashSet::new();
    for contract in contracts {
        for column in contract.columns {
            if column.class == ColumnClass::Label && label_seen.insert(column.name) {
                labels.push(column.name.to_owned());
            }
        }
    }
    Ok((
        class.expect("one validated numeric field"),
        unit.expect("one validated numeric field"),
        labels,
    ))
}

struct PhysicalPlan {
    type_id: u32,
    contract: &'static kronika_registry::TypeContract,
    rows: u64,
    timestamp: &'static str,
    projection: Vec<&'static str>,
    bindings: Vec<Binding>,
    first_index: usize,
}

struct Binding {
    accumulator: usize,
    metrics: Vec<&'static str>,
    groups: Vec<Option<&'static str>>,
    labels: Vec<Option<&'static str>>,
}

fn physical_plans(segment: &Segment, specs: &[ItemSpec]) -> Vec<PhysicalPlan> {
    let mut plans = Vec::new();
    let mut sections = Vec::new();
    let mut seen = HashSet::new();
    for spec in specs {
        let section = spec.query.ranking.section.as_str();
        if seen.insert(section) {
            sections.push(section);
        }
    }
    for section in sections {
        for (type_id, stored) in segment.layouts(section) {
            let Some(contract) = contract(type_id) else {
                continue;
            };
            let Some(timestamp) = contract
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name)
            else {
                continue;
            };
            let mut projection = vec![timestamp];
            projection.extend(contract.identity.iter().copied());
            let mut bindings = Vec::new();
            let mut first_index = usize::MAX;
            for (accumulator, spec) in specs.iter().enumerate() {
                if spec.query.ranking.section != section
                    || spec
                        .query
                        .view
                        .type_id()
                        .is_some_and(|wanted| wanted != type_id)
                {
                    continue;
                }
                let metrics: Vec<_> = spec
                    .query
                    .ranking
                    .fields
                    .iter()
                    .filter_map(|name| contract.column(name).map(|column| column.name))
                    .collect();
                if metrics.is_empty() {
                    continue;
                }
                let groups = spec
                    .query
                    .view
                    .groups()
                    .iter()
                    .map(|name| contract.column(name).map(|column| column.name))
                    .collect::<Vec<_>>();
                let labels = if spec.query.view.groups().is_empty() {
                    spec.labels
                        .iter()
                        .map(|name| contract.column(name).map(|column| column.name))
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                projection.extend(metrics.iter().copied());
                projection.extend(groups.iter().flatten().copied());
                projection.extend(labels.iter().flatten().copied());
                bindings.push(Binding {
                    accumulator,
                    metrics,
                    groups,
                    labels,
                });
                first_index = first_index.min(spec.first_index);
            }
            if bindings.is_empty() {
                continue;
            }
            projection.sort_unstable();
            projection.dedup();
            plans.push(PhysicalPlan {
                type_id,
                contract,
                rows: stored.rows,
                timestamp,
                projection,
                bindings,
                first_index,
            });
        }
    }
    plans
}

fn scan_plan(
    segment: &Segment,
    plan: &PhysicalPlan,
    range: crate::api::time::TimeRange,
    accumulators: &mut [Accumulator],
    budget: &mut WorkingBudget,
    cancelled: &impl Fn() -> bool,
    #[cfg(test)] row_visits: &std::sync::atomic::AtomicU64,
) -> Result<(), HeatmapError> {
    let take = usize::try_from(plan.rows).unwrap_or(usize::MAX);
    let mut visited = 0_usize;
    let mut failure = None;
    segment
        .visit_rows(plan.type_id, &plan.projection, 0, take, |ordinal, row| {
            #[cfg(test)]
            row_visits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if cancelled() {
                failure = Some(HeatmapError::storage(plan.first_index, "request cancelled"));
                return false;
            }
            visited = visited.saturating_add(1);
            let Some(Cell::Ts(timestamp)) = row.get(plan.timestamp) else {
                return true;
            };
            for binding in &plan.bindings {
                let accumulator = &mut accumulators[binding.accumulator];
                let timestamp = *timestamp;
                if timestamp < range.from {
                    accumulator.scan.nearest_before = Some(
                        accumulator
                            .scan
                            .nearest_before
                            .map_or(timestamp, |seen| seen.max(timestamp)),
                    );
                    continue;
                }
                if timestamp >= range.to_exclusive {
                    accumulator.scan.nearest_after = Some(
                        accumulator
                            .scan
                            .nearest_after
                            .map_or(timestamp, |seen| seen.min(timestamp)),
                    );
                    continue;
                }
                accumulator.scan.window_rows = accumulator.scan.window_rows.saturating_add(1);
                if let Err(error) = accumulator.observe(
                    plan.type_id,
                    plan.contract,
                    &row,
                    ordinal,
                    timestamp,
                    binding,
                    budget,
                ) {
                    failure = Some(error);
                    return false;
                }
            }
            failure.is_none()
        })
        .map_err(|error| HeatmapError::storage(plan.first_index, error))?;
    if let Some(error) = failure {
        return Err(error);
    }
    debug_assert!(visited <= take, "row visitor exceeded its declared bound");
    Ok(())
}

#[derive(Default)]
struct WorkingBudget {
    used: usize,
}

impl WorkingBudget {
    fn reserve(&mut self, bytes: usize, index: usize) -> Result<(), HeatmapError> {
        let used = self.used.checked_add(bytes).ok_or_else(|| {
            HeatmapError::invalid(index, "heatmap execution working set size overflows")
        })?;
        if used > WORKING_SET_MAX_BYTES {
            return Err(HeatmapError::invalid(
                index,
                format!(
                    "heatmap execution exceeds {WORKING_SET_MAX_BYTES} working bytes; split rankings into several calls or reduce top"
                ),
            ));
        }
        self.used = used;
        Ok(())
    }
}

#[derive(Default)]
struct ScanStats {
    nearest_before: Option<i64>,
    nearest_after: Option<i64>,
    window_rows: u64,
}

struct Accumulator {
    range: crate::api::time::TimeRange,
    columns: usize,
    cumulative: bool,
    grouped: bool,
    labels: usize,
    first_index: usize,
    entities: HashMap<String, EntityState>,
    totals: Vec<CellSum>,
    groups: Vec<GroupState>,
    group_index: HashMap<String, usize>,
    out_of_order: u64,
    as_of: Option<i64>,
    scan: ScanStats,
}

struct EntityState {
    type_id: u32,
    identity: Vec<Cell>,
    labels: Vec<Option<StoredLabel>>,
    numeric: bool,
    window: Obs,
    column: usize,
    current: Obs,
    carry: Option<(i64, f64)>,
    cells: Vec<Obs>,
    grid_carry: Option<(i64, f64)>,
    group: Option<usize>,
}

#[derive(Clone)]
struct StoredLabel {
    timestamp: i64,
    ordinal: u64,
    value: Cell,
}

struct GroupState {
    values: Vec<Cell>,
    members: u32,
}

impl Accumulator {
    fn new(spec: &ItemSpec, range: crate::api::time::TimeRange) -> Self {
        let columns = spec.query.view.columns();
        let grouped = !spec.query.view.groups().is_empty();
        Self {
            range,
            columns,
            cumulative: spec.class == ColumnClass::Cumulative,
            grouped,
            labels: if grouped { 0 } else { spec.labels.len() },
            first_index: spec.first_index,
            entities: HashMap::new(),
            totals: vec![CellSum::default(); columns],
            groups: Vec::new(),
            group_index: HashMap::new(),
            out_of_order: 0,
            as_of: None,
            scan: ScanStats::default(),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one decoded row and its physical binding"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "the one-pass fold keeps row-derived state updates together"
    )]
    fn observe(
        &mut self,
        type_id: u32,
        contract: &'static kronika_registry::TypeContract,
        row: &Row,
        ordinal: u64,
        timestamp: i64,
        binding: &Binding,
        budget: &mut WorkingBudget,
    ) -> Result<(), HeatmapError> {
        let identity: Vec<Cell> = contract
            .identity
            .iter()
            .map(|name| row.get(name).cloned().unwrap_or(Cell::Null))
            .collect();
        let mut key = String::new();
        raw_key_into(&mut key, type_id, &identity);
        if !self.entities.contains_key(&key) {
            let bytes = key
                .len()
                .saturating_add(identity.len().saturating_mul(size_of::<Cell>()))
                .saturating_add(self.labels.saturating_mul(size_of::<Option<StoredLabel>>()))
                .saturating_add(self.columns.saturating_mul(size_of::<Obs>()))
                .saturating_add(size_of::<EntityState>());
            budget.reserve(bytes, self.first_index)?;
            self.entities.insert(
                key.clone(),
                EntityState {
                    type_id,
                    identity,
                    labels: vec![None; self.labels],
                    numeric: false,
                    window: Obs::default(),
                    column: 0,
                    current: Obs::default(),
                    carry: None,
                    cells: vec![Obs::default(); self.columns],
                    grid_carry: None,
                    group: None,
                },
            );
        }
        let state = self.entities.get_mut(&key).expect("inserted entity");
        for (slot, column) in state.labels.iter_mut().zip(&binding.labels) {
            let Some(value) = column.and_then(|name| row.get(name)) else {
                continue;
            };
            if matches!(value, Cell::Null) {
                continue;
            }
            let replace = slot
                .as_ref()
                .is_none_or(|stored| (timestamp, ordinal) >= (stored.timestamp, stored.ordinal));
            if replace {
                *slot = Some(StoredLabel {
                    timestamp,
                    ordinal,
                    value: value.clone(),
                });
            }
        }

        let Some(value) = summed(row, &binding.metrics) else {
            return Ok(());
        };
        self.as_of = Some(self.as_of.map_or(timestamp, |seen| seen.max(timestamp)));
        if !state.numeric {
            state.numeric = true;
            state.column = column_of(timestamp, self.range, self.columns);
            if self.grouped {
                let values: Vec<Cell> = binding
                    .groups
                    .iter()
                    .map(|column| {
                        column
                            .and_then(|name| row.get(name))
                            .cloned()
                            .unwrap_or(Cell::Null)
                    })
                    .collect();
                let mut group_key = String::new();
                raw_key_into(&mut group_key, 0, &values);
                let group = *self.group_index.entry(group_key).or_insert_with(|| {
                    self.groups.push(GroupState { values, members: 0 });
                    self.groups.len() - 1
                });
                self.groups[group].members = self.groups[group].members.saturating_add(1);
                state.group = Some(group);
            }
        }

        let previous_ts = (state.window.count > 0).then_some(state.window.last_ts);
        let column = column_of_span(previous_ts, timestamp, self.range, self.columns);
        state.window.observe(timestamp, value);
        if !self.grouped || column >= state.column {
            let grid_column = column_of_span(
                state.grid_carry.map(|(carry_ts, _value)| carry_ts),
                timestamp,
                self.range,
                self.columns,
            );
            if self.cumulative
                && state.cells[grid_column].count == 0
                && let Some((carry_ts, carry_value)) = state.grid_carry
                && carry_ts < timestamp
            {
                state.cells[grid_column].observe(carry_ts, carry_value);
            }
            state.cells[grid_column].observe(timestamp, value);
            if state
                .grid_carry
                .is_none_or(|(carry_ts, _value)| carry_ts <= timestamp)
            {
                state.grid_carry = Some((timestamp, value));
            }
        }
        if column < state.column {
            self.out_of_order = self.out_of_order.saturating_add(1);
            return Ok(());
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
            if self.cumulative
                && let Some((carry_ts, carry_value)) = state.carry
            {
                state.current.observe(carry_ts, carry_value);
            }
        }
        state.current.observe(timestamp, value);
        Ok(())
    }

    fn collect_ids(
        &self,
        retained: &mut HashSet<u64>,
        budget: &mut WorkingBudget,
    ) -> Result<(), HeatmapError> {
        for state in self.entities.values() {
            if !state.numeric {
                continue;
            }
            reserve_ids(&state.identity, retained, budget, self.first_index)?;
            for label in state.labels.iter().flatten() {
                reserve_id(&label.value, retained, budget, self.first_index)?;
            }
        }
        for group in &self.groups {
            reserve_ids(&group.values, retained, budget, self.first_index)?;
        }
        Ok(())
    }

    fn finish(
        mut self,
        spec: &ItemSpec,
        recorded: Option<(i64, i64)>,
        dictionary: &HashMap<u64, Value>,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        let mut ranked = Vec::new();
        for (_raw_key, state) in self.entities.drain() {
            if !state.numeric {
                continue;
            }
            if let Some(finished) = state.current.cell(self.cumulative) {
                self.totals[state.column].add(finished);
            }
            let identity_values = render_cells(&state.identity, dictionary, self.first_index)?;
            let mut key = String::new();
            entity_key_into(&mut key, state.type_id, &identity_values);
            ranked.push(RankedState {
                key,
                total: state.window.total(self.cumulative),
                identity_values,
                state,
            });
        }
        ranked.sort_by(|left, right| match (left.total, right.total) {
            (Some(left_total), Some(right_total)) => right_total
                .partial_cmp(&left_total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.key.cmp(&right.key)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.key.cmp(&right.key),
        });
        let physical_entity_count = u64::try_from(ranked.len()).unwrap_or(u64::MAX);
        let totals_total =
            summary_total(ranked.iter().filter_map(|row| row.total), self.cumulative);
        let coverage = HeatmapCoverage {
            state: if ranked.is_empty() {
                CoverageState::NoData
            } else {
                CoverageState::Data
            },
            recorded_from: recorded.map(|(from, _to)| from),
            recorded_to: recorded.map(|(_from, to)| to),
            nearest_row_before: self.scan.nearest_before,
            nearest_row_after: self.scan.nearest_after,
            window_rows: self.scan.window_rows,
        };
        let ranking = spec.query.ranking.clone();
        if self.grouped {
            return self.finish_grouped(
                spec,
                &ranked,
                physical_entity_count,
                totals_total,
                coverage,
                ranking,
                dictionary,
            );
        }
        self.finish_ungrouped(
            spec,
            ranked,
            physical_entity_count,
            totals_total,
            coverage,
            ranking,
            dictionary,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the typed result's shared completed parts"
    )]
    fn finish_ungrouped(
        self,
        spec: &ItemSpec,
        mut ranked: Vec<RankedState>,
        entity_count: u64,
        totals_total: Option<f64>,
        coverage: HeatmapCoverage,
        ranking: super::query::NormalizedRanking,
        dictionary: &HashMap<u64, Value>,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        let top = spec.query.ranking.top;
        let others_total = summary_total(
            ranked.iter().skip(top).filter_map(|row| row.total),
            self.cumulative,
        );
        let totals = self.totals;
        ranked.truncate(top);
        let mut winner_sums = vec![CellSum::default(); self.columns];
        let mut entities = Vec::with_capacity(ranked.len());
        for row in ranked {
            for (sum, observed) in winner_sums.iter_mut().zip(&row.state.cells) {
                if let Some(value) = observed.cell(self.cumulative) {
                    sum.add(value);
                }
            }
            entities.push(HeatmapEntity {
                type_id: row.state.type_id,
                identity: identity_object(row.state.type_id, row.identity_values),
                labels: labels_object(spec, &row.state.labels, dictionary, self.first_index)?,
                total: row.total,
                cells: matches!(spec.query.view, HeatmapView::Grid { .. }).then(|| {
                    row.state
                        .cells
                        .iter()
                        .map(|observed| observed.cell(self.cumulative))
                        .collect()
                }),
            });
        }
        let grid = match &spec.query.view {
            HeatmapView::RankingOnly => None,
            HeatmapView::Grid { group, .. } => {
                debug_assert!(group.is_empty(), "ungrouped result carried group fields");
                let other_cells: Vec<Option<f64>> = totals
                    .iter()
                    .zip(&winner_sums)
                    .map(|(all, winners)| all.minus(winners))
                    .collect();
                Some(HeatmapGrid {
                    label_names: spec.labels.clone(),
                    group_names: Vec::new(),
                    intervals: intervals(self.range, self.columns),
                    groups: Vec::new(),
                    totals: HeatmapBand {
                        total: if self.cumulative {
                            totals_total
                        } else {
                            band_peak(&totals)
                        },
                        cells: totals.iter().map(CellSum::value).collect(),
                    },
                    others: HeatmapBand {
                        total: if self.cumulative {
                            others_total
                        } else {
                            peak_values(&other_cells)
                        },
                        cells: other_cells,
                    },
                })
            }
        };
        Ok(HeatmapItemResult {
            ranking,
            as_of: self.as_of,
            coverage,
            class: spec.class,
            unit: spec.unit,
            entities,
            totals_total,
            others_total,
            entity_count,
            out_of_order: self.out_of_order,
            grid,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the typed result's shared completed parts"
    )]
    fn finish_grouped(
        self,
        spec: &ItemSpec,
        ranked: &[RankedState],
        _physical_entity_count: u64,
        _ranking_totals_total: Option<f64>,
        coverage: HeatmapCoverage,
        ranking: super::query::NormalizedRanking,
        dictionary: &HashMap<u64, Value>,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        let totals = self.totals;
        let mut group_totals = vec![None; self.groups.len()];
        let mut group_cells = vec![vec![CellSum::default(); self.columns]; self.groups.len()];
        for row in ranked {
            let Some(group) = row.state.group else {
                continue;
            };
            if let Some(total) = row.total {
                group_totals[group] = Some(group_totals[group].unwrap_or(0.0) + total);
            }
            for (sum, observed) in group_cells[group].iter_mut().zip(&row.state.cells) {
                if let Some(value) = observed.cell(self.cumulative) {
                    sum.add(value);
                }
            }
        }
        let mut order: Vec<usize> = (0..self.groups.len()).collect();
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
        let top = spec.query.ranking.top.min(order.len());
        let winners: HashSet<usize> = order.iter().take(top).copied().collect();
        let mut others = vec![CellSum::default(); self.columns];
        for group in order.iter().skip(top) {
            for (sum, value) in others.iter_mut().zip(&group_cells[*group]) {
                if let Some(value) = value.value() {
                    sum.add(value);
                }
            }
        }
        let mut groups = Vec::with_capacity(top);
        for group in order.iter().take(top) {
            let state = &self.groups[*group];
            groups.push(HeatmapGroup {
                values: render_cells(&state.values, dictionary, self.first_index)?,
                members: state.members,
                total: group_totals[*group],
                cells: group_cells[*group].iter().map(CellSum::value).collect(),
            });
        }
        let totals_total = if self.cumulative {
            summary_total(group_totals.iter().flatten().copied(), true)
        } else {
            band_peak(&totals)
        };
        let others_total = if self.cumulative {
            summary_total(
                order
                    .iter()
                    .filter(|group| !winners.contains(group))
                    .filter_map(|group| group_totals[*group]),
                true,
            )
        } else {
            band_peak(&others)
        };
        let grid = Some(HeatmapGrid {
            label_names: spec.labels.clone(),
            group_names: spec.query.view.groups().to_vec(),
            intervals: intervals(self.range, self.columns),
            groups,
            totals: HeatmapBand {
                total: totals_total,
                cells: totals.iter().map(CellSum::value).collect(),
            },
            others: HeatmapBand {
                total: others_total,
                cells: others.iter().map(CellSum::value).collect(),
            },
        });
        Ok(HeatmapItemResult {
            ranking,
            as_of: self.as_of,
            coverage,
            class: spec.class,
            unit: spec.unit,
            entities: Vec::new(),
            totals_total,
            others_total,
            entity_count: u64::try_from(self.groups.len()).unwrap_or(u64::MAX),
            out_of_order: self.out_of_order,
            grid,
        })
    }
}

struct RankedState {
    key: String,
    total: Option<f64>,
    identity_values: Vec<Value>,
    state: EntityState,
}

fn summary_total(values: impl Iterator<Item = f64>, cumulative: bool) -> Option<f64> {
    values.fold(None, |current, value| {
        Some(match current {
            Some(current) if !cumulative => current.max(value),
            Some(current) => current + value,
            None => value,
        })
    })
}

fn identity_object(type_id: u32, values: Vec<Value>) -> NamedValues {
    let names = contract(type_id)
        .map(|contract| contract.identity)
        .unwrap_or_default();
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let name = names.get(index).map_or_else(
                || format!("value_{index}"),
                |name| public_identity_name(name).to_owned(),
            );
            (name, value)
        })
        .collect()
}

fn public_identity_name(name: &str) -> &str {
    IDENTITY_ALIASES
        .iter()
        .find(|(recorded, _public)| recorded == &name)
        .map_or(name, |(_recorded, public)| public)
}

fn labels_object(
    spec: &ItemSpec,
    labels: &[Option<StoredLabel>],
    dictionary: &HashMap<u64, Value>,
    index: usize,
) -> Result<NamedValues, HeatmapError> {
    spec.labels
        .iter()
        .zip(labels)
        .map(|(name, stored)| {
            let value = stored.as_ref().map_or(Ok(Value::Null), |stored| {
                render_cell(&stored.value, dictionary, index)
            })?;
            Ok((name.clone(), value))
        })
        .collect()
}

fn render_cells(
    cells: &[Cell],
    dictionary: &HashMap<u64, Value>,
    index: usize,
) -> Result<Vec<Value>, HeatmapError> {
    cells
        .iter()
        .map(|stored| render_cell(stored, dictionary, index))
        .collect()
}

fn render_cell(
    stored: &Cell,
    dictionary: &HashMap<u64, Value>,
    index: usize,
) -> Result<Value, HeatmapError> {
    if let Cell::StrId(id) = stored {
        return dictionary
            .get(id)
            .cloned()
            .ok_or_else(|| HeatmapError::storage(index, format!("unresolved dictionary id {id}")));
    }
    cell(stored, &Dictionary::default()).map_err(|error| HeatmapError::storage(index, error))
}

fn reserve_ids(
    cells: &[Cell],
    retained: &mut HashSet<u64>,
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    for stored in cells {
        reserve_id(stored, retained, budget, index)?;
    }
    Ok(())
}

fn reserve_id(
    stored: &Cell,
    retained: &mut HashSet<u64>,
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    if let Cell::StrId(id) = stored
        && retained.insert(*id)
    {
        budget.reserve(size_of::<u64>().saturating_mul(4), index)?;
    }
    Ok(())
}

fn raw_key_into(key: &mut String, type_id: u32, cells: &[Cell]) {
    use std::fmt::Write as _;
    key.clear();
    let _ = write!(key, "{type_id}:");
    for cell in cells {
        match cell {
            Cell::I16(value) => {
                let _ = write!(key, "a{value};");
            }
            Cell::I32(value) => {
                let _ = write!(key, "b{value};");
            }
            Cell::I64(value) => {
                let _ = write!(key, "c{value};");
            }
            Cell::U32(value) => {
                let _ = write!(key, "d{value};");
            }
            Cell::U64(value) => {
                let _ = write!(key, "e{value};");
            }
            Cell::F64(value) => {
                let _ = write!(key, "f{:016x};", value.to_bits());
            }
            Cell::Bool(value) => {
                let _ = write!(key, "g{};", u8::from(*value));
            }
            Cell::Ts(value) => {
                let _ = write!(key, "h{value};");
            }
            Cell::StrId(value) => {
                let _ = write!(key, "i{value:016x};");
            }
            Cell::ListI32(values) => {
                let _ = write!(key, "j{}:", values.len());
                for value in values {
                    let _ = write!(key, "{value},");
                }
                key.push(';');
            }
            Cell::Null => key.push_str("n;"),
        }
    }
}

pub(super) fn summed(row: &Row, columns: &[&str]) -> Option<f64> {
    let mut sum = None;
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

fn intervals(range: crate::api::time::TimeRange, columns: usize) -> Vec<HeatmapInterval> {
    (0..columns)
        .map(|index| HeatmapInterval {
            start: interval_start(range, columns, index),
            end: interval_start(range, columns, index + 1).saturating_sub(1),
        })
        .collect()
}

pub(super) fn interval_start(
    range: crate::api::time::TimeRange,
    columns: usize,
    index: usize,
) -> i64 {
    let span = i128::from(range.to_exclusive) - i128::from(range.from);
    let offset = span * to_i128(index) / to_i128(columns.max(1));
    range.from.saturating_add(clamped(offset))
}

pub(super) fn column_of_span(
    previous_ts: Option<i64>,
    timestamp: i64,
    range: crate::api::time::TimeRange,
    columns: usize,
) -> usize {
    let middle = match previous_ts {
        Some(previous) if previous < timestamp => previous + (timestamp - previous) / 2,
        _ => timestamp,
    };
    column_of(middle.max(range.from), range, columns)
}

pub(super) fn column_of(
    timestamp: i64,
    range: crate::api::time::TimeRange,
    columns: usize,
) -> usize {
    let span = (i128::from(range.to_exclusive) - i128::from(range.from)).max(1);
    let offset = i128::from(timestamp) - i128::from(range.from);
    let column = (offset * to_i128(columns.max(1)) / span).max(0);
    usize::try_from(column)
        .unwrap_or_else(|_| columns.saturating_sub(1))
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
    pub(super) fn observe(&mut self, timestamp: i64, value: f64) {
        if self.count == 0 {
            *self = Self {
                count: 1,
                first_ts: timestamp,
                first_value: value,
                last_ts: timestamp,
                last_value: value,
                max_value: value,
            };
            return;
        }
        self.count = self.count.saturating_add(1);
        if timestamp < self.first_ts {
            self.first_ts = timestamp;
            self.first_value = value;
        }
        if timestamp >= self.last_ts {
            self.last_ts = timestamp;
            self.last_value = value;
        }
        if value > self.max_value {
            self.max_value = value;
        }
    }

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
struct CellSum {
    sum: f64,
    contributors: u32,
}

impl CellSum {
    fn add(&mut self, value: f64) {
        self.sum += value;
        self.contributors = self.contributors.saturating_add(1);
    }

    fn value(&self) -> Option<f64> {
        (self.contributors > 0).then_some(self.sum)
    }

    fn minus(&self, winners: &Self) -> Option<f64> {
        let contributors = self.contributors.saturating_sub(winners.contributors);
        (contributors > 0).then_some(self.sum - winners.sum)
    }
}

fn band_peak(cells: &[CellSum]) -> Option<f64> {
    cells
        .iter()
        .filter_map(CellSum::value)
        .fold(None, |current, value| {
            Some(current.map_or(value, |stored: f64| stored.max(value)))
        })
}

fn peak_values(cells: &[Option<f64>]) -> Option<f64> {
    cells
        .iter()
        .flatten()
        .copied()
        .fold(None, |current, value| {
            Some(current.map_or(value, |stored: f64| stored.max(value)))
        })
}

fn check_result_budget(result: &HeatmapBatchResult) -> Result<(), HeatmapError> {
    let mut budget = ResultBudget {
        remaining: RESULT_MAX_BYTES,
    };
    std::io::Write::write_all(&mut budget, b"{\"results\":[")
        .map_err(|_error| HeatmapError::invalid(0, result_overflow_message()))?;
    for (index, item) in result.results.iter().enumerate() {
        if index > 0 {
            std::io::Write::write_all(&mut budget, b",")
                .map_err(|_error| HeatmapError::invalid(index, result_overflow_message()))?;
        }
        serde_json::to_writer(&mut budget, item)
            .map_err(|_error| HeatmapError::invalid(index, result_overflow_message()))?;
    }
    std::io::Write::write_all(&mut budget, b"]}").map_err(|_error| {
        HeatmapError::invalid(
            result.results.len().saturating_sub(1),
            result_overflow_message(),
        )
    })?;
    Ok(())
}

fn result_overflow_message() -> String {
    format!(
        "heatmap result exceeds {RESULT_MAX_BYTES} encoded bytes; split rankings into several calls or reduce top"
    )
}

struct ResultBudget {
    remaining: usize,
}

impl std::io::Write for ResultBudget {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(std::io::Error::other("over budget"));
        }
        self.remaining -= buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn emit_http(
    item: &HeatmapItemResult,
    from: i64,
    to: i64,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let grid = item
        .grid
        .as_ref()
        .ok_or_else(|| ApiError::BadFilter("HTTP heatmap requires a grid result".to_owned()))?;
    let grouped = !grid.group_names.is_empty();
    let mut header = json!({
        "record": "heatmap",
        "from": from.to_string(),
        "to": to.to_string(),
        "section": item.ranking.section,
        "fields": item.ranking.fields,
        "class": item.class.code(),
        "labels": grid.label_names,
        "top": if grouped { grid.groups.len() } else { item.entities.len() },
        "entity_count": item.entity_count,
        "others_count": item.entity_count.saturating_sub(
            u64::try_from(if grouped { grid.groups.len() } else { item.entities.len() })
                .unwrap_or(u64::MAX)
        ),
        "out_of_order": item.out_of_order.to_string(),
        "intervals": grid.intervals,
    });
    if grouped {
        header
            .as_object_mut()
            .expect("heatmap header is an object")
            .insert("group".to_owned(), json!(grid.group_names));
    }
    if cancelled() || !emit(record(header)?) {
        return Ok(());
    }
    if grouped {
        for group in &grid.groups {
            if cancelled()
                || !emit(record(json!({
                    "record": "heatmap_row",
                    "type_id": "0",
                    "identity": group.values,
                    "labels": [],
                    "members": group.members,
                    "total": group.total,
                    "cells": group.cells,
                }))?)
            {
                return Ok(());
            }
        }
    } else {
        for entity in &item.entities {
            let labels: Vec<Value> = grid
                .label_names
                .iter()
                .map(|name| entity.labels.get(name).cloned().unwrap_or(Value::Null))
                .collect();
            if cancelled()
                || !emit(record(json!({
                    "record": "heatmap_row",
                    "type_id": entity.type_id.to_string(),
                    "identity": http_identity(entity.type_id, &entity.identity),
                    "labels": labels,
                    "total": entity.total,
                    "cells": entity.cells,
                }))?)
            {
                return Ok(());
            }
        }
    }
    if !emit(record(json!({
        "record": "heatmap_band",
        "band": "totals",
        "total": grid.totals.total,
        "cells": grid.totals.cells,
    }))?) {
        return Ok(());
    }
    if !emit(record(json!({
        "record": "heatmap_band",
        "band": "others",
        "total": grid.others.total,
        "cells": grid.others.cells,
    }))?) {
        return Ok(());
    }
    Ok(())
}

fn http_identity(type_id: u32, identity: &BTreeMap<String, Value>) -> Vec<Value> {
    let Some(contract) = contract(type_id) else {
        return identity.values().cloned().collect();
    };
    contract
        .identity
        .iter()
        .map(|name| {
            identity
                .get(public_identity_name(name))
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect()
}

#[cfg(test)]
impl PreparedHeatmapBatch {
    pub(crate) fn execution_operations(&self) -> u64 {
        self.executions.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn row_visits(&self) -> u64 {
        self.row_visits.load(std::sync::atomic::Ordering::Relaxed)
    }
}
