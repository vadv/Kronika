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
use crate::budget::ByteBudget;

const WORKING_SET_MAX_BYTES: usize = 8 * 1024 * 1024;
const RESULT_MAX_BYTES: usize = 8 * 1024 * 1024;
const IDENTITY_ALIASES: [(&str, &str); 2] = [("queryid", "query_id"), ("planid", "plan_id")];
type RenderedIds = HashMap<(usize, u64), Value>;

#[derive(Debug)]
enum HeatmapApiError {
    BadFilter(String),
    NoSuchSection,
    NoSuchColumn(String),
    MixedUnits(String),
}

#[derive(Debug)]
pub(crate) struct HeatmapError {
    ranking_index: usize,
    message: String,
    api_error: Option<HeatmapApiError>,
    valid_options: Vec<String>,
}

impl HeatmapError {
    fn invalid(ranking_index: usize, message: impl Into<String>) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapApiError::BadFilter("heatmap".to_owned()),
            Vec::new(),
        )
    }

    fn invalid_parameter(
        ranking_index: usize,
        message: impl Into<String>,
        parameter: &str,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapApiError::BadFilter(parameter.to_owned()),
            Vec::new(),
        )
    }

    fn no_such_section(
        ranking_index: usize,
        message: impl Into<String>,
        valid_options: Vec<String>,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapApiError::NoSuchSection,
            valid_options,
        )
    }

    fn no_such_column(
        ranking_index: usize,
        message: impl Into<String>,
        column: String,
        valid_options: Vec<String>,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapApiError::NoSuchColumn(column),
            valid_options,
        )
    }

    fn mixed_units(ranking_index: usize, message: impl Into<String>, fields: String) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapApiError::MixedUnits(fields),
            Vec::new(),
        )
    }

    fn invalid_as(
        ranking_index: usize,
        message: impl Into<String>,
        api_error: HeatmapApiError,
        valid_options: Vec<String>,
    ) -> Self {
        Self {
            ranking_index,
            message: message.into(),
            api_error: Some(api_error),
            valid_options,
        }
    }

    fn storage(ranking_index: usize, error: impl std::fmt::Display) -> Self {
        Self {
            ranking_index,
            message: error.to_string(),
            api_error: None,
            valid_options: Vec::new(),
        }
    }

    pub(crate) const fn ranking_index(&self) -> usize {
        self.ranking_index
    }

    pub(crate) fn valid_options(&self) -> &[String] {
        &self.valid_options
    }

    pub(crate) fn into_api(mut self) -> ApiError {
        match self.api_error.take() {
            Some(HeatmapApiError::BadFilter(parameter)) => ApiError::BadFilter(parameter),
            Some(HeatmapApiError::NoSuchSection) => ApiError::NoSuchSection,
            Some(HeatmapApiError::NoSuchColumn(column)) => ApiError::NoSuchColumn(column),
            Some(HeatmapApiError::MixedUnits(fields)) => ApiError::MixedUnits(fields),
            None => ApiError::Unreadable(Box::new(self)),
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
        .map_err(|_error| ApiError::BadFilter("to".to_owned()))?;
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
        for spec in &self.unique {
            budget.reserve_array::<Accumulator>(2, spec.first_index)?;
            if matches!(spec.query.view, HeatmapView::Grid { .. }) {
                budget.reserve_array::<CellSum>(spec.query.view.columns(), spec.first_index)?;
            }
        }
        let mut accumulators = Vec::with_capacity(self.unique.len());
        for spec in &self.unique {
            accumulators.push(Accumulator::new(spec, self.query.range));
        }
        budget.reserve_array::<Segment>(self.segments.len(), 0)?;
        budget.reserve_array::<HashSet<u64>>(self.segments.len(), 0)?;
        budget.reserve_array::<Vec<(u64, usize)>>(self.segments.len(), 0)?;
        let mut opened_segments = Vec::with_capacity(self.segments.len());
        let mut rendered_ids = RenderedIds::new();

        for (segment_slot, segment_ref) in self.segments.iter().enumerate() {
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
                    segment_slot,
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

        let mut retained_ids = vec![HashSet::new(); opened_segments.len()];
        let mut retained_indices = vec![Vec::new(); opened_segments.len()];
        for accumulator in &accumulators {
            accumulator.collect_ids(&mut retained_ids, &mut retained_indices, &mut budget)?;
        }
        for (segment_slot, ((segment, ids), indexed_ids)) in opened_segments
            .iter()
            .zip(&retained_ids)
            .zip(&retained_indices)
            .enumerate()
        {
            if ids.is_empty() {
                continue;
            }
            let Some(index) = indexed_ids.first().map(|(_id, index)| *index) else {
                return Err(HeatmapError::storage(
                    0,
                    "retained dictionary IDs have no dependent ranking",
                ));
            };
            let dictionary = segment
                .dictionary_once_for(ids)
                .map_err(|error| HeatmapError::storage(index, error))?;
            for (id, index) in indexed_ids {
                let value = cell(&Cell::StrId(*id), &dictionary)
                    .map_err(|error| HeatmapError::storage(*index, error))?;
                let bytes = encoded_value_len(&value, *index)?;
                // The selected decoded value and its rendered JSON value
                // coexist until this segment's dictionary is dropped.
                budget.reserve_parts(
                    [
                        bytes,
                        bytes,
                        size_of::<((usize, u64), Value)>(),
                        size_of::<((usize, u64), Value)>(),
                    ],
                    *index,
                )?;
                rendered_ids.insert((segment_slot, *id), value);
            }
        }

        for spec in &self.unique {
            budget.reserve_array::<HeatmapItemResult>(1, spec.first_index)?;
        }
        let mut unique_results = Vec::with_capacity(accumulators.len());
        for (spec, accumulator) in self.unique.iter().zip(accumulators) {
            unique_results.push(accumulator.finish(
                spec,
                self.recorded,
                &rendered_ids,
                &mut budget,
            )?);
        }
        let results = expand_results(&unique_results, &self.original_to_unique, &mut budget)?;
        Ok(HeatmapBatchResult { results })
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
        return Err(HeatmapError::invalid_parameter(
            index,
            "section must contain 1 to 128 UTF-8 bytes",
            "section",
        ));
    }
    if !(1..=MAX_FIELDS).contains(&ranking.fields.len()) {
        return Err(HeatmapError::invalid_parameter(
            index,
            format!("fields must contain 1 to {MAX_FIELDS} names"),
            "field",
        ));
    }
    let mut seen = HashSet::new();
    for field in &ranking.fields {
        if field.is_empty() || !seen.insert(field) {
            return Err(HeatmapError::invalid_parameter(
                index,
                format!("field {field:?} is empty or repeated"),
                "field",
            ));
        }
    }
    if !(1..=MAX_TOP).contains(&ranking.top) {
        return Err(HeatmapError::invalid_parameter(
            index,
            format!("top must be between 1 and {MAX_TOP}, got {}", ranking.top),
            "top",
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
        let mut seen = HashSet::new();
        let valid_options = registry()
            .iter()
            .filter_map(|contract| logical_section_name(contract.type_id.get()))
            .filter(|section| seen.insert(*section))
            .map(str::to_owned)
            .collect();
        return Err(HeatmapError::no_such_section(
            index,
            "no such logical section",
            valid_options,
        ));
    }
    let numeric_options = || {
        let mut seen = HashSet::new();
        contracts
            .iter()
            .flat_map(|contract| contract.columns)
            .filter(|column| {
                matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge)
                    && seen.insert(column.name)
            })
            .map(|column| column.name.to_owned())
            .collect::<Vec<_>>()
    };
    let mut class = None;
    let mut unit = None;
    for field in &ranking.fields {
        let columns: Vec<_> = contracts
            .iter()
            .filter_map(|contract| contract.column(field))
            .collect();
        if columns.is_empty() {
            return Err(HeatmapError::no_such_column(
                index,
                format!("no such column {field:?}"),
                field.clone(),
                numeric_options(),
            ));
        }
        for column in columns {
            if !matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge) {
                return Err(HeatmapError::no_such_column(
                    index,
                    format!("column {field:?} is not numeric"),
                    field.clone(),
                    numeric_options(),
                ));
            }
            if class
                .replace(column.class)
                .is_some_and(|seen| seen != column.class)
            {
                return Err(HeatmapError::no_such_column(
                    index,
                    format!(
                        "fields carry different classes: {}",
                        ranking.fields.join("+")
                    ),
                    field.clone(),
                    numeric_options(),
                ));
            }
            if unit
                .replace(column.unit)
                .is_some_and(|seen| seen != column.unit)
            {
                let fields = ranking.fields.join("+");
                return Err(HeatmapError::mixed_units(
                    index,
                    format!("fields carry different units: {fields}"),
                    fields,
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
            let mut seen = HashSet::new();
            let valid_options = contracts
                .iter()
                .flat_map(|contract| contract.columns)
                .filter(|column| seen.insert(column.name))
                .map(|column| column.name.to_owned())
                .collect();
            return Err(HeatmapError::no_such_column(
                index,
                format!("no such column {group:?}"),
                group.clone(),
                valid_options,
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

#[cfg_attr(
    test,
    expect(
        clippy::too_many_arguments,
        reason = "one physical scan's explicit shared state"
    )
)]
fn scan_plan(
    segment: &Segment,
    segment_slot: usize,
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
                    segment_slot,
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

    fn reserve_array<T>(&mut self, count: usize, index: usize) -> Result<(), HeatmapError> {
        let bytes = size_of::<T>().checked_mul(count).ok_or_else(|| {
            HeatmapError::invalid(index, "heatmap execution working set size overflows")
        })?;
        self.reserve(bytes, index)
    }

    fn reserve_parts(
        &mut self,
        parts: impl IntoIterator<Item = usize>,
        index: usize,
    ) -> Result<(), HeatmapError> {
        let mut bytes = 0_usize;
        for part in parts {
            bytes = bytes.checked_add(part).ok_or_else(|| {
                HeatmapError::invalid(index, "heatmap execution working set size overflows")
            })?;
        }
        self.reserve(bytes, index)
    }

    fn reserve_matrix<T>(
        &mut self,
        rows: usize,
        columns: usize,
        index: usize,
    ) -> Result<(), HeatmapError> {
        let count = rows.checked_mul(columns).ok_or_else(|| {
            HeatmapError::invalid(index, "heatmap execution working set size overflows")
        })?;
        self.reserve_array::<T>(count, index)
    }

    fn release(&mut self, bytes: usize) {
        debug_assert!(
            bytes <= self.used,
            "working-set release exceeds the reserved bytes"
        );
        self.used = self.used.saturating_sub(bytes);
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
    grid: bool,
    grouped: bool,
    labels: usize,
    top: usize,
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
    identity_segment: usize,
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
    segment_slot: usize,
    timestamp: i64,
    ordinal: u64,
    value: Cell,
    reserved_heap: usize,
}

struct GroupState {
    segment_slot: usize,
    values: Vec<Cell>,
    members: u32,
}

#[derive(Clone, Copy)]
enum LabelCutoff {
    Value(f64),
    Null,
}

impl Accumulator {
    fn new(spec: &ItemSpec, range: crate::api::time::TimeRange) -> Self {
        let columns = spec.query.view.columns();
        let grid = matches!(spec.query.view, HeatmapView::Grid { .. });
        let grouped = !spec.query.view.groups().is_empty();
        Self {
            range,
            columns,
            cumulative: spec.class == ColumnClass::Cumulative,
            grid,
            grouped,
            labels: if grouped { 0 } else { spec.labels.len() },
            top: spec.query.ranking.top,
            first_index: spec.first_index,
            entities: HashMap::new(),
            totals: if grid {
                vec![CellSum::default(); columns]
            } else {
                Vec::new()
            },
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
        segment_slot: usize,
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
            budget.reserve_parts(
                [
                    key.len(),
                    key.len(),
                    cells_retained_bytes(&identity, self.first_index)?,
                    size_of::<(String, EntityState)>(),
                    size_of::<(String, EntityState)>(),
                ],
                self.first_index,
            )?;
            budget.reserve_array::<Option<StoredLabel>>(self.labels, self.first_index)?;
            if self.grid {
                budget.reserve_array::<Obs>(self.columns, self.first_index)?;
            }
            self.entities.insert(
                key.clone(),
                EntityState {
                    type_id,
                    identity_segment: segment_slot,
                    identity,
                    labels: vec![None; self.labels],
                    numeric: false,
                    window: Obs::default(),
                    column: 0,
                    current: Obs::default(),
                    carry: None,
                    cells: if self.grid {
                        vec![Obs::default(); self.columns]
                    } else {
                        Vec::new()
                    },
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
            let replace = slot.as_ref().is_none_or(|stored| {
                (timestamp, segment_slot, ordinal)
                    >= (stored.timestamp, stored.segment_slot, stored.ordinal)
            });
            if replace {
                let nested = cell_nested_bytes(value, self.first_index)?;
                let replaced_heap = slot.as_ref().map_or(0, |stored| stored.reserved_heap);
                budget.reserve(nested, self.first_index)?;
                *slot = Some(StoredLabel {
                    segment_slot,
                    timestamp,
                    ordinal,
                    value: value.clone(),
                    reserved_heap: nested,
                });
                budget.release(replaced_heap);
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
                let group = if let Some(group) = self.group_index.get(&group_key).copied() {
                    group
                } else {
                    budget.reserve_parts(
                        [
                            group_key.len(),
                            group_key.len(),
                            cells_retained_bytes(&values, self.first_index)?,
                            size_of::<(String, usize)>(),
                            size_of::<(String, usize)>(),
                            size_of::<GroupState>(),
                        ],
                        self.first_index,
                    )?;
                    self.groups.push(GroupState {
                        segment_slot,
                        values,
                        members: 0,
                    });
                    let group = self.groups.len() - 1;
                    self.group_index.insert(group_key, group);
                    group
                };
                self.groups[group].members = self.groups[group].members.saturating_add(1);
                state.group = Some(group);
            }
        }

        let previous_ts = (state.window.count > 0).then_some(state.window.last_ts);
        let column = column_of_span(previous_ts, timestamp, self.range, self.columns);
        state.window.observe(timestamp, value);
        if self.grid && (!self.grouped || column >= state.column) {
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
            if self.grid
                && let Some(finished) = state.current.cell(self.cumulative)
            {
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
        retained: &mut [HashSet<u64>],
        retained_indices: &mut [Vec<(u64, usize)>],
        budget: &mut WorkingBudget,
    ) -> Result<(), HeatmapError> {
        if self.grouped {
            budget.reserve_array::<Option<f64>>(self.groups.len(), self.first_index)?;
            budget.reserve_array::<usize>(self.groups.len(), self.first_index)?;
            budget
                .reserve_array::<(&String, &EntityState)>(self.entities.len(), self.first_index)?;
            let group_totals = self.group_totals();
            let order = group_order(&group_totals);
            for group in order.into_iter().take(self.top) {
                let group = &self.groups[group];
                reserve_ids(
                    &group.values,
                    group.segment_slot,
                    retained,
                    retained_indices,
                    budget,
                    self.first_index,
                )?;
            }
            return Ok(());
        }
        let numeric = self.entities.values().filter(|state| state.numeric).count();
        budget.reserve_array::<Option<f64>>(numeric, self.first_index)?;
        let label_cutoff = self.label_cutoff();
        for state in self.entities.values() {
            if !state.numeric {
                continue;
            }
            reserve_ids(
                &state.identity,
                state.identity_segment,
                retained,
                retained_indices,
                budget,
                self.first_index,
            )?;
            if label_cutoff.is_some_and(|cutoff| {
                ranking_reaches_cutoff(state.window.total(self.cumulative), cutoff)
            }) {
                for label in state.labels.iter().flatten() {
                    reserve_id(
                        &label.value,
                        label.segment_slot,
                        retained,
                        retained_indices,
                        budget,
                        self.first_index,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn group_totals(&self) -> Vec<Option<f64>> {
        let mut totals = vec![None; self.groups.len()];
        for (_key, state) in self.ordered_numeric_states() {
            if let (Some(group), Some(total)) = (state.group, state.window.total(self.cumulative)) {
                totals[group] = Some(totals[group].unwrap_or(0.0) + total);
            }
        }
        totals
    }

    fn ordered_numeric_states(&self) -> Vec<(&String, &EntityState)> {
        let mut states = self
            .entities
            .iter()
            .filter(|(_key, state)| state.numeric)
            .collect::<Vec<_>>();
        states.sort_unstable_by(|(left_key, left), (right_key, right)| {
            compare_totals(
                left.window.total(self.cumulative).as_ref(),
                right.window.total(self.cumulative).as_ref(),
            )
            .then_with(|| left_key.cmp(right_key))
        });
        states
    }

    fn label_cutoff(&self) -> Option<LabelCutoff> {
        if self.grouped {
            return None;
        }
        let mut totals = self
            .entities
            .values()
            .filter(|state| state.numeric)
            .map(|state| state.window.total(self.cumulative))
            .collect::<Vec<_>>();
        totals.sort_by(|left, right| compare_totals(left.as_ref(), right.as_ref()));
        totals
            .get(self.top.min(totals.len()).saturating_sub(1))
            .copied()
            .map(|total| total.map_or(LabelCutoff::Null, LabelCutoff::Value))
    }

    fn finish(
        mut self,
        spec: &ItemSpec,
        recorded: Option<(i64, i64)>,
        dictionary: &RenderedIds,
        budget: &mut WorkingBudget,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        self.reserve_finish(spec, dictionary, budget)?;
        let has_data = self.entities.values().any(|state| state.numeric);
        let coverage = HeatmapCoverage {
            state: if has_data {
                CoverageState::Data
            } else {
                CoverageState::NoData
            },
            recorded_from: recorded.map(|(from, _to)| from),
            recorded_to: recorded.map(|(_from, to)| to),
            nearest_row_before: self.scan.nearest_before,
            nearest_row_after: self.scan.nearest_after,
            window_rows: self.scan.window_rows,
        };
        let ranking = spec.query.ranking.clone();
        if self.grouped {
            return self.finish_grouped(spec, coverage, ranking, dictionary);
        }
        let mut ranked = Vec::new();
        for (_raw_key, state) in self.entities.drain() {
            if !state.numeric {
                continue;
            }
            if self.grid
                && let Some(finished) = state.current.cell(self.cumulative)
            {
                self.totals[state.column].add(finished);
            }
            let identity_values = render_cells(
                &state.identity,
                state.identity_segment,
                dictionary,
                self.first_index,
            )?;
            let mut key = String::new();
            entity_key_into(&mut key, state.type_id, &identity_values);
            ranked.push(RankedState {
                key,
                total: state.window.total(self.cumulative),
                identity_values,
                state,
            });
        }
        ranked.sort_by(|left, right| {
            compare_totals(left.total.as_ref(), right.total.as_ref())
                .then_with(|| left.key.cmp(&right.key))
        });
        let physical_entity_count = u64::try_from(ranked.len()).unwrap_or(u64::MAX);
        let totals_total =
            summary_total(ranked.iter().filter_map(|row| row.total), self.cumulative);
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

    fn reserve_finish(
        &self,
        spec: &ItemSpec,
        dictionary: &RenderedIds,
        budget: &mut WorkingBudget,
    ) -> Result<(), HeatmapError> {
        budget.reserve(spec.query.ranking.section.len(), self.first_index)?;
        reserve_strings_clone(&spec.query.ranking.fields, budget, self.first_index)?;
        if let HeatmapView::Grid { group, .. } = &spec.query.view {
            reserve_strings_clone(&spec.labels, budget, self.first_index)?;
            reserve_strings_clone(group, budget, self.first_index)?;
        }
        if self.grouped {
            return self.reserve_grouped_finish(dictionary, budget);
        }
        let numeric = self.entities.values().filter(|state| state.numeric).count();
        budget.reserve_array::<RankedState>(numeric, self.first_index)?;
        budget.reserve_array::<RankedState>(numeric, self.first_index)?;
        let top = spec.query.ranking.top.min(numeric);
        budget.reserve_array::<HeatmapEntity>(top, self.first_index)?;
        budget.reserve_array::<HeatmapEntity>(top, self.first_index)?;
        let label_cutoff = self.label_cutoff();
        for (raw_key, state) in &self.entities {
            if !state.numeric {
                continue;
            }
            budget.reserve_parts([raw_key.len(), raw_key.len()], self.first_index)?;
            budget.reserve_array::<Value>(state.identity.len(), self.first_index)?;
            let identity_names = contract(state.type_id)
                .map(|contract| contract.identity)
                .unwrap_or_default();
            for (position, stored) in state.identity.iter().enumerate() {
                let name_len = identity_names.get(position).map_or_else(
                    || format!("value_{position}").len(),
                    |name| public_identity_name(name).len(),
                );
                budget.reserve_parts(
                    [
                        size_of::<(String, Value)>(),
                        size_of::<(String, Value)>(),
                        name_len,
                        name_len,
                    ],
                    self.first_index,
                )?;
                reserve_rendered_clone(
                    stored,
                    state.identity_segment,
                    dictionary,
                    budget,
                    self.first_index,
                )?;
                reserve_rendered_clone(
                    stored,
                    state.identity_segment,
                    dictionary,
                    budget,
                    self.first_index,
                )?;
            }
            if label_cutoff.is_some_and(|cutoff| {
                ranking_reaches_cutoff(state.window.total(self.cumulative), cutoff)
            }) {
                for (name, label) in spec.labels.iter().zip(&state.labels) {
                    budget.reserve_parts(
                        [
                            size_of::<(String, Value)>(),
                            size_of::<(String, Value)>(),
                            name.len(),
                            name.len(),
                        ],
                        self.first_index,
                    )?;
                    if let Some(label) = label {
                        reserve_rendered_clone(
                            &label.value,
                            label.segment_slot,
                            dictionary,
                            budget,
                            self.first_index,
                        )?;
                    }
                }
            }
        }
        if self.grid {
            budget.reserve_array::<CellSum>(self.columns, self.first_index)?;
            budget.reserve_array::<HeatmapInterval>(self.columns, self.first_index)?;
            budget.reserve_matrix::<Option<f64>>(top, self.columns, self.first_index)?;
            budget.reserve_array::<Option<f64>>(self.columns, self.first_index)?;
            budget.reserve_array::<Option<f64>>(self.columns, self.first_index)?;
        }
        Ok(())
    }

    fn reserve_grouped_finish(
        &self,
        dictionary: &RenderedIds,
        budget: &mut WorkingBudget,
    ) -> Result<(), HeatmapError> {
        let group_totals = self.group_totals();
        let order = group_order(&group_totals);
        let top = self.top.min(order.len());
        budget.reserve_array::<Option<f64>>(self.groups.len(), self.first_index)?;
        budget.reserve_matrix::<CellSum>(self.groups.len(), self.columns, self.first_index)?;
        budget.reserve_array::<usize>(self.groups.len(), self.first_index)?;
        budget.reserve_array::<usize>(self.groups.len(), self.first_index)?;
        budget.reserve_array::<CellSum>(self.columns, self.first_index)?;
        budget.reserve_array::<HeatmapInterval>(self.columns, self.first_index)?;
        budget.reserve_array::<HeatmapGroup>(top, self.first_index)?;
        budget.reserve_matrix::<Option<f64>>(top, self.columns, self.first_index)?;
        budget.reserve_array::<Option<f64>>(self.columns, self.first_index)?;
        budget.reserve_array::<Option<f64>>(self.columns, self.first_index)?;
        for group in order.into_iter().take(top) {
            let group = &self.groups[group];
            budget.reserve_array::<Value>(group.values.len(), self.first_index)?;
            for stored in &group.values {
                reserve_rendered_clone(
                    stored,
                    group.segment_slot,
                    dictionary,
                    budget,
                    self.first_index,
                )?;
            }
        }
        Ok(())
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
        dictionary: &RenderedIds,
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

    fn finish_grouped(
        mut self,
        spec: &ItemSpec,
        coverage: HeatmapCoverage,
        ranking: super::query::NormalizedRanking,
        dictionary: &RenderedIds,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        let mut totals = std::mem::take(&mut self.totals);
        let ordered = self.ordered_numeric_states();
        let mut group_totals = vec![None; self.groups.len()];
        let mut group_cells = vec![vec![CellSum::default(); self.columns]; self.groups.len()];
        for (_key, state) in ordered {
            if let Some(finished) = state.current.cell(self.cumulative) {
                totals[state.column].add(finished);
            }
            let Some(group) = state.group else {
                continue;
            };
            if let Some(total) = state.window.total(self.cumulative) {
                group_totals[group] = Some(group_totals[group].unwrap_or(0.0) + total);
            }
            for (sum, observed) in group_cells[group].iter_mut().zip(&state.cells) {
                if let Some(value) = observed.cell(self.cumulative) {
                    sum.add(value);
                }
            }
        }
        let order = group_order(&group_totals);
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
                values: render_cells(
                    &state.values,
                    state.segment_slot,
                    dictionary,
                    self.first_index,
                )?,
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

fn compare_totals(left: Option<&f64>, right: Option<&f64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn group_order(totals: &[Option<f64>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..totals.len()).collect();
    order.sort_by(|left, right| {
        compare_totals(totals[*left].as_ref(), totals[*right].as_ref())
            .then_with(|| left.cmp(right))
    });
    order
}

fn ranking_reaches_cutoff(total: Option<f64>, cutoff: LabelCutoff) -> bool {
    match cutoff {
        LabelCutoff::Value(cutoff) if !cutoff.is_finite() => true,
        LabelCutoff::Value(cutoff) => {
            total.is_some_and(|total| !total.is_finite() || total >= cutoff)
        }
        LabelCutoff::Null => true,
    }
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
    dictionary: &RenderedIds,
    index: usize,
) -> Result<NamedValues, HeatmapError> {
    spec.labels
        .iter()
        .zip(labels)
        .map(|(name, stored)| {
            let value = stored.as_ref().map_or(Ok(Value::Null), |stored| {
                render_cell(&stored.value, stored.segment_slot, dictionary, index)
            })?;
            Ok((name.clone(), value))
        })
        .collect()
}

fn render_cells(
    cells: &[Cell],
    segment_slot: usize,
    dictionary: &RenderedIds,
    index: usize,
) -> Result<Vec<Value>, HeatmapError> {
    cells
        .iter()
        .map(|stored| render_cell(stored, segment_slot, dictionary, index))
        .collect()
}

fn render_cell(
    stored: &Cell,
    segment_slot: usize,
    dictionary: &RenderedIds,
    index: usize,
) -> Result<Value, HeatmapError> {
    if let Cell::StrId(id) = stored {
        return dictionary
            .get(&(segment_slot, *id))
            .cloned()
            .ok_or_else(|| HeatmapError::storage(index, format!("unresolved dictionary id {id}")));
    }
    cell(stored, &Dictionary::default()).map_err(|error| HeatmapError::storage(index, error))
}

fn reserve_rendered_clone(
    stored: &Cell,
    segment_slot: usize,
    dictionary: &RenderedIds,
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    budget.reserve_array::<Value>(1, index)?;
    if let Cell::StrId(id) = stored {
        if let Some(value) = dictionary.get(&(segment_slot, *id)) {
            let bytes = encoded_value_len(value, index)?;
            budget.reserve(bytes, index)?;
        }
    } else if let Cell::ListI32(values) = stored {
        budget.reserve_array::<Value>(values.len(), index)?;
        budget.reserve_array::<Value>(values.len(), index)?;
    }
    Ok(())
}

fn encoded_value_len(value: &Value, index: usize) -> Result<usize, HeatmapError> {
    let mut counter = ByteBudget::new(usize::MAX);
    serde_json::to_writer(&mut counter, value)
        .map_err(|error| HeatmapError::storage(index, error))?;
    Ok(usize::MAX - counter.remaining())
}

fn reserve_ids(
    cells: &[Cell],
    segment_slot: usize,
    retained: &mut [HashSet<u64>],
    retained_indices: &mut [Vec<(u64, usize)>],
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    for stored in cells {
        reserve_id(
            stored,
            segment_slot,
            retained,
            retained_indices,
            budget,
            index,
        )?;
    }
    Ok(())
}

fn cells_retained_bytes(cells: &[Cell], index: usize) -> Result<usize, HeatmapError> {
    let mut bytes = size_of::<Cell>().checked_mul(cells.len()).ok_or_else(|| {
        HeatmapError::invalid(index, "heatmap execution working set size overflows")
    })?;
    for stored in cells {
        bytes = bytes
            .checked_add(cell_nested_bytes(stored, index)?)
            .ok_or_else(|| {
                HeatmapError::invalid(index, "heatmap execution working set size overflows")
            })?;
    }
    Ok(bytes)
}

fn cell_nested_bytes(stored: &Cell, index: usize) -> Result<usize, HeatmapError> {
    match stored {
        Cell::ListI32(values) => size_of::<i32>().checked_mul(values.len()).ok_or_else(|| {
            HeatmapError::invalid(index, "heatmap execution working set size overflows")
        }),
        _ => Ok(0),
    }
}

fn reserve_id(
    stored: &Cell,
    segment_slot: usize,
    retained: &mut [HashSet<u64>],
    retained_indices: &mut [Vec<(u64, usize)>],
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    if let Cell::StrId(id) = stored
        && !retained[segment_slot].contains(id)
    {
        budget.reserve_parts([size_of::<u64>() * 4, size_of::<(u64, usize)>() * 2], index)?;
        retained[segment_slot].insert(*id);
        retained_indices[segment_slot].push((*id, index));
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

fn expand_results(
    unique: &[HeatmapItemResult],
    original_to_unique: &[usize],
    working: &mut WorkingBudget,
) -> Result<Vec<HeatmapItemResult>, HeatmapError> {
    check_result_budget(unique, original_to_unique)?;
    for (index, unique_index) in original_to_unique.iter().copied().enumerate() {
        reserve_result_clone(&unique[unique_index], working, index)?;
    }
    Ok(original_to_unique
        .iter()
        .map(|index| unique[*index].clone())
        .collect())
}

fn check_result_budget(
    unique: &[HeatmapItemResult],
    original_to_unique: &[usize],
) -> Result<(), HeatmapError> {
    let mut encoded = ByteBudget::new(RESULT_MAX_BYTES);
    std::io::Write::write_all(&mut encoded, b"{\"results\":[")
        .map_err(|_error| HeatmapError::invalid(0, result_overflow_message()))?;
    for (index, unique_index) in original_to_unique.iter().copied().enumerate() {
        if index > 0 {
            std::io::Write::write_all(&mut encoded, b",")
                .map_err(|_error| HeatmapError::invalid(index, result_overflow_message()))?;
        }
        let item = &unique[unique_index];
        serde_json::to_writer(&mut encoded, item)
            .map_err(|_error| HeatmapError::invalid(index, result_overflow_message()))?;
    }
    std::io::Write::write_all(&mut encoded, b"]}").map_err(|_error| {
        HeatmapError::invalid(
            original_to_unique.len().saturating_sub(1),
            result_overflow_message(),
        )
    })?;
    Ok(())
}

fn reserve_result_clone(
    item: &HeatmapItemResult,
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    budget.reserve_array::<HeatmapItemResult>(1, index)?;
    budget.reserve(item.ranking.section.len(), index)?;
    reserve_strings_clone(&item.ranking.fields, budget, index)?;
    budget.reserve_array::<HeatmapEntity>(item.entities.len(), index)?;
    for entity in &item.entities {
        reserve_named_values_clone(&entity.identity, budget, index)?;
        reserve_named_values_clone(&entity.labels, budget, index)?;
        if let Some(cells) = &entity.cells {
            budget.reserve_array::<Option<f64>>(cells.len(), index)?;
        }
    }
    if let Some(grid) = &item.grid {
        reserve_strings_clone(&grid.label_names, budget, index)?;
        reserve_strings_clone(&grid.group_names, budget, index)?;
        budget.reserve_array::<HeatmapInterval>(grid.intervals.len(), index)?;
        budget.reserve_array::<HeatmapGroup>(grid.groups.len(), index)?;
        for group in &grid.groups {
            budget.reserve_array::<Value>(group.values.len(), index)?;
            for value in &group.values {
                reserve_value_clone(value, budget, index)?;
            }
            budget.reserve_array::<Option<f64>>(group.cells.len(), index)?;
        }
        budget.reserve_array::<Option<f64>>(grid.totals.cells.len(), index)?;
        budget.reserve_array::<Option<f64>>(grid.others.cells.len(), index)?;
    }
    Ok(())
}

fn reserve_strings_clone(
    values: &[String],
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    budget.reserve_array::<String>(values.len(), index)?;
    for value in values {
        budget.reserve(value.len(), index)?;
    }
    Ok(())
}

fn reserve_named_values_clone(
    values: &NamedValues,
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    for (name, value) in values {
        reserve_map_entry_clone(name, value, budget, index)?;
    }
    Ok(())
}

fn reserve_map_entry_clone(
    name: &str,
    value: &Value,
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    budget.reserve_parts(
        [
            size_of::<(String, Value)>(),
            size_of::<(String, Value)>(),
            name.len(),
        ],
        index,
    )?;
    reserve_value_clone(value, budget, index)
}

fn reserve_value_clone(
    value: &Value,
    budget: &mut WorkingBudget,
    index: usize,
) -> Result<(), HeatmapError> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(value) => budget.reserve(value.len(), index),
        Value::Array(values) => {
            budget.reserve_array::<Value>(values.len(), index)?;
            for value in values {
                reserve_value_clone(value, budget, index)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (name, value) in values {
                reserve_map_entry_clone(name, value, budget, index)?;
            }
            Ok(())
        }
    }
}

fn result_overflow_message() -> String {
    format!(
        "heatmap result exceeds {RESULT_MAX_BYTES} encoded bytes; split rankings into several calls or reduce top"
    )
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

#[cfg(test)]
mod budget_tests {
    use std::collections::BTreeMap;
    use std::io::Write as _;

    use kronika_registry::ColumnClass;
    use serde_json::Value;

    use crate::api::heatmap::NormalizedRanking;
    use crate::budget::ByteBudget;

    use super::{
        CoverageState, HeatmapBatchResult, HeatmapCoverage, HeatmapEntity, HeatmapItemResult,
        RESULT_MAX_BYTES, WORKING_SET_MAX_BYTES, WorkingBudget, check_result_budget,
        expand_results, reserve_result_clone,
    };

    fn item(label: String) -> HeatmapItemResult {
        HeatmapItemResult {
            ranking: NormalizedRanking {
                section: "os_process".to_owned(),
                fields: vec!["utime".to_owned()],
                top: 1,
            },
            as_of: Some(1),
            coverage: HeatmapCoverage {
                state: CoverageState::Data,
                recorded_from: Some(1),
                recorded_to: Some(2),
                nearest_row_before: None,
                nearest_row_after: None,
                window_rows: 1,
            },
            class: ColumnClass::Cumulative,
            unit: None,
            entities: vec![HeatmapEntity {
                type_id: 1,
                identity: BTreeMap::new(),
                labels: BTreeMap::from([("comm".to_owned(), Value::String(label))]),
                total: Some(1.0),
                cells: None,
            }],
            totals_total: Some(1.0),
            others_total: None,
            entity_count: 1,
            out_of_order: 0,
            grid: None,
        }
    }

    #[test]
    fn working_set_accepts_the_exact_limit_and_indexes_the_crossing_byte() {
        let mut budget = WorkingBudget::default();
        budget
            .reserve(WORKING_SET_MAX_BYTES, 3)
            .expect("exact working limit");
        let error = budget.reserve(1, 7).expect_err("one byte over limit");
        assert_eq!(error.ranking_index(), 7);
    }

    #[test]
    fn working_set_arithmetic_overflow_keeps_the_original_usage_and_index() {
        let mut budget = WorkingBudget::default();
        let error = budget
            .reserve_array::<u64>(usize::MAX, 7)
            .expect_err("array byte count must overflow");
        assert_eq!(error.ranking_index(), 7);
        assert_eq!(budget.used, 0);
    }

    #[test]
    fn duplicate_results_reserve_each_deep_clone_at_its_request_index() {
        let item = item("x".repeat(1_024));
        let mut measured = WorkingBudget::default();
        reserve_result_clone(&item, &mut measured, 0).expect("measure one clone");
        let both = measured.used.checked_mul(2).expect("two clone sizes");

        let mut exact = WorkingBudget {
            used: WORKING_SET_MAX_BYTES - both,
        };
        let results = expand_results(std::slice::from_ref(&item), &[0, 0], &mut exact)
            .expect("exact remaining working bytes");
        assert_eq!(results.len(), 2);
        assert_eq!(exact.used, WORKING_SET_MAX_BYTES);

        let mut over = WorkingBudget {
            used: WORKING_SET_MAX_BYTES - both + 1,
        };
        let error = expand_results(std::slice::from_ref(&item), &[0, 0], &mut over)
            .expect_err("second clone must cross the working limit");
        assert_eq!(error.ranking_index(), 1);
        assert!(error.to_string().contains("working bytes"));
    }

    #[test]
    fn encoded_result_accepts_the_exact_limit_and_rejects_one_more_byte() {
        let mut budget = ByteBudget::new(RESULT_MAX_BYTES);
        let chunk = [0_u8; 1_024];
        for _ in 0..(RESULT_MAX_BYTES / chunk.len()) {
            budget.write_all(&chunk).expect("exact result limit");
        }
        assert_eq!(budget.remaining(), 0);
        assert!(budget.write_all(&[0]).is_err(), "one byte over must fail");
    }

    #[test]
    fn typed_result_accepts_exactly_eight_mib_and_rejects_one_more_byte() {
        let mut item = item(String::new());
        let base = serde_json::to_vec(&HeatmapBatchResult {
            results: vec![item.clone()],
        })
        .expect("measure fixed result envelope")
        .len();
        let label = item.entities[0].labels.get_mut("comm").expect("comm label");
        *label = Value::String("x".repeat(RESULT_MAX_BYTES - base));
        check_result_budget(std::slice::from_ref(&item), &[0]).expect("exact encoded result limit");

        let Value::String(label) = item.entities[0].labels.get_mut("comm").expect("comm label")
        else {
            panic!("comm label must be text");
        };
        label.push('x');
        let error = check_result_budget(std::slice::from_ref(&item), &[0])
            .expect_err("one encoded byte over must fail");
        assert_eq!(error.ranking_index(), 0);
    }
}
