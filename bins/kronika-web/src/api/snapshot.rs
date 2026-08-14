//! Reads one snapshot and derives counter rates.

mod relation;

use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
use std::cell::Cell as Counter;

use kronika_reader::{Cell, Dictionary, Reader, Resolved, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, ColumnType, contract, logical_section_name};
use serde_json::{Value, json};

use super::query::{Plan, plans, resolved_dictionary};
use super::render::{cell, projected_layout, record, shorten};
use super::{ApiError, CachePolicy, ResponseMeta, explicit_segment_with_listing};
use crate::route::{DataRequest, Order, RelationGroup, SegmentRequest, SnapshotRequest};
use crate::route::{SeriesRequest, Window};

pub(crate) struct PreparedSnapshot {
    segment: Segment,
    earlier: Option<Segment>,
    older: Option<Box<Segment>>,
    at: i64,
    sections: Vec<SectionPlans>,
    relation_filters: Vec<crate::route::Filter>,
    by: Vec<String>,
    direction: Order,
    group: Option<RelationGroup>,
    relation_fields: Vec<String>,
    page_size: Option<usize>,
    cursor: Option<SnapshotCursor>,
    binding: u64,
    search: Vec<GlobPattern>,
    text: Option<usize>,
    row_ordinal: Option<u64>,
}

type Readings = BTreeMap<Vec<IdentityCell>, CounterReadings>;
type CounterReadings = BTreeMap<&'static str, Cell>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotCursor {
    segment_id: i64,
    active_position: u64,
    context_index: usize,
    ordinal: u64,
    binding: u64,
}

struct PageRows {
    limit: usize,
    rows: BinaryHeap<Reverse<PageRankedRow>>,
}

struct PageRankedRow {
    staged: PageStagedRow,
    value: Option<PageOrderValue>,
    direction: Order,
}

struct PageStagedRow {
    context_index: usize,
    ordinal: u64,
    row: Row,
    identity: Vec<IdentityCell>,
}

enum PageOrderValue {
    Integer(i128),
    Float(f64),
    IntegerRate { delta: i128, elapsed: i64 },
    FloatRate(f64),
    IntegerRatio { numerator: u128, denominator: u128 },
    FloatRatio(f64),
    Text(Vec<u8>),
}

#[derive(Clone)]
struct PageOrder {
    name: &'static str,
    kind: PageOrderKind,
}

#[derive(Clone)]
enum PageOrderKind {
    Column(&'static str),
    CounterRatio {
        numerator: Vec<&'static str>,
        denominator: Vec<&'static str>,
        neutral_nulls: bool,
    },
    ValueRatio {
        numerator: Vec<&'static str>,
        denominator: Vec<&'static str>,
    },
}

enum RowWindow {
    Untimed,
    Shared {
        timestamp: &'static str,
        current: i64,
    },
    Partitioned {
        timestamp: &'static str,
        column: &'static str,
        current: BTreeMap<IdentityCell, i64>,
    },
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

struct PageContext<'a> {
    context_index: usize,
    plan: &'a Plan,
    source: &'a Segment,
    rows: u64,
    window: RowWindow,
    previous: Option<Arc<Readings>>,
    elapsed: Option<i64>,
    elapsed_by_partition: BTreeMap<IdentityCell, i64>,
    sample_from: Option<i64>,
    sample_to: Option<i64>,
    order: Option<PageOrder>,
    search_columns: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SampleSource {
    Older,
    Earlier,
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct LocatedMoment {
    at: i64,
    source: SampleSource,
}

#[derive(Clone, Copy)]
struct PartitionMoments {
    current: LocatedMoment,
    previous: Option<LocatedMoment>,
}

struct SelectedPartition {
    layout_index: usize,
    type_id: u32,
    moments: PartitionMoments,
}

#[derive(Clone, Copy)]
struct SnapshotViewSpec {
    temporal_partition: &'static str,
}

impl SnapshotViewSpec {
    fn for_logical_name(logical_name: &str) -> Option<Self> {
        match logical_name {
            "pg_stat_user_tables" | "pg_stat_user_indexes" => Some(Self {
                temporal_partition: "datid",
            }),
            _ => None,
        }
    }
}

impl RowWindow {
    fn matches(&self, row: &Row) -> bool {
        match self {
            Self::Untimed => true,
            Self::Shared { timestamp, current } => row_timestamp(row, timestamp) == Some(*current),
            Self::Partitioned {
                timestamp,
                column,
                current,
            } => row.get(column).is_some_and(|partition| {
                current
                    .get(&identity_cell(partition))
                    .is_some_and(|at| row_timestamp(row, timestamp) == Some(*at))
            }),
        }
    }
}

impl PageContext<'_> {
    fn elapsed_for(&self, row: &Row) -> Option<i64> {
        match &self.window {
            RowWindow::Partitioned { column, .. } => row
                .get(column)
                .and_then(|partition| self.elapsed_by_partition.get(&identity_cell(partition)))
                .copied(),
            RowWindow::Untimed | RowWindow::Shared { .. } => self.elapsed,
        }
    }

    fn predecessor<'a>(
        &'a self,
        row: &Row,
        identity: &[IdentityCell],
    ) -> Option<&'a CounterReadings> {
        let before = self.previous.as_ref()?.get(identity)?;
        if let Some(column) = self.plan.contract.column("stats_since") {
            let current = row.get(column.name)?;
            let previous = before.get(column.name)?;
            if current != previous {
                return None;
            }
        }
        Some(before)
    }
}

struct PageMetadata {
    eligible: u64,
    returned: usize,
    has_more: bool,
    next_cursor: Option<String>,
    page_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobPattern(Vec<GlobToken>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    Star,
    Any,
    Literal(char),
}

const SNAPSHOT_CHUNK_ROWS: usize = 16;

#[cfg(test)]
thread_local! {
    static PAGE_CHUNK_ROWS: Counter<usize> = const { Counter::new(0) };
    static PAGE_SOURCE_VISITS: Counter<usize> = const { Counter::new(0) };
    static PAGE_CANDIDATE_DICTIONARIES: Counter<usize> = const { Counter::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_page_operations() {
    PAGE_CHUNK_ROWS.set(0);
    PAGE_SOURCE_VISITS.set(0);
    PAGE_CANDIDATE_DICTIONARIES.set(0);
}

#[cfg(test)]
pub(crate) fn page_operations() -> (usize, usize, usize) {
    (
        PAGE_SOURCE_VISITS.get(),
        PAGE_CANDIDATE_DICTIONARIES.get(),
        PAGE_CHUNK_ROWS.get(),
    )
}

pub(super) fn stream_relation_history(
    reader: &Reader,
    listed: &[SegmentRef],
    window: Window,
    request: &SeriesRequest,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    relation::stream_history(reader, listed, window, request, emit, cancelled)
}

#[cfg(test)]
pub(crate) use relation::{history_operations, reset_history_operations};

pub(super) fn prepare(root: &Path, request: SnapshotRequest) -> Result<PreparedSnapshot, ApiError> {
    let binding = snapshot_binding(&request);
    let parsed = request
        .cursor
        .as_deref()
        .map(SnapshotCursor::parse)
        .transpose()?
        .filter(|cursor| cursor.segment_id == request.segment_id && cursor.binding == binding);
    if request.cursor.is_some() && parsed.is_none() {
        return Err(ApiError::BadCursor);
    }
    let (reader, current, segments) = explicit_segment_with_listing(root, request.segment_id)?;
    let segment_ref = pin(current, parsed)?;
    let segment = reader.open_segment(&segment_ref)?;
    let active_position = segment.active_position().unwrap_or(0);
    if parsed.is_some_and(|cursor| cursor.active_position != active_position) {
        return Err(ApiError::BadCursor);
    }
    let (earlier, older) = preceding(
        &reader,
        &segment_ref,
        segments,
        &request.sections,
        request.at,
    )?;
    let relation_fields = request
        .group
        .map(|group| relation::output_fields(&request.sections, group, &request.fields))
        .transpose()?
        .unwrap_or_default();
    let (physical_filters, relation_filters) = relation::split_filters(&request)?;
    let mut physical_request = request.clone();
    physical_request.filters = physical_filters;
    let sections = section_plans(&segment, &physical_request)?;
    validate_search_projection(&request, &sections)?;
    validate_exact_locator(&segment, &request, &sections)?;
    Ok(PreparedSnapshot {
        segment,
        earlier,
        older: older.map(Box::new),
        at: request.at,
        sections,
        relation_filters,
        by: request.by,
        direction: request.direction,
        group: request.group,
        relation_fields,
        page_size: request.page_size,
        cursor: parsed,
        binding,
        search: request
            .search
            .iter()
            .map(|raw| GlobPattern::new(raw))
            .collect(),
        text: request.text,
        row_ordinal: request.row_ordinal,
    })
}

fn section_plans(
    segment: &Segment,
    request: &SnapshotRequest,
) -> Result<Vec<SectionPlans>, ApiError> {
    let shared_projection = request.sections.len() > 1 && !request.fields.is_empty();
    if shared_projection {
        validate_shared_projection(segment, &request.sections, &request.fields)?;
    }
    let mut sections = Vec::with_capacity(request.sections.len());
    for logical_name in &request.sections {
        let fields = if request.group.is_some() {
            // Relation reducers need the complete union of columns carried by
            // the layouts actually present in this segment. An empty generic
            // projection asks `plans` for exactly that union and keeps older
            // layouts honest when newer PostgreSQL fields do not exist.
            Vec::new()
        } else if shared_projection {
            section_projection(segment, logical_name, &request.fields)
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
        match plans(segment, &data, true) {
            Ok(mut plans) => {
                for plan in &mut plans {
                    let order = page_order(logical_name, plan, &request.by);
                    plan.add_projection_columns(
                        &order.as_ref().map_or_else(Vec::new, PageOrder::columns),
                    );
                    if plan.contract.column("stats_since").is_some() {
                        plan.add_projection_columns(&["stats_since"]);
                    }
                    if !request.search.is_empty() {
                        plan.add_projection_columns(&search_columns(logical_name, plan));
                    }
                }
                sections.push(SectionPlans {
                    logical_name: logical_name.clone(),
                    plans,
                });
            }
            Err(ApiError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(sections)
}

impl PageOrder {
    fn columns(&self) -> Vec<&'static str> {
        match &self.kind {
            PageOrderKind::Column(column) => vec![*column],
            PageOrderKind::CounterRatio {
                numerator,
                denominator,
                ..
            }
            | PageOrderKind::ValueRatio {
                numerator,
                denominator,
                ..
            } => numerator.iter().chain(denominator).copied().collect(),
        }
    }

    fn dictionary_column(&self, plan: &Plan) -> Option<&'static str> {
        match &self.kind {
            PageOrderKind::Column(column)
                if plan
                    .contract
                    .column(column)
                    .is_some_and(|column| column.ty == ColumnType::StrId) =>
            {
                Some(*column)
            }
            PageOrderKind::Column(_)
            | PageOrderKind::CounterRatio { .. }
            | PageOrderKind::ValueRatio { .. } => None,
        }
    }
}

fn page_order(logical_name: &str, plan: &Plan, requested: &[String]) -> Option<PageOrder> {
    requested.iter().find_map(|name| {
        plan.contract
            .column(name)
            .map(|column| PageOrder {
                name: column.name,
                kind: PageOrderKind::Column(column.name),
            })
            .or_else(|| derived_page_order(logical_name, plan, name))
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "all fixed derived sort tokens remain visibly allowlisted together"
)]
fn derived_page_order(logical_name: &str, plan: &Plan, token: &str) -> Option<PageOrder> {
    let supported = match logical_name {
        "pg_stat_statements" => matches!(plan.type_id, 1_002_001..=1_002_006),
        "pg_store_plans" => matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001),
        "pg_stat_user_tables" => matches!(plan.type_id, 1_013_001..=1_013_004),
        "pg_stat_user_indexes" => matches!(plan.type_id, 1_014_001..=1_014_002),
        _ => false,
    };
    if !supported {
        return None;
    }
    let column = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| plan.contract.column(name).map(|column| column.name))
    };
    let columns = |names: &[&str]| {
        names
            .iter()
            .filter_map(|name| column(&[*name]))
            .collect::<Vec<_>>()
    };
    let counters =
        |name, numerator: Vec<&'static str>, denominator: Vec<&'static str>, neutral_nulls| {
            (!numerator.is_empty() && !denominator.is_empty()).then_some(PageOrder {
                name,
                kind: PageOrderKind::CounterRatio {
                    numerator,
                    denominator,
                    neutral_nulls,
                },
            })
        };
    let one_over = |name, numerator: &[&str], denominator: &[&str]| {
        counters(
            name,
            vec![column(numerator)?],
            vec![column(denominator)?],
            false,
        )
    };
    let gauges = |name, numerator: &[&str], denominator: &[&str]| {
        let numerator = columns(numerator);
        let denominator = columns(denominator);
        (!numerator.is_empty() && !denominator.is_empty()).then_some(PageOrder {
            name,
            kind: PageOrderKind::ValueRatio {
                numerator,
                denominator,
            },
        })
    };
    match token {
        "derived.mean_exec_ms_per_call" => one_over(
            "mean_exec_ms_per_call",
            &["total_exec_time", "total_time"],
            &["calls"],
        ),
        "derived.rows_per_call" => one_over("rows_per_call", &["rows"], &["calls"]),
        "derived.blocks_per_call" => counters(
            "blocks_per_call",
            [
                "shared_blks_hit",
                "shared_blks_read",
                "local_blks_hit",
                "local_blks_read",
            ]
            .iter()
            .filter_map(|name| column(&[*name]))
            .collect(),
            vec![column(&["calls"])?],
            false,
        ),
        "derived.hit_pct" => {
            let hit = column(&["shared_blks_hit"])?;
            let read = column(&["shared_blks_read"])?;
            counters("hit_pct", vec![hit], vec![hit, read], false)
        }
        "derived.wal_per_call" => one_over("wal_per_call", &["wal_bytes"], &["calls"]),
        "derived.plan_time_pct" => {
            let planning = column(&["total_plan_time"])?;
            let execution = column(&["total_exec_time", "total_time"])?;
            counters(
                "plan_time_pct",
                vec![planning],
                vec![planning, execution],
                false,
            )
        }
        "derived.cv" => Some(PageOrder {
            name: "cv",
            kind: PageOrderKind::ValueRatio {
                numerator: vec![column(&["stddev_exec_time", "stddev_time"])?],
                denominator: vec![column(&["mean_exec_time", "mean_time"])?],
            },
        }),
        "derived.dead_pct" => gauges("dead_pct", &["n_dead_tup"], &["n_live_tup", "n_dead_tup"]),
        "derived.hot_pct" => one_over("hot_pct", &["n_tup_hot_upd"], &["n_tup_upd"]),
        "derived.new_page_pct" => one_over("new_page_pct", &["n_tup_newpage_upd"], &["n_tup_upd"]),
        "derived.sequential_share_pct" => counters(
            "sequential_share_pct",
            columns(&["seq_scan"]),
            columns(&["seq_scan", "idx_scan"]),
            true,
        ),
        "derived.seq_tuples_per_scan" => {
            one_over("seq_tuples_per_scan", &["seq_tup_read"], &["seq_scan"])
        }
        "derived.idx_tuples_per_scan" => {
            one_over("idx_tuples_per_scan", &["idx_tup_fetch"], &["idx_scan"])
        }
        "derived.buffer_hit_pct" => counters(
            "buffer_hit_pct",
            columns(&[
                "heap_blks_hit",
                "idx_blks_hit",
                "toast_blks_hit",
                "tidx_blks_hit",
            ]),
            columns(&[
                "heap_blks_hit",
                "heap_blks_read",
                "idx_blks_hit",
                "idx_blks_read",
                "toast_blks_hit",
                "toast_blks_read",
                "tidx_blks_hit",
                "tidx_blks_read",
            ]),
            true,
        ),
        "derived.vacuum_mean_ms" => {
            one_over("vacuum_mean_ms", &["total_vacuum_time"], &["vacuum_count"])
        }
        "derived.autovacuum_mean_ms" => one_over(
            "autovacuum_mean_ms",
            &["total_autovacuum_time"],
            &["autovacuum_count"],
        ),
        "derived.analyze_mean_ms" => one_over(
            "analyze_mean_ms",
            &["total_analyze_time"],
            &["analyze_count"],
        ),
        "derived.autoanalyze_mean_ms" => one_over(
            "autoanalyze_mean_ms",
            &["total_autoanalyze_time"],
            &["autoanalyze_count"],
        ),
        "derived.tuples_per_scan" => one_over("tuples_per_scan", &["idx_tup_read"], &["idx_scan"]),
        "derived.fetches_per_scan" => {
            one_over("fetches_per_scan", &["idx_tup_fetch"], &["idx_scan"])
        }
        _ => None,
    }
}

fn validate_search_projection(
    request: &SnapshotRequest,
    sections: &[SectionPlans],
) -> Result<(), ApiError> {
    if request.page_size.is_some() {
        let [section] = sections else {
            return Err(ApiError::BadCursor);
        };
        if !request.search.is_empty()
            && !section
                .plans
                .iter()
                .any(|plan| !search_columns(&section.logical_name, plan).is_empty())
        {
            return Err(ApiError::BadFilter("search".to_owned()));
        }
    }
    Ok(())
}

fn validate_exact_locator(
    segment: &Segment,
    request: &SnapshotRequest,
    sections: &[SectionPlans],
) -> Result<(), ApiError> {
    if let Some(ordinal) = request.row_ordinal {
        let [section] = sections else {
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
    Ok(())
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
        if self.page_size.is_some() {
            if self.group.is_some() {
                return self.emit_relation_page(emit, cancelled);
            }
            return self.emit_page(emit, cancelled);
        }
        for section in &self.sections {
            if SnapshotViewSpec::for_logical_name(&section.logical_name).is_some() {
                if !self.emit_partitioned_section(section, emit, cancelled)? {
                    return Ok(());
                }
            } else {
                for (layout_index, plan) in section.plans.iter().enumerate() {
                    if cancelled() {
                        return Ok(());
                    }
                    if !self.emit_section(section, layout_index, plan, emit, cancelled)? {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    fn emit_partitioned_section(
        &self,
        section: &SectionPlans,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(false);
            }
        }
        let contexts = self.partitioned_contexts(section, cancelled)?;
        for context in &contexts {
            if self.row_ordinal.is_some() && context.source.id() != self.segment.id() {
                continue;
            }
            if cancelled() || !self.emit_context_rows(context, emit, cancelled)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_context_rows(
        &self,
        context: &PageContext<'_>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let mut rows = Vec::new();
        context.source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                if context.window.matches(&row) {
                    rows.push((ordinal, row));
                }
                true
            },
        )?;
        if cancelled() {
            return Ok(false);
        }
        let selection_dictionary = context.plan.selection_dictionary(context.source, &rows)?;
        let mut staged = Vec::with_capacity(rows.len());
        for (ordinal, row) in rows {
            if !context.plan.matches(&row, &selection_dictionary) {
                continue;
            }
            let Some(identity) = identity_of(context.plan, &row) else {
                continue;
            };
            staged.push(StagedRow {
                ordinal,
                row,
                identity,
            });
        }
        let dictionary = retained_dictionary(context.source, &staged)?;
        for staged in staged {
            let before = context.predecessor(&staged.row, &staged.identity);
            let value = Self::row_record(
                context.plan,
                &staged.row,
                before,
                context.elapsed_for(&staged.row),
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

    fn emit_section(
        &self,
        section: &SectionPlans,
        layout_index: usize,
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
        for context in self.timed_contexts(section, layout_index, plan, timestamp, cancelled)? {
            if self.row_ordinal.is_some() && context.source.id() != self.segment.id() {
                continue;
            }
            if cancelled() || !self.emit_context_rows(&context, emit, cancelled)? {
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
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let rates = RateContext {
            previous: None,
            elapsed: None,
        };
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
        self.emit_rows(&self.segment, plan, rows, rates, emit, cancelled)
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
        self.emit_staged_rows(source, plan, staged, rates, emit, cancelled)
    }

    fn emit_staged_rows(
        &self,
        source: &Segment,
        plan: &Plan,
        staged: Vec<StagedRow>,
        rates: RateContext<'_>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
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

    fn emit_page(
        &self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let [section] = self.sections.as_slice() else {
            return Err(ApiError::BadCursor);
        };
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(());
            }
        }
        let contexts = self.page_contexts(section, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let anchor = self
            .cursor
            .map(|cursor| self.cursor_anchor(&contexts, cursor))
            .transpose()?;
        let page_size = self.page_size.ok_or(ApiError::BadCursor)?;
        let mut page = PageRows::new(page_size.saturating_add(1));
        let mut eligible = 0_u64;
        for context in &contexts {
            self.scan_page(
                context,
                anchor.as_ref(),
                &mut page,
                &mut eligible,
                cancelled,
            )?;
            if cancelled() {
                return Ok(());
            }
        }
        let mut ranked = page.finish();
        let has_more = ranked.len() > page_size;
        let next_cursor = has_more.then(|| {
            let row = &ranked[page_size].staged;
            SnapshotCursor {
                segment_id: self.segment.id(),
                active_position: self.segment.active_position().unwrap_or(0),
                context_index: row.context_index,
                ordinal: row.ordinal,
                binding: self.binding,
            }
            .encode()
        });
        ranked.truncate(page_size);
        let returned = ranked.len();
        if !self.emit_page_rows(&contexts, ranked, emit, cancelled)? {
            return Ok(());
        }
        Self::emit_page_trailer(
            section,
            &contexts,
            &PageMetadata {
                eligible,
                returned,
                has_more,
                next_cursor,
                page_size,
            },
            self.direction,
            emit,
        )?;
        Ok(())
    }

    fn emit_page_rows(
        &self,
        contexts: &[PageContext<'_>],
        ranked: Vec<PageRankedRow>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let mut ids_by_context: HashMap<usize, HashSet<u64>> = HashMap::new();
        for ranked in &ranked {
            for (_name, value) in ranked.staged.row.iter() {
                if let Cell::StrId(id) = value {
                    ids_by_context
                        .entry(ranked.staged.context_index)
                        .or_default()
                        .insert(*id);
                }
            }
        }
        let dictionaries = contexts
            .iter()
            .map(|context| {
                let ids = ids_by_context
                    .get(&context.context_index)
                    .cloned()
                    .unwrap_or_default();
                resolved_dictionary(context.source, &ids)
                    .map(|dictionary| (context.context_index, dictionary))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        for ranked in ranked {
            let context = contexts
                .iter()
                .find(|context| context.context_index == ranked.staged.context_index)
                .ok_or(ApiError::BadCursor)?;
            let dictionary = dictionaries
                .get(&context.context_index)
                .ok_or(ApiError::BadCursor)?;
            let before = context.predecessor(&ranked.staged.row, &ranked.staged.identity);
            let value = Self::row_record(
                context.plan,
                &ranked.staged.row,
                before,
                context.elapsed_for(&ranked.staged.row),
                ranked.staged.ordinal,
                dictionary,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_page_trailer(
        section: &SectionPlans,
        contexts: &[PageContext<'_>],
        metadata: &PageMetadata,
        direction: Order,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
    ) -> Result<(), ApiError> {
        let mut order_by = Vec::new();
        for context in contexts {
            if let Some(order) = &context.order
                && !order_by.contains(&order.name)
            {
                order_by.push(order.name);
            }
        }
        let from = contexts
            .iter()
            .filter_map(|context| context.sample_from)
            .min();
        let to = contexts
            .iter()
            .filter_map(|context| context.sample_to)
            .max();
        let _connected = emit(record(json!({
            "record": "snapshot_page",
            "logical_name": section.logical_name,
            "eligible": metadata.eligible.to_string(),
            "returned": metadata.returned.to_string(),
            "has_more": metadata.has_more,
            "truncated": metadata.eligible > metadata.returned as u64,
            "next_cursor": metadata.next_cursor,
            "page_size": metadata.page_size,
            "order_by": order_by,
            "order_direction": match direction {
                Order::Asc => "asc",
                Order::Desc => "desc",
            },
            "from": from.map(|value| value.to_string()),
            "to": to.map(|value| value.to_string()),
        }))?);
        Ok(())
    }

    fn page_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<PageContext<'a>>, ApiError> {
        if SnapshotViewSpec::for_logical_name(&section.logical_name).is_some() {
            return self.partitioned_contexts(section, cancelled);
        }
        let mut contexts = Vec::with_capacity(section.plans.len());
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                contexts.push(PageContext {
                    context_index: layout_index,
                    plan,
                    source: &self.segment,
                    rows: self.segment.rows_of(plan.type_id).unwrap_or(0),
                    window: RowWindow::Untimed,
                    previous: None,
                    elapsed: None,
                    elapsed_by_partition: BTreeMap::new(),
                    sample_from: None,
                    sample_to: None,
                    order: page_order(&section.logical_name, plan, &self.by),
                    search_columns: if self.search.is_empty() {
                        Vec::new()
                    } else {
                        search_columns(&section.logical_name, plan)
                    },
                });
                continue;
            };
            contexts.extend(self.timed_contexts(
                section,
                layout_index,
                plan,
                timestamp,
                cancelled,
            )?);
        }
        Ok(contexts)
    }

    fn timed_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        layout_index: usize,
        plan: &'a Plan,
        timestamp: &'static str,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<PageContext<'a>>, ApiError> {
        let Some(moments) = self.shared_moments(plan, timestamp, cancelled)? else {
            return Ok(Vec::new());
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut previous = Readings::new();
        if let Some(before) = moments.previous {
            for source_kind in [
                SampleSource::Older,
                SampleSource::Earlier,
                SampleSource::Current,
            ] {
                let Some(source) = self.source_for(source_kind) else {
                    continue;
                };
                if source.rows_of(plan.type_id).is_some() {
                    previous.extend(Self::collect(
                        source,
                        plan,
                        timestamp,
                        before,
                        &order_columns,
                        cancelled,
                    )?);
                }
            }
        }
        let previous = Arc::new(previous);
        let elapsed = moments
            .previous
            .and_then(|before| moments.current.checked_sub(before))
            .filter(|elapsed| *elapsed > 0);
        let mut contexts = Vec::new();
        for source_kind in [
            SampleSource::Current,
            SampleSource::Earlier,
            SampleSource::Older,
        ] {
            let Some(source) = self.source_for(source_kind) else {
                continue;
            };
            if source.rows_of(plan.type_id).is_none()
                || Self::moments(source, plan, timestamp, moments.current, cancelled)?
                    .is_none_or(|source_moments| source_moments.current != moments.current)
            {
                continue;
            }
            contexts.push(PageContext {
                context_index: timed_context_index(layout_index, source_kind),
                plan,
                source,
                rows: source.rows_of(plan.type_id).unwrap_or(0),
                window: RowWindow::Shared {
                    timestamp,
                    current: moments.current,
                },
                previous: Some(Arc::clone(&previous)),
                elapsed,
                elapsed_by_partition: BTreeMap::new(),
                sample_from: moments.previous,
                sample_to: Some(moments.current),
                order: order.clone(),
                search_columns: if self.search.is_empty() {
                    Vec::new()
                } else {
                    search_columns(&section.logical_name, plan)
                },
            });
        }
        Ok(contexts)
    }

    fn shared_moments(
        &self,
        plan: &Plan,
        timestamp: &'static str,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Moments>, ApiError> {
        let here = Self::moments(&self.segment, plan, timestamp, self.at, cancelled)?;
        let current = if let Some(here) = here {
            Some(here.current)
        } else {
            let mut fallback = None;
            for source_kind in [SampleSource::Earlier, SampleSource::Older] {
                let Some(source) = self.source_for(source_kind) else {
                    continue;
                };
                if source.rows_of(plan.type_id).is_none() {
                    continue;
                }
                if let Some(moments) = Self::moments(source, plan, timestamp, self.at, cancelled)? {
                    fallback = Some(
                        fallback.map_or(moments.current, |chosen: i64| chosen.max(moments.current)),
                    );
                }
            }
            fallback
        };
        let Some(current) = current else {
            return Ok(None);
        };
        let mut previous = None;
        if let Some(before) = current.checked_sub(1) {
            for source_kind in [
                SampleSource::Current,
                SampleSource::Earlier,
                SampleSource::Older,
            ] {
                let Some(source) = self.source_for(source_kind) else {
                    continue;
                };
                if source.rows_of(plan.type_id).is_none() {
                    continue;
                }
                if let Some(moments) = Self::moments(source, plan, timestamp, before, cancelled)? {
                    previous = Some(
                        previous.map_or(moments.current, |chosen: i64| chosen.max(moments.current)),
                    );
                }
            }
        }
        Ok(Some(Moments { current, previous }))
    }

    fn partitioned_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<PageContext<'a>>, ApiError> {
        let Some(spec) = SnapshotViewSpec::for_logical_name(&section.logical_name) else {
            return Ok(Vec::new());
        };
        let selected = self.selected_partitions(section, spec, cancelled)?;
        if cancelled() {
            return Ok(Vec::new());
        }
        let mut contexts = Vec::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            for source in [
                SampleSource::Current,
                SampleSource::Earlier,
                SampleSource::Older,
            ] {
                if let Some(context) = self.partitioned_context(
                    section,
                    layout_index,
                    plan,
                    source,
                    &selected,
                    spec,
                    cancelled,
                )? {
                    contexts.push(context);
                }
            }
        }
        Ok(contexts)
    }

    fn selected_partitions(
        &self,
        section: &SectionPlans,
        spec: SnapshotViewSpec,
        cancelled: &impl Fn() -> bool,
    ) -> Result<BTreeMap<IdentityCell, SelectedPartition>, ApiError> {
        let mut selected = BTreeMap::<IdentityCell, SelectedPartition>::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                continue;
            };
            let mut moments = BTreeMap::new();
            Self::partition_moments(
                &self.segment,
                plan,
                timestamp,
                spec.temporal_partition,
                self.at,
                SampleSource::Current,
                &mut moments,
                cancelled,
            )?;
            if let Some(earlier) = self.earlier.as_ref() {
                Self::partition_moments(
                    earlier,
                    plan,
                    timestamp,
                    spec.temporal_partition,
                    self.at,
                    SampleSource::Earlier,
                    &mut moments,
                    cancelled,
                )?;
            }
            if let Some(older) = self.older.as_deref() {
                Self::partition_moments(
                    older,
                    plan,
                    timestamp,
                    spec.temporal_partition,
                    self.at,
                    SampleSource::Older,
                    &mut moments,
                    cancelled,
                )?;
            }
            for (partition, moments) in moments {
                let candidate = SelectedPartition {
                    layout_index,
                    type_id: plan.type_id,
                    moments,
                };
                let replace = selected.get(&partition).is_none_or(|chosen| {
                    (candidate.moments.current, candidate.type_id)
                        > (chosen.moments.current, chosen.type_id)
                });
                if replace {
                    selected.insert(partition, candidate);
                }
            }
        }
        Ok(selected)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the selected layout, source, and partition contract define one context"
    )]
    fn partitioned_context<'a>(
        &'a self,
        section: &SectionPlans,
        layout_index: usize,
        plan: &'a Plan,
        source_kind: SampleSource,
        selected: &BTreeMap<IdentityCell, SelectedPartition>,
        spec: SnapshotViewSpec,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<PageContext<'a>>, ApiError> {
        let Some(timestamp) = plan.timestamp else {
            return Ok(None);
        };
        if !selected.values().any(|selection| {
            selection.layout_index == layout_index
                && selection.moments.current.source == source_kind
        }) {
            return Ok(None);
        }
        let Some(source) = self.source_for(source_kind) else {
            return Ok(None);
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut current = BTreeMap::new();
        let mut previous = Readings::new();
        let mut elapsed_by_partition = BTreeMap::new();
        let mut sample_from: Option<i64> = None;
        let mut sample_to: Option<i64> = None;
        for (partition, selection) in selected {
            if selection.layout_index != layout_index
                || selection.moments.current.source != source_kind
            {
                continue;
            }
            current.insert(partition.clone(), selection.moments.current.at);
            sample_to = Some(sample_to.map_or(selection.moments.current.at, |chosen| {
                chosen.max(selection.moments.current.at)
            }));
            let Some(before) = selection.moments.previous else {
                continue;
            };
            let Some(elapsed) = selection
                .moments
                .current
                .at
                .checked_sub(before.at)
                .filter(|elapsed| *elapsed > 0)
            else {
                continue;
            };
            let Some(before_source) = self.source_for(before.source) else {
                continue;
            };
            previous.extend(Self::collect_partition(
                before_source,
                plan,
                timestamp,
                before.at,
                spec.temporal_partition,
                partition,
                &order_columns,
                cancelled,
            )?);
            elapsed_by_partition.insert(partition.clone(), elapsed);
            sample_from = Some(sample_from.map_or(before.at, |chosen| chosen.min(before.at)));
        }
        Ok(Some(PageContext {
            context_index: partition_context_index(layout_index, source_kind),
            plan,
            source,
            rows: source.rows_of(plan.type_id).unwrap_or(0),
            window: RowWindow::Partitioned {
                timestamp,
                column: spec.temporal_partition,
                current,
            },
            previous: Some(previous.into()),
            elapsed: None,
            elapsed_by_partition,
            sample_from,
            sample_to,
            order,
            search_columns: if self.search.is_empty() {
                Vec::new()
            } else {
                search_columns(&section.logical_name, plan)
            },
        }))
    }

    fn source_for(&self, source: SampleSource) -> Option<&Segment> {
        match source {
            SampleSource::Current => Some(&self.segment),
            SampleSource::Earlier => self.earlier.as_ref(),
            SampleSource::Older => self.older.as_deref(),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the scan coordinates are explicit to prevent cross-database samples"
    )]
    fn partition_moments(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        partition_column: &'static str,
        at: i64,
        source: SampleSource,
        moments: &mut BTreeMap<IdentityCell, PartitionMoments>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        if segment.rows_of(plan.type_id).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            plan.type_id,
            &[timestamp, partition_column],
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                let (Some(stored), Some(partition)) =
                    (row_timestamp(&row, timestamp), row.get(partition_column))
                else {
                    return true;
                };
                if stored <= at {
                    record_partition_moment(
                        moments,
                        identity_cell(partition),
                        LocatedMoment { at: stored, source },
                    );
                }
                true
            },
        )?;
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the exact timestamp and partition keep database predecessors isolated"
    )]
    fn collect_partition(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        at: i64,
        partition_column: &'static str,
        partition: &IdentityCell,
        extra_columns: &[&'static str],
        cancelled: &impl Fn() -> bool,
    ) -> Result<Readings, ApiError> {
        let mut collected = BTreeMap::new();
        let mut counters = rate_columns(plan);
        if plan.contract.column("stats_since").is_some() {
            counters.push("stats_since");
        }
        for column in extra_columns {
            if plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
                && !counters.contains(column)
            {
                counters.push(column);
            }
        }
        if counters.is_empty() {
            return Ok(collected);
        }
        let mut projection = counters.clone();
        projection.extend(plan.contract.identity.iter().copied());
        projection.extend([timestamp, partition_column]);
        projection.sort_unstable();
        projection.dedup();
        segment.visit_rows(plan.type_id, &projection, 0, usize::MAX, |_ordinal, row| {
            if cancelled() {
                return false;
            }
            if row_timestamp(&row, timestamp) != Some(at)
                || row
                    .get(partition_column)
                    .is_none_or(|stored| &identity_cell(stored) != partition)
            {
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
        })?;
        Ok(collected)
    }

    fn cursor_anchor(
        &self,
        contexts: &[PageContext<'_>],
        cursor: SnapshotCursor,
    ) -> Result<PageRankedRow, ApiError> {
        let context = contexts
            .iter()
            .find(|context| context.context_index == cursor.context_index)
            .ok_or(ApiError::BadCursor)?;
        if cursor.ordinal >= context.rows {
            return Err(ApiError::BadCursor);
        }
        let mut stored = None;
        context.source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            cursor.ordinal,
            1,
            |ordinal, row| {
                stored = Some((ordinal, row));
                false
            },
        )?;
        let (ordinal, row) = stored.ok_or(ApiError::BadCursor)?;
        let dictionary = page_dictionary(context, std::slice::from_ref(&(ordinal, row.clone())))?;
        self.page_candidate(context, ordinal, row, &dictionary)
            .ok_or(ApiError::BadCursor)
    }

    fn scan_page(
        &self,
        context: &PageContext<'_>,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        if !context.plan.needs_selection_dictionary()
            && context.search_columns.is_empty()
            && context
                .order
                .as_ref()
                .and_then(|order| order.dictionary_column(context.plan))
                .is_none()
        {
            #[cfg(test)]
            PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
            let dictionary = Dictionary::default();
            context.source.visit_rows(
                context.plan.type_id,
                &context.plan.projection,
                0,
                usize::MAX,
                |ordinal, row| {
                    self.rank_page_row(context, anchor, page, eligible, &dictionary, ordinal, row);
                    !cancelled()
                },
            )?;
            return Ok(());
        }
        let mut chunk = Vec::with_capacity(SNAPSHOT_CHUNK_ROWS);
        #[cfg(test)]
        PAGE_CHUNK_ROWS.set(SNAPSHOT_CHUNK_ROWS);
        let mut failure = None;
        #[cfg(test)]
        PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
        context.source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            0,
            usize::MAX,
            |ordinal, row| {
                chunk.push((ordinal, row));
                if chunk.len() == SNAPSHOT_CHUNK_ROWS
                    && let Err(error) =
                        self.rank_page_chunk(context, anchor, page, eligible, &mut chunk)
                {
                    failure = Some(error);
                    return false;
                }
                !cancelled()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if !cancelled() && !chunk.is_empty() {
            self.rank_page_chunk(context, anchor, page, eligible, &mut chunk)?;
        }
        Ok(())
    }

    fn rank_page_chunk(
        &self,
        context: &PageContext<'_>,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        chunk: &mut Vec<(u64, Row)>,
    ) -> Result<(), ApiError> {
        let dictionary = page_dictionary(context, chunk)?;
        for (ordinal, row) in chunk.drain(..) {
            self.rank_page_row(context, anchor, page, eligible, &dictionary, ordinal, row);
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "ranking keeps request, page, and physical-row coordinates explicit"
    )]
    fn rank_page_row(
        &self,
        context: &PageContext<'_>,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        dictionary: &Dictionary,
        ordinal: u64,
        row: Row,
    ) {
        let Some(candidate) = self.page_candidate(context, ordinal, row, dictionary) else {
            return;
        };
        *eligible = eligible.saturating_add(1);
        if anchor.is_none_or(|anchor| candidate.cmp(anchor) != Ordering::Greater) {
            page.push(candidate);
        }
    }

    fn page_candidate(
        &self,
        context: &PageContext<'_>,
        ordinal: u64,
        row: Row,
        dictionary: &Dictionary,
    ) -> Option<PageRankedRow> {
        if !context.window.matches(&row)
            || !context.plan.matches(&row, dictionary)
            || !self.search.is_empty()
                && !search_matches(&row, dictionary, &context.search_columns, &self.search)
        {
            return None;
        }
        let identity = identity_of(context.plan, &row)?;
        let value = page_order_value(context, &row, &identity, dictionary);
        Some(PageRankedRow {
            staged: PageStagedRow {
                context_index: context.context_index,
                ordinal,
                row,
                identity,
            },
            value,
            direction: self.direction,
        })
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
        extra_columns: &[&'static str],
        cancelled: &impl Fn() -> bool,
    ) -> Result<Readings, ApiError> {
        let mut collected = BTreeMap::new();
        let counters = rate_columns(plan);
        let mut counters = counters;
        if plan.contract.column("stats_since").is_some() {
            counters.push("stats_since");
        }
        for column in extra_columns {
            if plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
                && !counters.contains(column)
            {
                counters.push(column);
            }
        }
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

impl PageRows {
    const fn new(limit: usize) -> Self {
        Self {
            limit,
            rows: BinaryHeap::new(),
        }
    }

    fn push(&mut self, row: PageRankedRow) {
        if self.limit == 0 {
            return;
        }
        if self.rows.len() < self.limit {
            self.rows.push(Reverse(row));
            return;
        }
        let Some(worst) = self.rows.peek() else {
            return;
        };
        if row > worst.0 {
            self.rows.pop();
            self.rows.push(Reverse(row));
        }
    }

    fn finish(self) -> Vec<PageRankedRow> {
        let mut rows: Vec<PageRankedRow> = self.rows.into_iter().map(|Reverse(row)| row).collect();
        rows.sort_by(|left, right| right.cmp(left));
        rows
    }

    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.rows.len()
    }
}

impl PartialEq for PageRankedRow {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PageRankedRow {}

impl PartialOrd for PageRankedRow {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PageRankedRow {
    fn cmp(&self, other: &Self) -> Ordering {
        debug_assert_eq!(
            self.direction, other.direction,
            "one page heap cannot compare opposite sort directions"
        );
        compare_page_order_values(self.value.as_ref(), other.value.as_ref(), self.direction)
            .then_with(|| other.staged.context_index.cmp(&self.staged.context_index))
            .then_with(|| other.staged.ordinal.cmp(&self.staged.ordinal))
    }
}

fn compare_page_order_values(
    left: Option<&PageOrderValue>,
    right: Option<&PageOrderValue>,
    direction: Order,
) -> Ordering {
    let ordered = match (left, right) {
        (Some(PageOrderValue::Integer(left)), Some(PageOrderValue::Integer(right))) => {
            left.cmp(right)
        }
        (Some(PageOrderValue::Float(left)), Some(PageOrderValue::Float(right)))
        | (Some(PageOrderValue::FloatRate(left)), Some(PageOrderValue::FloatRate(right)))
        | (Some(PageOrderValue::FloatRatio(left)), Some(PageOrderValue::FloatRatio(right))) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (
            Some(PageOrderValue::IntegerRate {
                delta: left,
                elapsed: left_elapsed,
            }),
            Some(PageOrderValue::IntegerRate {
                delta: right,
                elapsed: right_elapsed,
            }),
        ) => (left * i128::from(*right_elapsed)).cmp(&(right * i128::from(*left_elapsed))),
        (
            Some(PageOrderValue::IntegerRatio {
                numerator: left_numerator,
                denominator: left_denominator,
            }),
            Some(PageOrderValue::IntegerRatio {
                numerator: right_numerator,
                denominator: right_denominator,
            }),
        ) => compare_u128_ratios(
            *left_numerator,
            *left_denominator,
            *right_numerator,
            *right_denominator,
        ),
        (
            Some(PageOrderValue::IntegerRatio {
                numerator,
                denominator,
            }),
            Some(PageOrderValue::FloatRatio(right)),
        ) => integer_ratio_as_f64(*numerator, *denominator)
            .partial_cmp(right)
            .unwrap_or(Ordering::Equal),
        (
            Some(PageOrderValue::FloatRatio(left)),
            Some(PageOrderValue::IntegerRatio {
                numerator,
                denominator,
            }),
        ) => left
            .partial_cmp(&integer_ratio_as_f64(*numerator, *denominator))
            .unwrap_or(Ordering::Equal),
        (Some(PageOrderValue::Text(left)), Some(PageOrderValue::Text(right))) => left.cmp(right),
        (Some(_), Some(_)) | (None, None) => Ordering::Equal,
        (Some(_), None) => return Ordering::Greater,
        (None, Some(_)) => return Ordering::Less,
    };
    match direction {
        Order::Asc => ordered.reverse(),
        Order::Desc => ordered,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "this path compares an exact ratio with an already inexact floating source"
)]
fn integer_ratio_as_f64(numerator: u128, denominator: u128) -> f64 {
    numerator as f64 / denominator as f64
}

fn compare_u128_ratios(
    mut left_numerator: u128,
    mut left_denominator: u128,
    mut right_numerator: u128,
    mut right_denominator: u128,
) -> Ordering {
    let mut reverse = false;
    loop {
        let whole = (left_numerator / left_denominator).cmp(&(right_numerator / right_denominator));
        if whole != Ordering::Equal {
            return if reverse { whole.reverse() } else { whole };
        }
        let left_remainder = left_numerator % left_denominator;
        let right_remainder = right_numerator % right_denominator;
        let ended = match (left_remainder == 0, right_remainder == 0) {
            (true, true) => return Ordering::Equal,
            (true, false) => Some(Ordering::Less),
            (false, true) => Some(Ordering::Greater),
            (false, false) => None,
        };
        if let Some(ended) = ended {
            return if reverse { ended.reverse() } else { ended };
        }
        (left_numerator, left_denominator) = (left_denominator, left_remainder);
        (right_numerator, right_denominator) = (right_denominator, right_remainder);
        reverse = !reverse;
    }
}

#[cfg(test)]
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

const fn stored_bytes(resolved: Resolved<'_>) -> &[u8] {
    match resolved {
        Resolved::Str(bytes) => bytes,
        Resolved::Blob(blob) => blob.stored_bytes,
    }
}

fn page_dictionary(context: &PageContext<'_>, rows: &[(u64, Row)]) -> Result<Dictionary, ApiError> {
    let mut ids = HashSet::new();
    for (_ordinal, row) in rows {
        if !context.window.matches(row) {
            continue;
        }
        context.plan.add_selection_ids(row, &mut ids);
        for column in context.search_columns.iter().copied().chain(
            context
                .order
                .as_ref()
                .and_then(|order| order.dictionary_column(context.plan)),
        ) {
            if let Some(Cell::StrId(id)) = row.get(column) {
                ids.insert(*id);
            }
        }
    }
    #[cfg(test)]
    if !ids.is_empty() {
        PAGE_CANDIDATE_DICTIONARIES.set(PAGE_CANDIDATE_DICTIONARIES.get() + 1);
    }
    resolved_dictionary(context.source, &ids)
}

fn page_order_value(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    dictionary: &Dictionary,
) -> Option<PageOrderValue> {
    let order = context.order.as_ref()?;
    match &order.kind {
        PageOrderKind::Column(column) => {
            column_order_value(context, row, identity, dictionary, column)
        }
        PageOrderKind::CounterRatio {
            numerator,
            denominator,
            neutral_nulls,
        } => {
            let _elapsed = context.elapsed_for(row)?;
            let before = context.predecessor(row, identity)?;
            let numerator = counter_sum(row, before, numerator, *neutral_nulls)?;
            let denominator = counter_sum(row, before, denominator, *neutral_nulls)?;
            ratio_order_value(numerator, denominator)
        }
        PageOrderKind::ValueRatio {
            numerator,
            denominator,
        } => ratio_order_value(value_sum(row, numerator)?, value_sum(row, denominator)?),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "an interval of 2^52 microseconds is 142 years"
)]
fn column_order_value(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    dictionary: &Dictionary,
    column: &'static str,
) -> Option<PageOrderValue> {
    let stored = row.get(column)?;
    let cumulative = context
        .plan
        .contract
        .column(column)
        .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
    if cumulative {
        let elapsed = context.elapsed_for(row)?;
        let earlier = context.predecessor(row, identity)?.get(column)?;
        return match counter_delta(stored, earlier)? {
            OrderedNumber::Integer(delta) => Some(PageOrderValue::IntegerRate { delta, elapsed }),
            OrderedNumber::Float(delta) => {
                let seconds = elapsed as f64 / 1_000_000.0;
                let rate = delta / seconds;
                rate.is_finite().then_some(PageOrderValue::FloatRate(rate))
            }
        };
    }
    match stored {
        Cell::StrId(id) => dictionary
            .resolve(*id)
            .map(stored_bytes)
            .map(<[u8]>::to_vec)
            .map(PageOrderValue::Text),
        _ => match ordered_cell(stored)? {
            OrderedNumber::Integer(value) => Some(PageOrderValue::Integer(value)),
            OrderedNumber::Float(value) => Some(PageOrderValue::Float(value)),
        },
    }
}

fn counter_sum(
    row: &Row,
    before: &CounterReadings,
    columns: &[&'static str],
    neutral_nulls: bool,
) -> Option<OrderedNumber> {
    let mut sum = None;
    for column in columns {
        let (Some(now), Some(earlier)) = (row.get(column), before.get(column)) else {
            return None;
        };
        if neutral_nulls && matches!((now, earlier), (Cell::Null, Cell::Null)) {
            continue;
        }
        let value = counter_delta(now, earlier)?;
        sum = Some(add_ordered(sum, value)?);
    }
    sum
}

fn value_sum(row: &Row, columns: &[&'static str]) -> Option<OrderedNumber> {
    let mut sum = None;
    for column in columns {
        let value = row.get(column).and_then(ordered_cell)?;
        sum = Some(add_ordered(sum, value)?);
    }
    sum
}

fn add_ordered(left: Option<OrderedNumber>, right: OrderedNumber) -> Option<OrderedNumber> {
    match (left, right) {
        (None, right) => Some(right),
        (Some(OrderedNumber::Integer(left)), OrderedNumber::Integer(right)) => {
            left.checked_add(right).map(OrderedNumber::Integer)
        }
        (Some(left), right) => {
            let sum = left.as_f64() + right.as_f64();
            sum.is_finite().then_some(OrderedNumber::Float(sum))
        }
    }
}

fn ratio_order_value(
    numerator: OrderedNumber,
    denominator: OrderedNumber,
) -> Option<PageOrderValue> {
    match (numerator, denominator) {
        (OrderedNumber::Integer(numerator), OrderedNumber::Integer(denominator))
            if numerator >= 0 && denominator > 0 =>
        {
            Some(PageOrderValue::IntegerRatio {
                numerator: u128::try_from(numerator).ok()?,
                denominator: u128::try_from(denominator).ok()?,
            })
        }
        (numerator, denominator) => {
            let denominator = denominator.as_f64();
            let ratio = numerator.as_f64() / denominator;
            (denominator > 0.0 && ratio.is_finite()).then_some(PageOrderValue::FloatRatio(ratio))
        }
    }
}

fn search_columns(logical_name: &str, plan: &Plan) -> Vec<&'static str> {
    allowed_search_columns(logical_name)
        .iter()
        .filter_map(|name| plan.contract.column(name).map(|column| column.name))
        .collect()
}

fn allowed_search_columns(logical_name: &str) -> &'static [&'static str] {
    match logical_name {
        "pg_stat_statements" => &[
            "query", "queryid", "dbid", "userid", "datname", "usename", "toplevel",
        ],
        "pg_store_plans" => &[
            "plan",
            "planid",
            "queryid",
            "queryid_stat_statements",
            "dbid",
            "userid",
            "datname",
            "usename",
        ],
        "pg_stat_user_tables" => &[
            "datname",
            "datid",
            "schemaname",
            "relname",
            "relid",
            "tablespace",
        ],
        "pg_stat_user_indexes" => &[
            "datname",
            "datid",
            "schemaname",
            "relname",
            "relid",
            "indexrelname",
            "indexrelid",
            "tablespace",
            "amname",
            "indexdef",
        ],
        _ => &[],
    }
}

fn search_matches(
    row: &Row,
    dictionary: &Dictionary,
    columns: &[&'static str],
    patterns: &[GlobPattern],
) -> bool {
    columns.iter().any(|column| {
        row.get(column)
            .and_then(|value| searchable_text(value, dictionary))
            .is_some_and(|text| patterns.iter().any(|pattern| pattern.matches(&text)))
    })
}

fn searchable_text(value: &Cell, dictionary: &Dictionary) -> Option<String> {
    match value {
        Cell::I16(value) => Some(value.to_string()),
        Cell::I32(value) => Some(value.to_string()),
        Cell::I64(value) | Cell::Ts(value) => Some(value.to_string()),
        Cell::U32(value) => Some(value.to_string()),
        Cell::U64(value) => Some(value.to_string()),
        Cell::F64(value) if value.is_finite() => Some(value.to_string()),
        Cell::Bool(value) => Some(value.to_string()),
        Cell::StrId(id) => dictionary
            .resolve(*id)
            .and_then(|resolved| std::str::from_utf8(stored_bytes(resolved)).ok())
            .map(ToOwned::to_owned),
        Cell::Null | Cell::ListI32(_) | Cell::F64(_) => None,
    }
}

impl GlobPattern {
    fn new(raw: &str) -> Self {
        let mut tokens = vec![GlobToken::Star];
        for character in raw.chars() {
            let token = match character {
                '*' => GlobToken::Star,
                '?' => GlobToken::Any,
                literal => GlobToken::Literal(literal),
            };
            if token != GlobToken::Star || tokens.last() != Some(&GlobToken::Star) {
                tokens.push(token);
            }
        }
        if tokens.last() != Some(&GlobToken::Star) {
            tokens.push(GlobToken::Star);
        }
        Self(tokens)
    }

    fn matches(&self, candidate: &str) -> bool {
        let candidate = candidate.chars().collect::<Vec<_>>();
        let mut pattern_index = 0;
        let mut candidate_index = 0;
        let mut star = None;
        let mut retry = 0;
        while candidate_index < candidate.len() {
            match self.0.get(pattern_index) {
                Some(GlobToken::Literal(wanted))
                    if unicode_char_equal(*wanted, candidate[candidate_index]) =>
                {
                    pattern_index += 1;
                    candidate_index += 1;
                }
                Some(GlobToken::Any) => {
                    pattern_index += 1;
                    candidate_index += 1;
                }
                Some(GlobToken::Star) => {
                    star = Some(pattern_index);
                    pattern_index += 1;
                    retry = candidate_index;
                }
                _ if star.is_some() => {
                    retry += 1;
                    candidate_index = retry;
                    pattern_index = star.unwrap_or(0) + 1;
                }
                _ => return false,
            }
        }
        while self.0.get(pattern_index) == Some(&GlobToken::Star) {
            pattern_index += 1;
        }
        pattern_index == self.0.len()
    }
}

fn unicode_char_equal(left: char, right: char) -> bool {
    left.to_lowercase().eq(right.to_lowercase())
}

impl SnapshotCursor {
    fn parse(raw: &str) -> Result<Self, ApiError> {
        let fields = raw.split(',').collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(ApiError::BadCursor);
        }
        Ok(Self {
            segment_id: fields[0].parse().map_err(|_error| ApiError::BadCursor)?,
            active_position: fields[1].parse().map_err(|_error| ApiError::BadCursor)?,
            context_index: fields[2].parse().map_err(|_error| ApiError::BadCursor)?,
            ordinal: fields[3].parse().map_err(|_error| ApiError::BadCursor)?,
            binding: fields[4].parse().map_err(|_error| ApiError::BadCursor)?,
        })
    }

    fn encode(self) -> String {
        format!(
            "{},{},{},{},{}",
            self.segment_id, self.active_position, self.context_index, self.ordinal, self.binding
        )
    }
}

const fn partition_context_index(layout_index: usize, source: SampleSource) -> usize {
    layout_index * 3
        + match source {
            SampleSource::Current => 0,
            SampleSource::Earlier => 1,
            SampleSource::Older => 2,
        }
}

const fn timed_context_index(layout_index: usize, source: SampleSource) -> usize {
    partition_context_index(layout_index, source)
}

fn record_partition_moment(
    moments: &mut BTreeMap<IdentityCell, PartitionMoments>,
    partition: IdentityCell,
    located: LocatedMoment,
) {
    let Some(chosen) = moments.get_mut(&partition) else {
        moments.insert(
            partition,
            PartitionMoments {
                current: located,
                previous: None,
            },
        );
        return;
    };
    match located.at.cmp(&chosen.current.at) {
        Ordering::Equal => chosen.current = chosen.current.max(located),
        Ordering::Greater => {
            chosen.previous = Some(chosen.current);
            chosen.current = located;
        }
        Ordering::Less => {
            chosen.previous =
                Some(
                    chosen
                        .previous
                        .map_or(located, |before| match located.at.cmp(&before.at) {
                            Ordering::Equal => before.max(located),
                            Ordering::Greater => located,
                            Ordering::Less => before,
                        }),
                );
        }
    }
}

fn pin(current: SegmentRef, cursor: Option<SnapshotCursor>) -> Result<SegmentRef, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(current);
    };
    match current.kind() {
        SegmentKind::Finished if cursor.active_position == 0 => Ok(current),
        SegmentKind::Active => current
            .at_active_position(cursor.active_position)
            .map_err(|_error| ApiError::BadCursor),
        SegmentKind::Finished => Err(ApiError::BadCursor),
    }
}

fn snapshot_binding(request: &SnapshotRequest) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_part(&mut hash, b"segment", &request.segment_id.to_le_bytes());
    hash_part(&mut hash, b"at", &request.at.to_le_bytes());
    for section in &request.sections {
        hash_part(&mut hash, b"section", section.as_bytes());
    }
    if let Some(type_id) = request.type_id {
        hash_part(&mut hash, b"type", &type_id.to_le_bytes());
    }
    for field in &request.fields {
        hash_part(&mut hash, b"field", field.as_bytes());
    }
    if let Some(text) = request.text {
        hash_part(&mut hash, b"text", &text.to_le_bytes());
    }
    for filter in &request.filters {
        hash_part(&mut hash, b"filter-column", filter.column.as_bytes());
        hash_part(&mut hash, b"filter-value", filter.value.as_bytes());
    }
    for search in &request.search {
        hash_part(&mut hash, b"search", search.as_bytes());
    }
    for by in &request.by {
        hash_part(&mut hash, b"by", by.as_bytes());
    }
    if let Some(group) = request.group {
        hash_part(
            &mut hash,
            b"group",
            match group {
                RelationGroup::Database => b"database",
                RelationGroup::Schema => b"schema",
                RelationGroup::Object => b"object",
            },
        );
    }
    hash_part(
        &mut hash,
        b"direction",
        match request.direction {
            Order::Asc => b"asc",
            Order::Desc => b"desc",
        },
    );
    hash
}

fn hash_part(hash: &mut u64, tag: &[u8], bytes: &[u8]) {
    hash_bytes(hash, tag);
    hash_bytes(hash, bytes);
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.len().to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
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

#[cfg(test)]
fn available_field_index(fields: &[super::query::OutputField], name: &str) -> Option<usize> {
    fields
        .iter()
        .position(|field| field.name == name && field.column.is_some())
}

fn preceding(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segments: Vec<SegmentRef>,
    sections: &[String],
    at: i64,
) -> Result<(Option<Segment>, Option<Segment>), ApiError> {
    let compatible = segment_ref
        .sections()
        .iter()
        .filter(|section| {
            logical_section_name(section.type_id)
                .is_some_and(|logical| sections.iter().any(|name| name == logical))
        })
        .map(|section| section.type_id)
        .collect::<HashSet<_>>();
    let mut candidates = segments
        .into_iter()
        .filter(|candidate| candidate.id() < segment_ref.id() && candidate.min_ts() <= at)
        .filter(|candidate| {
            candidate
                .sections()
                .iter()
                .any(|section| compatible.contains(&section.type_id))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(SegmentRef::id);
    let mut chosen = Vec::with_capacity(2);
    while let Some(candidate) = candidates.pop() {
        let segment = reader.open_segment(&candidate)?;
        if segment_has_sample_at(&segment, &compatible, at)? {
            chosen.push(segment);
            if chosen.len() == 2 {
                break;
            }
        }
    }
    let mut chosen = chosen.into_iter();
    Ok((chosen.next(), chosen.next()))
}

fn segment_has_sample_at(
    segment: &Segment,
    compatible: &HashSet<u32>,
    at: i64,
) -> Result<bool, ApiError> {
    for type_id in compatible {
        if segment.rows_of(*type_id).is_none() {
            continue;
        }
        let Some(timestamp) = contract(*type_id).and_then(|layout| {
            layout
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name)
        }) else {
            continue;
        };
        let mut found = false;
        segment.visit_rows(*type_id, &[timestamp], 0, usize::MAX, |_ordinal, row| {
            found = row_timestamp(&row, timestamp).is_some_and(|stored| stored <= at);
            !found
        })?;
        if found {
            return Ok(true);
        }
    }
    Ok(false)
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
