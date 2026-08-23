//! Reads one snapshot and derives counter rates.

mod relation;
mod search;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
use std::cell::Cell as Counter;

use kronika_reader::{Cell, Dictionary, Reader, Resolved, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, ColumnType, contract, logical_section_name};
use serde_json::{Value, json};

use self::search::{
    Quantity, SearchClause, SearchOperator, SearchValue, StructuredSearch, result_field,
    search_fields,
};
#[cfg(test)]
use self::search::{SEARCH_MAX_CLAUSES, SEARCH_MAX_VALUE_CHARS};
use super::query::{Plan, plans, resolved_dictionary};
use super::render::{cell, projected_layout, shorten};
use super::{ApiError, CachePolicy, ResponseMeta, explicit_segment_with_listing};
use crate::route::{DataRequest, Order, RelationGroup, SegmentRequest, SnapshotRequest};
use crate::route::{SeriesRequest, Window};

/// The accepted shared structured-search grammar specialized to Event facts.
pub(crate) struct EventSearch(StructuredSearch);

impl EventSearch {
    pub(crate) fn parse(raw: &str) -> Result<Self, ApiError> {
        StructuredSearch::parse(raw, "events")
            .map(Self)
            .map_err(|_diagnostic| ApiError::BadFilter("find".to_owned()))
    }

    pub(crate) fn canonical(&self) -> &str {
        self.0.canonical()
    }

    pub(crate) fn matches(
        &self,
        source: &str,
        tier: &str,
        row: &Row,
        dictionary: &Dictionary,
    ) -> bool {
        self.0.matches_all(|clause| {
            let matches = |candidate: &str| search_value_matches(candidate, &clause.value);
            let category = row.get("category").and_then(event_cell_text);
            match clause.key {
                "text" => {
                    matches(source)
                        || matches(tier)
                        || category.as_deref().is_some_and(matches)
                        || row.iter().any(|(_name, cell)| {
                            event_resolved_text(cell, dictionary).is_some_and(matches)
                        })
                }
                "kind" => matches(tier),
                "source" => matches(source),
                "category" => category.as_deref().is_some_and(matches),
                _other => false,
            }
        })
    }
}

fn event_cell_text(cell: &Cell) -> Option<String> {
    match cell {
        Cell::I16(value) => Some(value.to_string()),
        Cell::I32(value) => Some(value.to_string()),
        Cell::I64(value) | Cell::Ts(value) => Some(value.to_string()),
        Cell::U32(value) => Some(value.to_string()),
        Cell::U64(value) => Some(value.to_string()),
        Cell::Bool(value) => Some(value.to_string()),
        Cell::F64(value) if value.is_finite() => Some(value.to_string()),
        Cell::Null | Cell::StrId(_) | Cell::ListI32(_) | Cell::F64(_) => None,
    }
}

fn event_resolved_text<'a>(cell: &Cell, dictionary: &'a Dictionary) -> Option<&'a str> {
    let Cell::StrId(id) = cell else {
        return None;
    };
    let bytes = match dictionary.resolve(*id)? {
        Resolved::Str(bytes) => bytes,
        Resolved::Blob(blob) => blob.stored_bytes,
    };
    std::str::from_utf8(bytes).ok()
}

pub(crate) struct PreparedSnapshot {
    reader: Reader,
    anchor: SegmentRef,
    prior_sources: Vec<SegmentRef>,
    relation_predecessors: Vec<SegmentRef>,
    relation_moments: RetainedRelationMoments,
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
    search: Option<Box<StructuredSearch>>,
    first_match_query_id: Option<i64>,
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

#[derive(Clone, Copy)]
struct RowCoordinate {
    segment_id: i64,
    ordinal: u64,
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
    logical_name: &'a str,
    source: &'a SegmentRef,
    rows: u64,
    window: RowWindow,
    previous: Option<Arc<Readings>>,
    elapsed: Option<i64>,
    elapsed_by_partition: Arc<BTreeMap<IdentityCell, i64>>,
    sample_from: Option<i64>,
    sample_to: Option<i64>,
    order: Option<PageOrder>,
    search_columns: Vec<&'static str>,
    clock_ticks_per_second: Option<u128>,
    block_size: Option<u128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PartitionSource {
    Earlier(usize),
    Current,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocatedMoment {
    at: i64,
    sources: BTreeSet<PartitionSource>,
}

#[derive(Clone)]
struct PartitionMoments {
    current: LocatedMoment,
    previous: Option<LocatedMoment>,
}

struct SelectedPartition {
    layout_index: usize,
    type_id: u32,
    moments: PartitionMoments,
}

struct PartitionRateState {
    previous: Arc<Readings>,
    elapsed_by_partition: Arc<BTreeMap<IdentityCell, i64>>,
    sample_from: Option<i64>,
    sample_to: Option<i64>,
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

fn rows_of(reference: &SegmentRef, type_id: u32) -> Option<u64> {
    reference
        .sections()
        .iter()
        .find(|section| section.type_id == type_id)
        .map(|section| section.rows)
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
        if let Some(column) = self.plan.contract.column("starttime")
            && row.get(column.name)? != before.get(column.name)?
        {
            return None;
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

#[derive(Clone, Copy, Default)]
struct PageFacts {
    clock_ticks_per_second: CachedFact,
    block_size: CachedFact,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CachedFact {
    #[default]
    Unknown,
    Absent,
    Value(u128),
}

impl CachedFact {
    const fn value(self) -> Option<u128> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unknown | Self::Absent => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobPattern(Vec<GlobToken>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    Star,
    Any,
    Literal(char),
}

// One bounded batch covers the maximum public page while amortizing projected
// section and dictionary reads for snapshot and relation-history consumers.
const SNAPSHOT_CHUNK_ROWS: usize = 1_024;
const PROCESS_USER_TYPE_ID: u32 = 1_124_002;
const PROCESS_VIRTUAL_FIELDS: &[&str] = &["user", "effective_user"];
const MAX_PROCESS_USERS: usize = 4 * 1024;

#[derive(Default)]
struct ProcessUsers {
    names: HashMap<(u8, u32), String>,
}

impl ProcessUsers {
    fn load(segment: &Segment, plan: &Plan) -> Result<Self, ApiError> {
        if plan.contract.name != "os_process" || segment.rows_of(PROCESS_USER_TYPE_ID).is_none() {
            return Ok(Self::default());
        }
        let mut encoded = Vec::new();
        let mut ids = HashSet::new();
        segment.visit_rows(
            PROCESS_USER_TYPE_ID,
            &["uid", "username", "scope"],
            0,
            MAX_PROCESS_USERS.saturating_add(1),
            |_ordinal, row| {
                let (Some(Cell::U32(uid)), Some(Cell::StrId(username)), Some(Cell::U32(scope))) =
                    (row.get("uid"), row.get("username"), row.get("scope"))
                else {
                    return true;
                };
                ids.insert(*username);
                encoded.push((*scope, *uid, *username));
                encoded.len() <= MAX_PROCESS_USERS
            },
        )?;
        if encoded.len() > MAX_PROCESS_USERS {
            return Err(ApiError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "os_user exceeds the per-segment mapping limit",
            ))));
        }
        let dictionary = resolved_dictionary(segment, &ids)?;
        let mut names = HashMap::with_capacity(encoded.len());
        for (scope, uid, username) in encoded {
            let Ok(scope) = u8::try_from(scope) else {
                continue;
            };
            let Some(Resolved::Str(bytes)) = dictionary.resolve(username) else {
                continue;
            };
            let Ok(username) = std::str::from_utf8(bytes) else {
                continue;
            };
            names
                .entry((scope, uid))
                .or_insert_with(|| username.to_owned());
        }
        Ok(Self { names })
    }

    fn for_row<'a>(&'a self, row: &Row, uid_column: &str) -> Option<&'a str> {
        let (Some(Cell::U32(scope)), Some(Cell::U32(uid))) =
            (row.get("scope"), row.get(uid_column))
        else {
            return None;
        };
        let scope = u8::try_from(*scope).ok()?;
        self.names.get(&(scope, *uid)).map(String::as_str)
    }
}

#[cfg(test)]
thread_local! {
    static PAGE_CHUNK_ROWS: Counter<usize> = const { Counter::new(0) };
    static PAGE_SOURCE_VISITS: Counter<usize> = const { Counter::new(0) };
    static PAGE_CANDIDATE_DICTIONARIES: Counter<usize> = const { Counter::new(0) };
    static FIRST_MATCH_ROWS: Counter<usize> = const { Counter::new(0) };
    static CONTEXT_CHUNK_ROWS: Counter<usize> = const { Counter::new(0) };
    static CONTEXT_STAGED_ROWS: Counter<usize> = const { Counter::new(0) };
    static CONTEXT_SELECTION_DICTIONARIES: Counter<usize> = const { Counter::new(0) };
    static RELATION_MOMENT_VISITS: Counter<usize> = const { Counter::new(0) };
    static PARTITION_PREDECESSOR_VISITS: Counter<usize> = const { Counter::new(0) };
    static RELATION_PROJECTED_METRICS: Counter<usize> = const { Counter::new(0) };
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

#[cfg(test)]
pub(crate) fn reset_first_match_rows() {
    FIRST_MATCH_ROWS.set(0);
}

#[cfg(test)]
pub(crate) fn first_match_rows() -> usize {
    FIRST_MATCH_ROWS.get()
}

#[cfg(test)]
pub(crate) fn reset_context_operations() {
    CONTEXT_CHUNK_ROWS.set(0);
    CONTEXT_STAGED_ROWS.set(0);
    CONTEXT_SELECTION_DICTIONARIES.set(0);
}

#[cfg(test)]
pub(crate) fn context_operations() -> (usize, usize, usize) {
    (
        CONTEXT_CHUNK_ROWS.get(),
        CONTEXT_STAGED_ROWS.get(),
        CONTEXT_SELECTION_DICTIONARIES.get(),
    )
}

#[cfg(test)]
pub(crate) fn reset_relation_snapshot_operations() {
    RELATION_MOMENT_VISITS.set(0);
    PARTITION_PREDECESSOR_VISITS.set(0);
    RELATION_PROJECTED_METRICS.set(0);
}

#[cfg(test)]
pub(crate) fn relation_snapshot_operations() -> (usize, usize, usize) {
    (
        RELATION_MOMENT_VISITS.get(),
        PARTITION_PREDECESSOR_VISITS.get(),
        RELATION_PROJECTED_METRICS.get(),
    )
}

pub(super) fn stream_relation_history(
    reader: &Reader,
    listed: &[SegmentRef],
    window: Window,
    request: &SeriesRequest,
    emit: &mut impl FnMut(Value) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    relation::stream_history(reader, listed, window, request, emit, cancelled)
}

#[cfg(test)]
pub(crate) use relation::{history_operations, reset_history_operations, tablespace_moment_visits};

pub(super) fn prepare(root: &Path, request: SnapshotRequest) -> Result<PreparedSnapshot, ApiError> {
    let cursor = request
        .cursor
        .as_deref()
        .map(SnapshotCursor::parse)
        .transpose()?;
    let search = prepared_search(&request, cursor.is_some())?;
    let first_match_query_id = if request.first_match {
        Some(
            search
                .as_deref()
                .and_then(StructuredSearch::first_match_query_id)
                .ok_or_else(|| ApiError::BadFilter("first_match".to_owned()))?,
        )
    } else {
        None
    };
    let binding = snapshot_binding(&request, search.as_deref());
    let parsed = cursor
        .filter(|cursor| cursor.segment_id == request.segment_id && cursor.binding == binding);
    if request.cursor.is_some() && parsed.is_none() {
        return Err(ApiError::BadCursor);
    }
    let (reader, current, segments) = explicit_segment_with_listing(root, request.segment_id)?;
    let segment_ref = pin(current, parsed)?;
    let segment = reader.open_segment(&segment_ref)?;
    let active_position = segment_ref.active_position().unwrap_or(0);
    if parsed.is_some_and(|cursor| cursor.active_position != active_position) {
        return Err(ApiError::BadCursor);
    }
    let relation_fields = request
        .group
        .map(|group| relation::output_fields(&request.sections, group, &request.fields))
        .transpose()?
        .unwrap_or_default();
    let (physical_filters, relation_filters) = relation::split_filters(&request)?;
    let mut physical_request = request.clone();
    physical_request.filters = physical_filters;
    let sections = section_plans(
        &segment,
        &physical_request,
        &relation_fields,
        search.as_deref(),
    )?;
    let has_relation = sections
        .iter()
        .any(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_some());
    let has_other = sections
        .iter()
        .any(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_none());
    let prior_sources = if has_other {
        preceding(
            &reader,
            &segment_ref,
            segments.clone(),
            &segment,
            &sections,
            request.at,
        )?
    } else {
        Vec::new()
    };
    let (relation_predecessors, relation_moments) = if has_relation {
        relation_preceding(
            &reader,
            &segment_ref,
            segments,
            &segment,
            &sections,
            &physical_request.filters,
            request.at,
        )?
    } else {
        (Vec::new(), BTreeMap::new())
    };
    validate_search_projection(search.as_deref(), &sections)?;
    validate_exact_locator(&segment, &request, &sections)?;
    drop(segment);
    Ok(PreparedSnapshot {
        reader,
        anchor: segment_ref,
        prior_sources,
        relation_predecessors,
        relation_moments,
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
        search,
        first_match_query_id,
        text: request.text,
        row_ordinal: request.row_ordinal,
    })
}

fn prepared_search(
    request: &SnapshotRequest,
    cursor_present: bool,
) -> Result<Option<Box<StructuredSearch>>, ApiError> {
    let parsed = request
        .search
        .as_deref()
        .map(|raw| {
            let [logical_name] = request.sections.as_slice() else {
                return Err(ApiError::BadFilter("search".to_owned()));
            };
            let search = StructuredSearch::parse(raw, logical_name).map_err(|_diagnostic| {
                if cursor_present {
                    ApiError::BadCursor
                } else {
                    ApiError::BadFilter("search".to_owned())
                }
            })?;
            if request
                .group
                .is_some_and(|group| group != RelationGroup::Object)
            {
                search.validate_grouped_phase().map_err(|_diagnostic| {
                    if cursor_present {
                        ApiError::BadCursor
                    } else {
                        ApiError::BadFilter("search".to_owned())
                    }
                })?;
            }
            Ok(search)
        })
        .transpose()?;
    Ok(parsed.map(Box::new))
}

fn section_plans(
    segment: &Segment,
    request: &SnapshotRequest,
    relation_fields: &[String],
    search: Option<&StructuredSearch>,
) -> Result<Vec<SectionPlans>, ApiError> {
    let shared_projection = request.sections.len() > 1 && !request.fields.is_empty();
    if shared_projection {
        validate_shared_projection(segment, &request.sections, &request.fields)?;
    }
    let mut sections = Vec::with_capacity(request.sections.len());
    for logical_name in &request.sections {
        let fields = if let Some(group) = request.group {
            let wanted = relation::snapshot_physical_fields(
                logical_name,
                group,
                relation_fields,
                &request.by,
                search,
            )?;
            section_projection(segment, logical_name, &wanted)
        } else if shared_projection {
            section_projection(segment, logical_name, &request.fields)
        } else {
            request.fields.clone()
        };
        if shared_projection && fields.is_empty() {
            continue;
        }
        let selected_virtual = if logical_name == "os_process" {
            if fields.is_empty() {
                PROCESS_VIRTUAL_FIELDS.to_vec()
            } else {
                fields
                    .iter()
                    .filter_map(|field| {
                        PROCESS_VIRTUAL_FIELDS
                            .contains(&field.as_str())
                            .then_some(field.as_str())
                    })
                    .collect()
            }
        } else {
            Vec::new()
        };
        let physical_fields = fields
            .iter()
            .filter(|field| !PROCESS_VIRTUAL_FIELDS.contains(&field.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: request.segment_id,
                section: logical_name.clone(),
            },
            fields: physical_fields.clone(),
            filters: request.filters.clone(),
            type_id: request.type_id,
            after: None,
        };
        // Missing sections are empty so one source cannot fail the snapshot.
        match plans(segment, &data, true) {
            Ok(mut plans) => {
                for plan in &mut plans {
                    if !fields.is_empty() {
                        plan.retain_output_fields(&physical_fields);
                    }
                    for field in &selected_virtual {
                        plan.add_virtual_output(field);
                    }
                    if !fields.is_empty() {
                        plan.order_output_fields(&fields);
                    }
                    if !selected_virtual.is_empty() {
                        plan.add_projection_columns(&["uid", "euid", "scope"]);
                    }
                    if logical_name == "pg_store_plans" {
                        plan.add_aliased_output("calls_per_second", "calls");
                    }
                    let order = page_order(logical_name, plan, &request.by);
                    plan.add_projection_columns(
                        &order.as_ref().map_or_else(Vec::new, PageOrder::columns),
                    );
                    if logical_name == "os_process" && plan.contract.column("starttime").is_some() {
                        plan.add_projection_columns(&["starttime"]);
                    }
                    if let Some(search) = search {
                        plan.add_projection_columns(&search_columns(logical_name, plan, search));
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
        "pg_stat_user_tables" => matches!(plan.type_id, 1_013_005..=1_013_008),
        "pg_stat_user_indexes" => matches!(plan.type_id, 1_014_003..=1_014_004),
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
    search: Option<&StructuredSearch>,
    sections: &[SectionPlans],
) -> Result<(), ApiError> {
    if let Some(search) = search {
        let [section] = sections else {
            return Err(ApiError::BadFilter("search".to_owned()));
        };
        if !section.plans.iter().any(|plan| {
            search.member_clauses().all(|clause| {
                !search_clause_columns(&section.logical_name, plan, clause.key).is_empty()
            })
        }) {
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
        ResponseMeta::ok(match self.anchor.kind() {
            SegmentKind::Finished => CachePolicy::Revalidate,
            SegmentKind::Active => CachePolicy::NoStore,
        })
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        if cancelled()
            || !emit(json!({
                "record": "snapshot",
                "segment": { "id": self.anchor.id().to_string() },
                "at": self.at.to_string(),
            }))
        {
            return Ok(());
        }
        if self.first_match_query_id.is_some() {
            return self.emit_first_match(emit, cancelled);
        }
        if self.page_size.is_some() {
            if self.group.is_some() {
                return self.emit_relation_page(emit, cancelled);
            }
            return self.emit_page(emit, cancelled);
        }
        let mut facts = HashMap::new();
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
                    if !self.emit_section(
                        section,
                        layout_index,
                        plan,
                        emit,
                        cancelled,
                        &mut facts,
                    )? {
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
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit) {
                return Ok(false);
            }
        }
        let contexts = self.partitioned_contexts(section, cancelled)?;
        for context in &contexts {
            if self.row_ordinal.is_some() && context.source.id() != self.anchor.id() {
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
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let source = self.reader.open_segment(context.source)?;
        let process_users = ProcessUsers::load(&source, context.plan)?;
        let selection_dictionary = context.plan.exact_filter_dictionary(&source)?;
        #[cfg(test)]
        if context.plan.needs_selection_dictionary() {
            CONTEXT_SELECTION_DICTIONARIES.set(CONTEXT_SELECTION_DICTIONARIES.get() + 1);
        }
        let mut chunk = Vec::with_capacity(SNAPSHOT_CHUNK_ROWS);
        let mut failure = None;
        let mut connected = true;
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                if context.window.matches(&row) {
                    chunk.push((ordinal, row));
                }
                if chunk.len() == SNAPSHOT_CHUNK_ROWS {
                    match self.emit_context_chunk(
                        context,
                        &source,
                        &process_users,
                        &selection_dictionary,
                        &mut chunk,
                        emit,
                        cancelled,
                    ) {
                        Ok(still_connected) => connected = still_connected,
                        Err(error) => failure = Some(error),
                    }
                }
                connected && failure.is_none() && !cancelled()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if cancelled() || !connected {
            return Ok(false);
        }
        if chunk.is_empty() {
            return Ok(true);
        }
        self.emit_context_chunk(
            context,
            &source,
            &process_users,
            &selection_dictionary,
            &mut chunk,
            emit,
            cancelled,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "chunk emission carries the exact source, dictionaries, output sink, and cancellation boundary"
    )]
    fn emit_context_chunk(
        &self,
        context: &PageContext<'_>,
        source: &Segment,
        process_users: &ProcessUsers,
        selection_dictionary: &Dictionary,
        rows: &mut Vec<(u64, Row)>,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        #[cfg(test)]
        CONTEXT_CHUNK_ROWS.set(CONTEXT_CHUNK_ROWS.get().max(rows.len()));
        let mut staged = Vec::new();
        for (ordinal, row) in rows.drain(..) {
            context
                .plan
                .validate_exact_filter_ids(&row, selection_dictionary)?;
            if !context.plan.matches(&row, selection_dictionary) {
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
        #[cfg(test)]
        CONTEXT_STAGED_ROWS.set(CONTEXT_STAGED_ROWS.get().saturating_add(staged.len()));
        if staged.is_empty() {
            return Ok(true);
        }
        let dictionary = retained_dictionary(source, &staged)?;
        for staged in staged {
            let before = context.predecessor(&staged.row, &staged.identity);
            let value = Self::row_record(
                context.plan,
                &staged.row,
                before,
                context.elapsed_for(&staged.row),
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: staged.ordinal,
                },
                &dictionary,
                process_users,
                self.text,
            )?;
            if cancelled() || !emit(value) {
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
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
        facts: &mut HashMap<i64, PageFacts>,
    ) -> Result<bool, ApiError> {
        if !Self::emit_layout(section, plan, emit) {
            return Ok(false);
        }
        if !plan.applies() {
            return Ok(true);
        }
        let Some(timestamp) = plan.timestamp else {
            return self.emit_untimed(plan, emit, cancelled);
        };
        for context in
            self.timed_contexts(section, layout_index, plan, timestamp, cancelled, facts)?
        {
            if self.row_ordinal.is_some() && context.source.id() != self.anchor.id() {
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
        emit: &mut impl FnMut(Value) -> bool,
    ) -> bool {
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
        let mut layout = projected_layout(&section.logical_name, plan.contract, &fields);
        if section.logical_name == "os_process"
            && let Some(columns) = layout.get_mut("columns").and_then(Value::as_array_mut)
        {
            for column in columns {
                if column
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| PROCESS_VIRTUAL_FIELDS.contains(&name))
                {
                    *column = json!({
                        "name": column.get("name").cloned().unwrap_or(Value::Null),
                        "type": "dictionary_value",
                        "class": "label",
                        "unit": "none",
                        "nullable": true,
                        "available": true,
                    });
                }
            }
        }
        emit(json!({
            "record": "layout",
            "layout": layout,
            "rates": output_rate_fields(plan),
        }))
    }

    fn emit_untimed(
        &self,
        plan: &Plan,
        emit: &mut impl FnMut(Value) -> bool,
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
        let source = self.reader.open_segment(&self.anchor)?;
        source.visit_rows(
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
        self.emit_rows(&source, plan, rows, rates, emit, cancelled)
    }

    fn emit_rows(
        &self,
        source: &Segment,
        plan: &Plan,
        rows: Vec<(u64, Row)>,
        rates: RateContext<'_>,
        emit: &mut impl FnMut(Value) -> bool,
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
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool, ApiError> {
        let dictionary = retained_dictionary(source, &staged)?;
        let process_users = ProcessUsers::load(source, plan)?;
        for staged in staged {
            let before = rates
                .previous
                .and_then(|previous| previous.get(&staged.identity));
            let value = Self::row_record(
                plan,
                &staged.row,
                before,
                rates.elapsed,
                RowCoordinate {
                    segment_id: source.id(),
                    ordinal: staged.ordinal,
                },
                &dictionary,
                &process_users,
                self.text,
            )?;
            if cancelled() || !emit(value) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_page(
        &self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let [section] = self.sections.as_slice() else {
            return Err(ApiError::BadCursor);
        };
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit) {
                return Ok(());
            }
        }
        let contexts = self.page_contexts(section, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let process_users = contexts
            .iter()
            .map(|context| {
                let source = self.reader.open_segment(context.source)?;
                ProcessUsers::load(&source, context.plan)
                    .map(|users| (context.context_index, users))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let anchor = self
            .cursor
            .map(|cursor| self.cursor_anchor(&contexts, &process_users, cursor))
            .transpose()?;
        let page_size = self.page_size.ok_or(ApiError::BadCursor)?;
        let mut page = PageRows::new(page_size.saturating_add(1));
        let mut eligible = 0_u64;
        for context in &contexts {
            self.scan_page(
                context,
                process_users
                    .get(&context.context_index)
                    .ok_or(ApiError::BadCursor)?,
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
                segment_id: self.anchor.id(),
                active_position: self.anchor.active_position().unwrap_or(0),
                context_index: row.context_index,
                ordinal: row.ordinal,
                binding: self.binding,
            }
            .encode()
        });
        ranked.truncate(page_size);
        let returned = ranked.len();
        if !self.emit_page_rows(&contexts, &process_users, ranked, emit, cancelled)? {
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
        );
        Ok(())
    }

    fn emit_first_match(
        &self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let [section] = self.sections.as_slice() else {
            return Err(ApiError::BadCursor);
        };
        let wanted = self.first_match_query_id.ok_or(ApiError::BadCursor)?;
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit) {
                return Ok(());
            }
        }
        let contexts = self.first_match_contexts(section, cancelled)?;
        let mut returned = 0_usize;
        for context in &contexts {
            let Some((ordinal, row, dictionary)) =
                self.first_match_row(context, wanted, cancelled)?
            else {
                if cancelled() {
                    return Ok(());
                }
                continue;
            };
            let value = Self::row_record(
                context.plan,
                &row,
                None,
                None,
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal,
                },
                &dictionary,
                &ProcessUsers::default(),
                None,
            )?;
            if cancelled() || !emit(value) {
                return Ok(());
            }
            returned = 1;
            break;
        }
        Self::emit_page_trailer(
            section,
            &contexts,
            &PageMetadata {
                eligible: u64::from(returned != 0),
                returned,
                has_more: false,
                next_cursor: None,
                page_size: 1,
            },
            self.direction,
            emit,
        );
        Ok(())
    }

    fn first_match_row(
        &self,
        context: &PageContext<'_>,
        wanted: i64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<(u64, Row, Dictionary)>, ApiError> {
        let query_id = context
            .plan
            .contract
            .column("queryid")
            .ok_or_else(|| ApiError::BadFilter("first_match".to_owned()))?
            .name;
        let query = context
            .plan
            .contract
            .column("query")
            .ok_or_else(|| ApiError::BadFilter("first_match".to_owned()))?
            .name;
        let mut projection = vec![query_id, query];
        if let RowWindow::Shared { timestamp, .. } = context.window {
            projection.push(timestamp);
        }
        projection.sort_unstable();
        projection.dedup();

        let source = self.reader.open_segment(context.source)?;
        let mut offset = 0;
        while offset < context.rows && !cancelled() {
            let mut candidate = None;
            source.visit_rows(
                context.plan.type_id,
                &projection,
                offset,
                usize::MAX,
                |ordinal, row| {
                    #[cfg(test)]
                    FIRST_MATCH_ROWS.set(FIRST_MATCH_ROWS.get() + 1);
                    if context.window.matches(&row)
                        && matches!(row.get(query_id), Some(Cell::I64(stored)) if *stored == wanted)
                    {
                        candidate = Some((ordinal, row));
                        return false;
                    }
                    !cancelled()
                },
            )?;
            let Some((ordinal, row)) = candidate else {
                return Ok(None);
            };
            offset = ordinal.checked_add(1).unwrap_or(context.rows);
            let Some(Cell::StrId(id)) = row.get(query) else {
                continue;
            };
            let dictionary = resolved_dictionary(&source, &HashSet::from([*id]))?;
            let resolved = dictionary.resolve(*id).ok_or_else(|| {
                ApiError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unresolved dictionary id {id}"),
                )))
            })?;
            if stored_bytes(resolved).is_empty() {
                continue;
            }
            return Ok(Some((ordinal, row, dictionary)));
        }
        Ok(None)
    }

    fn emit_page_rows(
        &self,
        contexts: &[PageContext<'_>],
        process_users: &HashMap<usize, ProcessUsers>,
        ranked: Vec<PageRankedRow>,
        emit: &mut impl FnMut(Value) -> bool,
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
                let source = self.reader.open_segment(context.source)?;
                resolved_dictionary(&source, &ids)
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
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: ranked.staged.ordinal,
                },
                dictionary,
                process_users
                    .get(&context.context_index)
                    .ok_or(ApiError::BadCursor)?,
                self.text,
            )?;
            if cancelled() || !emit(value) {
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
        emit: &mut impl FnMut(Value) -> bool,
    ) {
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
        let _connected = emit(json!({
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
        }));
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
        let mut facts = HashMap::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                let facts = self.page_facts(&section.logical_name, &self.anchor, &mut facts)?;
                contexts.push(PageContext {
                    context_index: layout_index,
                    plan,
                    logical_name: &section.logical_name,
                    source: &self.anchor,
                    rows: rows_of(&self.anchor, plan.type_id).unwrap_or(0),
                    window: RowWindow::Untimed,
                    previous: None,
                    elapsed: None,
                    elapsed_by_partition: Arc::new(BTreeMap::new()),
                    sample_from: None,
                    sample_to: None,
                    order: page_order(&section.logical_name, plan, &self.by),
                    search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                        search_columns(&section.logical_name, plan, search)
                    }),
                    clock_ticks_per_second: facts.clock_ticks_per_second.value(),
                    block_size: facts.block_size.value(),
                });
                continue;
            };
            contexts.extend(self.timed_contexts(
                section,
                layout_index,
                plan,
                timestamp,
                cancelled,
                &mut facts,
            )?);
        }
        Ok(contexts)
    }

    fn page_facts(
        &self,
        logical_name: &str,
        source_ref: &SegmentRef,
        cached: &mut HashMap<i64, PageFacts>,
    ) -> Result<PageFacts, ApiError> {
        if !matches!(
            logical_name,
            "os_process" | "pg_stat_statements" | "pg_store_plans"
        ) {
            return Ok(PageFacts::default());
        }
        let mut facts = cached.get(&source_ref.id()).copied().unwrap_or_default();
        let present = if logical_name == "os_process" {
            facts.clock_ticks_per_second != CachedFact::Unknown
        } else {
            facts.block_size != CachedFact::Unknown
        };
        if present {
            return Ok(facts);
        }
        let source = self.reader.open_segment(source_ref)?;
        if logical_name == "os_process" {
            facts.clock_ticks_per_second =
                clock_ticks_per_second(&source)?.map_or(CachedFact::Absent, CachedFact::Value);
        } else {
            facts.block_size =
                postgres_block_size(&source)?.map_or(CachedFact::Absent, CachedFact::Value);
        }
        cached.insert(source_ref.id(), facts);
        Ok(facts)
    }

    fn first_match_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<PageContext<'a>>, ApiError> {
        let mut contexts = Vec::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                return Err(ApiError::BadFilter("first_match".to_owned()));
            };
            let mut moments = Vec::new();
            for source_index in 0..self.source_count() {
                let source_ref = self.source_for(source_index).ok_or(ApiError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.reader.open_segment(source_ref)?;
                if let Some(moment) = Self::moments(&source, plan, timestamp, self.at, cancelled)? {
                    moments.push((source_index, moment.current));
                }
            }
            let Some(current) = moments.iter().map(|(_source, at)| *at).max() else {
                continue;
            };
            for (source_index, at) in moments {
                if at != current {
                    continue;
                }
                let source = self.source_for(source_index).ok_or(ApiError::BadCursor)?;
                contexts.push(PageContext {
                    context_index: timed_context_index(
                        layout_index,
                        source_index,
                        self.source_count(),
                    ),
                    plan,
                    logical_name: &section.logical_name,
                    source,
                    rows: rows_of(source, plan.type_id).unwrap_or(0),
                    window: RowWindow::Shared { timestamp, current },
                    previous: None,
                    elapsed: None,
                    elapsed_by_partition: Arc::new(BTreeMap::new()),
                    sample_from: None,
                    sample_to: Some(current),
                    order: None,
                    search_columns: Vec::new(),
                    clock_ticks_per_second: None,
                    block_size: None,
                });
            }
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
        facts: &mut HashMap<i64, PageFacts>,
    ) -> Result<Vec<PageContext<'a>>, ApiError> {
        let Some(moments) = self.shared_moments(plan, timestamp, cancelled)? else {
            return Ok(Vec::new());
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut previous = Readings::new();
        if let Some(before) = moments.previous {
            for source_index in (0..self.source_count()).rev() {
                let source_ref = self.source_for(source_index).ok_or(ApiError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_some() {
                    let source = self.reader.open_segment(source_ref)?;
                    previous.extend(Self::collect(
                        &source,
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
        for source_index in 0..self.source_count() {
            let source_ref = self.source_for(source_index).ok_or(ApiError::BadCursor)?;
            if rows_of(source_ref, plan.type_id).is_none() {
                continue;
            }
            let source = self.reader.open_segment(source_ref)?;
            if Self::moments(&source, plan, timestamp, moments.current, cancelled)?
                .is_none_or(|source_moments| source_moments.current != moments.current)
            {
                continue;
            }
            let facts = self.page_facts(&section.logical_name, source_ref, facts)?;
            contexts.push(PageContext {
                context_index: timed_context_index(layout_index, source_index, self.source_count()),
                plan,
                logical_name: &section.logical_name,
                source: source_ref,
                rows: rows_of(source_ref, plan.type_id).unwrap_or(0),
                window: RowWindow::Shared {
                    timestamp,
                    current: moments.current,
                },
                previous: Some(Arc::clone(&previous)),
                elapsed,
                elapsed_by_partition: Arc::new(BTreeMap::new()),
                sample_from: moments.previous,
                sample_to: Some(moments.current),
                order: order.clone(),
                search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                    search_columns(&section.logical_name, plan, search)
                }),
                clock_ticks_per_second: facts.clock_ticks_per_second.value(),
                block_size: facts.block_size.value(),
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
        let anchor = self.reader.open_segment(&self.anchor)?;
        let here = Self::moments(&anchor, plan, timestamp, self.at, cancelled)?;
        drop(anchor);
        let current = if let Some(here) = here {
            Some(here.current)
        } else {
            let mut fallback = None;
            for source_ref in &self.prior_sources {
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.reader.open_segment(source_ref)?;
                if let Some(moments) = Self::moments(&source, plan, timestamp, self.at, cancelled)?
                {
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
            for source_index in 0..self.source_count() {
                let source_ref = self.source_for(source_index).ok_or(ApiError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.reader.open_segment(source_ref)?;
                if let Some(moments) = Self::moments(&source, plan, timestamp, before, cancelled)? {
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
        let selected = self.selected_partitions(section, cancelled);
        if cancelled() {
            return Ok(Vec::new());
        }
        let mut contexts = Vec::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || plan.timestamp.is_none() {
                continue;
            }
            let rate_state =
                self.partition_rate_state(section, layout_index, plan, &selected, spec, cancelled)?;
            for source in std::iter::once(PartitionSource::Current).chain(
                (0..self.relation_predecessors.len())
                    .rev()
                    .map(PartitionSource::Earlier),
            ) {
                if let Some(context) = self.partitioned_context(
                    section,
                    layout_index,
                    plan,
                    source,
                    &selected,
                    &rate_state,
                    spec,
                ) {
                    contexts.push(context);
                }
            }
        }
        Ok(contexts)
    }

    fn selected_partitions(
        &self,
        section: &SectionPlans,
        cancelled: &impl Fn() -> bool,
    ) -> BTreeMap<IdentityCell, SelectedPartition> {
        let source_by_id = std::iter::once((self.anchor.id(), PartitionSource::Current))
            .chain(
                self.relation_predecessors
                    .iter()
                    .enumerate()
                    .map(|(index, source)| (source.id(), PartitionSource::Earlier(index))),
            )
            .collect::<HashMap<_, _>>();
        let mut selected = BTreeMap::<IdentityCell, SelectedPartition>::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            if plan.timestamp.is_none() {
                continue;
            }
            for ((type_id, partition), retained) in &self.relation_moments {
                if *type_id != plan.type_id {
                    continue;
                }
                let Some(current) = retained
                    .current
                    .as_ref()
                    .and_then(|moment| located_moment(moment, &source_by_id))
                else {
                    continue;
                };
                let moments = PartitionMoments {
                    current,
                    previous: retained
                        .previous
                        .as_ref()
                        .and_then(|moment| located_moment(moment, &source_by_id)),
                };
                let candidate = SelectedPartition {
                    layout_index,
                    type_id: plan.type_id,
                    moments,
                };
                let replace = selected.get(partition).is_none_or(|chosen| {
                    (candidate.moments.current.at, candidate.type_id)
                        > (chosen.moments.current.at, chosen.type_id)
                });
                if replace {
                    selected.insert(partition.clone(), candidate);
                }
            }
        }
        selected
    }

    fn partition_rate_state(
        &self,
        section: &SectionPlans,
        layout_index: usize,
        plan: &Plan,
        selected: &BTreeMap<IdentityCell, SelectedPartition>,
        spec: SnapshotViewSpec,
        cancelled: &impl Fn() -> bool,
    ) -> Result<PartitionRateState, ApiError> {
        let Some(timestamp) = plan.timestamp else {
            return Err(ApiError::BadCursor);
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut previous = Readings::new();
        let mut elapsed_by_partition = BTreeMap::new();
        let mut sample_from: Option<i64> = None;
        let mut sample_to: Option<i64> = None;
        let mut before_by_source =
            BTreeMap::<i64, (&SegmentRef, BTreeMap<IdentityCell, i64>)>::new();
        for (partition, selection) in selected {
            if selection.layout_index != layout_index {
                continue;
            }
            sample_to = Some(sample_to.map_or(selection.moments.current.at, |chosen| {
                chosen.max(selection.moments.current.at)
            }));
            let Some(before) = selection.moments.previous.as_ref() else {
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
            for source in &before.sources {
                if let Some(reference) = self.partition_source(*source) {
                    before_by_source
                        .entry(reference.id())
                        .or_insert_with(|| (reference, BTreeMap::new()))
                        .1
                        .insert(partition.clone(), before.at);
                }
            }
            elapsed_by_partition.insert(partition.clone(), elapsed);
            sample_from = Some(sample_from.map_or(before.at, |chosen| chosen.min(before.at)));
        }
        for (_segment_id, (before_source_ref, partitions)) in before_by_source {
            let before_source = self.reader.open_segment(before_source_ref)?;
            previous.extend(Self::collect_partitions(
                &before_source,
                plan,
                timestamp,
                spec.temporal_partition,
                &partitions,
                &order_columns,
                cancelled,
            )?);
        }
        Ok(PartitionRateState {
            previous: Arc::new(previous),
            elapsed_by_partition: Arc::new(elapsed_by_partition),
            sample_from,
            sample_to,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the selected layout, source, and partition contract define one context"
    )]
    fn partitioned_context<'a>(
        &'a self,
        section: &'a SectionPlans,
        layout_index: usize,
        plan: &'a Plan,
        source_kind: PartitionSource,
        selected: &BTreeMap<IdentityCell, SelectedPartition>,
        rate_state: &PartitionRateState,
        spec: SnapshotViewSpec,
    ) -> Option<PageContext<'a>> {
        let timestamp = plan.timestamp?;
        if !selected.values().any(|selection| {
            selection.layout_index == layout_index
                && selection.moments.current.sources.contains(&source_kind)
        }) {
            return None;
        }
        let source_ref = self.partition_source(source_kind)?;
        let order = page_order(&section.logical_name, plan, &self.by);
        let mut current = BTreeMap::new();
        for (partition, selection) in selected {
            if selection.layout_index != layout_index
                || !selection.moments.current.sources.contains(&source_kind)
            {
                continue;
            }
            current.insert(partition.clone(), selection.moments.current.at);
        }
        Some(PageContext {
            context_index: partition_context_index(
                layout_index,
                source_kind,
                self.relation_predecessors.len(),
            ),
            plan,
            logical_name: &section.logical_name,
            source: source_ref,
            rows: rows_of(source_ref, plan.type_id).unwrap_or(0),
            window: RowWindow::Partitioned {
                timestamp,
                column: spec.temporal_partition,
                current,
            },
            previous: Some(Arc::clone(&rate_state.previous)),
            elapsed: None,
            elapsed_by_partition: Arc::clone(&rate_state.elapsed_by_partition),
            sample_from: rate_state.sample_from,
            sample_to: rate_state.sample_to,
            order,
            search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                search_columns(&section.logical_name, plan, search)
            }),
            clock_ticks_per_second: None,
            block_size: None,
        })
    }

    const fn source_count(&self) -> usize {
        self.prior_sources.len() + 1
    }

    fn source_for(&self, source_index: usize) -> Option<&SegmentRef> {
        if source_index == 0 {
            Some(&self.anchor)
        } else {
            self.prior_sources.get(source_index - 1)
        }
    }

    fn partition_source(&self, source: PartitionSource) -> Option<&SegmentRef> {
        match source {
            PartitionSource::Current => Some(&self.anchor),
            PartitionSource::Earlier(index) => self.relation_predecessors.get(index),
        }
    }

    fn collect_partitions(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        partition_column: &'static str,
        partitions: &BTreeMap<IdentityCell, i64>,
        extra_columns: &[&'static str],
        cancelled: &impl Fn() -> bool,
    ) -> Result<Readings, ApiError> {
        #[cfg(test)]
        PARTITION_PREDECESSOR_VISITS.set(PARTITION_PREDECESSOR_VISITS.get().saturating_add(1));
        let mut collected = BTreeMap::new();
        let mut counters = projected_rate_columns(plan);
        if plan.contract.column("starttime").is_some() {
            counters.push("starttime");
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
            let Some(partition) = row.get(partition_column).map(identity_cell) else {
                return true;
            };
            let Some(at) = partitions.get(&partition) else {
                return true;
            };
            if row_timestamp(&row, timestamp) != Some(*at) {
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
        process_users: &HashMap<usize, ProcessUsers>,
        cursor: SnapshotCursor,
    ) -> Result<PageRankedRow, ApiError> {
        let context = contexts
            .iter()
            .find(|context| context.context_index == cursor.context_index)
            .ok_or(ApiError::BadCursor)?;
        if cursor.ordinal >= context.rows {
            return Err(ApiError::BadCursor);
        }
        let source = self.reader.open_segment(context.source)?;
        let mut stored = None;
        source.visit_rows(
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
        let dictionary = page_dictionary(
            &source,
            context,
            std::slice::from_ref(&(ordinal, row.clone())),
        )?;
        self.page_candidate(
            context,
            process_users
                .get(&context.context_index)
                .ok_or(ApiError::BadCursor)?,
            ordinal,
            row,
            &dictionary,
        )
        .ok_or(ApiError::BadCursor)
    }

    fn scan_page(
        &self,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let source = self.reader.open_segment(context.source)?;
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
            source.visit_rows(
                context.plan.type_id,
                &context.plan.projection,
                0,
                usize::MAX,
                |ordinal, row| {
                    self.rank_page_row(
                        context,
                        process_users,
                        anchor,
                        page,
                        eligible,
                        &dictionary,
                        ordinal,
                        row,
                    );
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
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            0,
            usize::MAX,
            |ordinal, row| {
                chunk.push((ordinal, row));
                if chunk.len() == SNAPSHOT_CHUNK_ROWS
                    && let Err(error) = self.rank_page_chunk(
                        &source,
                        context,
                        process_users,
                        anchor,
                        page,
                        eligible,
                        &mut chunk,
                    )
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
            self.rank_page_chunk(
                &source,
                context,
                process_users,
                anchor,
                page,
                eligible,
                &mut chunk,
            )?;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "page ranking keeps segment-local user resolution beside existing paging state"
    )]
    fn rank_page_chunk(
        &self,
        source: &Segment,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        chunk: &mut Vec<(u64, Row)>,
    ) -> Result<(), ApiError> {
        let dictionary = page_dictionary(source, context, chunk)?;
        for (ordinal, row) in chunk.drain(..) {
            self.rank_page_row(
                context,
                process_users,
                anchor,
                page,
                eligible,
                &dictionary,
                ordinal,
                row,
            );
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
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        dictionary: &Dictionary,
        ordinal: u64,
        row: Row,
    ) {
        let Some(candidate) = self.page_candidate(context, process_users, ordinal, row, dictionary)
        else {
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
        process_users: &ProcessUsers,
        ordinal: u64,
        row: Row,
        dictionary: &Dictionary,
    ) -> Option<PageRankedRow> {
        if !context.window.matches(&row) || !context.plan.matches(&row, dictionary) {
            return None;
        }
        let identity = identity_of(context.plan, &row)?;
        if self.search.as_ref().is_some_and(|search| {
            !search.matches_all(|clause| {
                if matches!(clause.value, SearchValue::Quantity(_)) {
                    result_search_matches(context, &row, &identity, clause)
                } else {
                    search_clause_matches(
                        context.logical_name,
                        context.plan,
                        &row,
                        dictionary,
                        Some(process_users),
                        clause,
                    )
                }
            })
        }) {
            return None;
        }
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
        let counters = projected_rate_columns(plan);
        let mut counters = counters;
        if plan.contract.column("starttime").is_some() {
            counters.push("starttime");
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
        segment.visit_rows(plan.type_id, &projection, 0, usize::MAX, |_ordinal, row| {
            if cancelled() {
                return false;
            }
            if row_timestamp(&row, timestamp) != Some(at) {
                return true;
            }
            let Some(key) = identity_of(plan, &row) else {
                return true;
            };
            let stored = counters
                .iter()
                .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
                .collect();
            collected.insert(key, stored);
            true
        })?;
        if cancelled() {
            collected.clear();
            return Ok(collected);
        }
        Ok(collected)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one row renderer combines physical values, rates, coordinates, and segment-local reference data"
    )]
    fn row_record(
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        coordinate: RowCoordinate,
        dictionary: &Dictionary,
        process_users: &ProcessUsers,
        text_limit: Option<usize>,
    ) -> Result<Value, ApiError> {
        let stamped = plan.timestamp.and_then(|column| row_timestamp(row, column));
        let mut values = Vec::with_capacity(plan.fields.len());
        for field in &plan.fields {
            let Some(column) = field.column else {
                let uid_column = match field.name.as_str() {
                    "user" => Some("uid"),
                    "effective_user" => Some("euid"),
                    _ => None,
                };
                values.push(
                    uid_column
                        .and_then(|column| process_users.for_row(row, column))
                        .map_or(Value::Null, |name| Value::String(name.to_owned())),
                );
                continue;
            };
            let stored = row.get(column);
            let is_rate = plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
            let exact_plan_calls =
                matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001) && field.name == "calls";
            if is_rate && !exact_plan_calls {
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
            "ordinal": coordinate.ordinal.to_string(),
            "segment_id": coordinate.segment_id.to_string(),
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

fn page_dictionary(
    segment: &Segment,
    context: &PageContext<'_>,
    rows: &[(u64, Row)],
) -> Result<Dictionary, ApiError> {
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
    resolved_dictionary(segment, &ids)
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

fn search_columns(logical_name: &str, plan: &Plan, search: &StructuredSearch) -> Vec<&'static str> {
    let mut columns = Vec::new();
    for clause in &search.clauses {
        let mut wanted = search_clause_columns(logical_name, plan, clause.key);
        if let Some((_clause, field)) = search
            .result_clauses(logical_name)
            .find(|(candidate, _field)| candidate.key == clause.key)
        {
            wanted.extend(
                field
                    .dependencies
                    .iter()
                    .filter_map(|name| plan.contract.column(name).map(|column| column.name)),
            );
        }
        for column in wanted {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
    }
    columns
}

fn clock_ticks_per_second(segment: &Segment) -> Result<Option<u128>, ApiError> {
    for (type_id, _rows) in segment.sections() {
        if logical_section_name(type_id) != Some("instance_metadata") {
            continue;
        }
        let Some(column) =
            contract(type_id).and_then(|layout| layout.column("clock_ticks_per_sec"))
        else {
            continue;
        };
        let mut stored = None;
        segment.visit_rows(type_id, &[column.name], 0, 1, |_ordinal, row| {
            stored = row
                .get(column.name)
                .and_then(ordered_cell)
                .and_then(|value| {
                    let OrderedNumber::Integer(value) = value else {
                        return None;
                    };
                    u128::try_from(value).ok().filter(|value| *value > 0)
                });
            false
        })?;
        if stored.is_some() {
            return Ok(stored);
        }
    }
    Ok(None)
}

fn postgres_block_size(segment: &Segment) -> Result<Option<u128>, ApiError> {
    for (type_id, _rows) in segment.sections() {
        if logical_section_name(type_id) != Some("pg_settings") {
            continue;
        }
        let Some(layout) = contract(type_id) else {
            continue;
        };
        let (Some(name), Some(setting)) = (layout.column("name"), layout.column("setting")) else {
            continue;
        };
        let mut candidates = Vec::new();
        let mut ids = HashSet::new();
        segment.visit_rows(
            type_id,
            &[name.name, setting.name],
            0,
            usize::MAX,
            |_ordinal, row| {
                let (Some(Cell::StrId(name_id)), Some(Cell::StrId(setting_id))) =
                    (row.get(name.name), row.get(setting.name))
                else {
                    return true;
                };
                ids.insert(*name_id);
                ids.insert(*setting_id);
                candidates.push((*name_id, *setting_id));
                true
            },
        )?;
        let dictionary = resolved_dictionary(segment, &ids)?;
        for (name_id, setting_id) in candidates {
            let Some(Resolved::Str(name)) = dictionary.resolve(name_id) else {
                continue;
            };
            if name != b"block_size" {
                continue;
            }
            let Some(Resolved::Str(setting)) = dictionary.resolve(setting_id) else {
                continue;
            };
            let Ok(setting) = std::str::from_utf8(setting) else {
                continue;
            };
            if let Ok(value) = setting.parse::<u128>()
                && value > 0
            {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

#[expect(
    variant_size_differences,
    reason = "exact quantity factors stay inline so per-row comparisons do not allocate"
)]
enum SearchMetricValue {
    Exact {
        numerator: ProductFactors,
        denominator: ProductFactors,
    },
    Float(f64),
}

#[derive(Clone, Copy)]
struct ProductFactors {
    values: [u128; 4],
    len: usize,
}

impl ProductFactors {
    const fn one(value: u128) -> Self {
        Self {
            values: [value, 1, 1, 1],
            len: 1,
        }
    }

    const fn two(first: u128, second: u128) -> Self {
        Self {
            values: [first, second, 1, 1],
            len: 2,
        }
    }

    fn push(mut self, value: u128) -> Option<Self> {
        let slot = self.values.get_mut(self.len)?;
        *slot = value;
        self.len += 1;
        Some(self)
    }

    fn as_slice(&self) -> &[u128] {
        &self.values[..self.len]
    }
}

impl SearchMetricValue {
    #[expect(
        clippy::cast_precision_loss,
        reason = "stored floating metrics must be scaled in their native f64 domain"
    )]
    fn scale(self, numerator_scale: u128, denominator_scale: u128) -> Option<Self> {
        match self {
            Self::Exact {
                numerator,
                denominator,
            } => Some(Self::Exact {
                numerator: numerator.push(numerator_scale)?,
                denominator: denominator.push(denominator_scale)?,
            }),
            Self::Float(value) => {
                let scaled = value * numerator_scale as f64 / denominator_scale as f64;
                scaled.is_finite().then_some(Self::Float(scaled))
            }
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "stored floating metrics are compared to the closest f64 threshold"
    )]
    fn matches(&self, operator: SearchOperator, quantity: &Quantity) -> bool {
        let ordering = match self {
            Self::Exact {
                numerator,
                denominator,
            } => {
                let (Some(left), Some(right)) = (
                    numerator.push(quantity.denominator),
                    denominator.push(quantity.numerator),
                ) else {
                    return false;
                };
                compare_products(left.as_slice(), right.as_slice())
            }
            Self::Float(value) => {
                let threshold = quantity.numerator as f64 / quantity.denominator as f64;
                if !value.is_finite() || !threshold.is_finite() {
                    return false;
                }
                value.total_cmp(&threshold)
            }
        };
        match operator {
            SearchOperator::Greater => ordering == Ordering::Greater,
            SearchOperator::Less => ordering == Ordering::Less,
            SearchOperator::Colon => false,
        }
    }
}

fn result_search_matches(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    clause: &SearchClause,
) -> bool {
    let SearchValue::Quantity(quantity) = &clause.value else {
        return false;
    };
    let Some(field) = result_field(context.logical_name, clause.key) else {
        return false;
    };
    search_metric(context, row, identity, field.metric)
        .is_some_and(|value| value.matches(clause.operator, quantity))
}

fn search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match context.logical_name {
        "os_process" => process_search_metric(context, row, identity, metric),
        "pg_stat_statements" | "pg_store_plans" => {
            postgres_search_metric(context, row, identity, metric)
        }
        _ => None,
    }
}

fn process_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match metric {
        "rss" => gauge_metric(row, "rmem_kb", 1_024, 1),
        "vsz" => gauge_metric(row, "vmem_kb", 1_024, 1),
        "swap" => gauge_metric(row, "vswap_kb", 1_024, 1),
        "threads" => gauge_metric(row, "num_threads", 1, 1),
        "cpu_cores" | "user_cpu_cores" | "system_cpu_cores" => {
            let columns: &[&str] = match metric {
                "user_cpu_cores" => &["utime"],
                "system_cpu_cores" => &["stime"],
                _ => &["utime", "stime"],
            };
            let ticks = context.clock_ticks_per_second?;
            rate_metric(context, row, identity, columns, 1_000_000, ticks)
        }
        "disk_read_rate" => rate_metric(context, row, identity, &["read_bytes"], 1_000_000, 1),
        "disk_write_rate" => rate_metric(context, row, identity, &["write_bytes"], 1_000_000, 1),
        "logical_read_rate" => rate_metric(context, row, identity, &["rchar"], 1_000_000, 1),
        "logical_write_rate" => rate_metric(context, row, identity, &["wchar"], 1_000_000, 1),
        "read_syscall_rate" => rate_metric(context, row, identity, &["syscr"], 1_000_000, 1),
        "write_syscall_rate" => rate_metric(context, row, identity, &["syscw"], 1_000_000, 1),
        "major_fault_rate" => rate_metric(context, row, identity, &["majflt"], 1_000_000, 1),
        "minor_fault_rate" => rate_metric(context, row, identity, &["minflt"], 1_000_000, 1),
        "context_switch_rate" => {
            rate_metric(context, row, identity, &["nvcsw", "nivcsw"], 1_000_000, 1)
        }
        "voluntary_context_switch_rate" => {
            rate_metric(context, row, identity, &["nvcsw"], 1_000_000, 1)
        }
        "involuntary_context_switch_rate" => {
            rate_metric(context, row, identity, &["nivcsw"], 1_000_000, 1)
        }
        "run_delay" => rate_metric(context, row, identity, &["rundelay_ns"], 1, 1),
        "block_io_delay" => rate_metric(
            context,
            row,
            identity,
            &["blkdelay_ticks"],
            1_000_000_000,
            context.clock_ticks_per_second?,
        ),
        _ => None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the fixed public PostgreSQL metric adapter stays exhaustive and fork-transparent"
)]
fn postgres_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    let execution = preferred_column(context.plan, "total_exec_time", "total_time");
    let mean = preferred_column(context.plan, "mean_exec_time", "mean_time");
    let stddev = preferred_column(context.plan, "stddev_exec_time", "stddev_time");
    if metric == "calls" {
        return gauge_metric(row, "calls", 1, 1);
    }
    let direct_rate = match metric {
        "call_rate" => Some("calls"),
        "exec_time_rate" => execution,
        "row_rate" => Some("rows"),
        "plan_rate" => Some("plans"),
        "planning_time_rate" => Some("total_plan_time"),
        "slow_call_rate" => Some("slow_log_calls"),
        "shared_read_time_rate" => {
            preferred_column(context.plan, "shared_blk_read_time", "blk_read_time")
        }
        "shared_write_time_rate" => {
            preferred_column(context.plan, "shared_blk_write_time", "blk_write_time")
        }
        "local_read_time_rate" => Some("local_blk_read_time"),
        "local_write_time_rate" => Some("local_blk_write_time"),
        "temp_read_time_rate" => Some("temp_blk_read_time"),
        "temp_write_time_rate" => Some("temp_blk_write_time"),
        "wal_rate" => Some("wal_bytes"),
        _ => None,
    };
    if let Some(column) = direct_rate {
        return rate_metric(context, row, identity, &[column], 1_000_000, 1);
    }
    let buffer_column = match metric {
        "shared_buffer_hit_rate" => Some("shared_blks_hit"),
        "shared_buffer_read_rate" => Some("shared_blks_read"),
        "shared_buffer_dirty_rate" => Some("shared_blks_dirtied"),
        "shared_buffer_write_rate" => Some("shared_blks_written"),
        "local_buffer_hit_rate" => Some("local_blks_hit"),
        "local_buffer_read_rate" => Some("local_blks_read"),
        "local_buffer_dirty_rate" => Some("local_blks_dirtied"),
        "local_buffer_write_rate" => Some("local_blks_written"),
        "temp_buffer_read_rate" => Some("temp_blks_read"),
        "temp_buffer_write_rate" => Some("temp_blks_written"),
        _ => None,
    };
    if let Some(column) = buffer_column {
        return rate_metric(context, row, identity, &[column], 1_000_000, 1)
            .and_then(|value| value.scale(context.block_size?, 1));
    }
    match metric {
        "mean_exec" => counter_ratio_metric(context, row, identity, &[execution?], &["calls"], 1),
        "rows_per_call" => counter_ratio_metric(context, row, identity, &["rows"], &["calls"], 1),
        "wal_per_call" => {
            counter_ratio_metric(context, row, identity, &["wal_bytes"], &["calls"], 1)
        }
        "planning_share" => counter_ratio_metric(
            context,
            row,
            identity,
            &["total_plan_time"],
            &["total_plan_time", execution?],
            100,
        ),
        "buffer_hit" => counter_ratio_metric(
            context,
            row,
            identity,
            &["shared_blks_hit"],
            &["shared_blks_hit", "shared_blks_read"],
            100,
        ),
        "buffer_per_call" => counter_ratio_metric(
            context,
            row,
            identity,
            &[
                "shared_blks_hit",
                "shared_blks_read",
                "shared_blks_dirtied",
                "shared_blks_written",
                "local_blks_hit",
                "local_blks_read",
                "local_blks_dirtied",
                "local_blks_written",
                "temp_blks_read",
                "temp_blks_written",
            ],
            &["calls"],
            context.block_size?,
        ),
        "exec_cv" => value_ratio_metric(row, &[stddev?], &[mean?], 1),
        "min_exec_since_reset" => gauge_metric(
            row,
            preferred_column(context.plan, "min_exec_time", "min_time")?,
            1,
            1,
        ),
        "max_exec_since_reset" => gauge_metric(
            row,
            preferred_column(context.plan, "max_exec_time", "max_time")?,
            1,
            1,
        ),
        "mean_exec_since_reset" => gauge_metric(row, mean?, 1, 1),
        "stddev_exec_since_reset" => gauge_metric(row, stddev?, 1, 1),
        _ => None,
    }
}

fn preferred_column(
    plan: &Plan,
    preferred: &'static str,
    fallback: &'static str,
) -> Option<&'static str> {
    plan.contract
        .column(preferred)
        .map(|column| column.name)
        .or_else(|| plan.contract.column(fallback).map(|column| column.name))
}

fn gauge_metric(
    row: &Row,
    column: &'static str,
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    ordered_metric(
        ordered_cell(row.get(column)?)?,
        numerator_scale,
        denominator_scale,
    )
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating counters preserve their recorded f64 arithmetic"
)]
fn rate_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    columns: &[&'static str],
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    let elapsed = u128::try_from(context.elapsed_for(row)?)
        .ok()
        .filter(|value| *value > 0)?;
    let before = context.predecessor(row, identity)?;
    let delta = counter_sum(row, before, columns, false)?;
    match delta {
        OrderedNumber::Integer(value) if value >= 0 => Some(SearchMetricValue::Exact {
            numerator: ProductFactors::two(u128::try_from(value).ok()?, numerator_scale),
            denominator: ProductFactors::two(elapsed, denominator_scale),
        }),
        OrderedNumber::Float(value) => {
            let converted =
                value * numerator_scale as f64 / elapsed as f64 / denominator_scale as f64;
            converted
                .is_finite()
                .then_some(SearchMetricValue::Float(converted))
        }
        OrderedNumber::Integer(_) => None,
    }
}

fn counter_ratio_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    numerator_columns: &[&'static str],
    denominator_columns: &[&'static str],
    scale: u128,
) -> Option<SearchMetricValue> {
    let _elapsed = context.elapsed_for(row)?;
    let before = context.predecessor(row, identity)?;
    let numerator = counter_sum(row, before, numerator_columns, false)?;
    let denominator = counter_sum(row, before, denominator_columns, false)?;
    ratio_metric(numerator, denominator, scale)
}

fn value_ratio_metric(
    row: &Row,
    numerator_columns: &[&'static str],
    denominator_columns: &[&'static str],
    scale: u128,
) -> Option<SearchMetricValue> {
    ratio_metric(
        value_sum(row, numerator_columns)?,
        value_sum(row, denominator_columns)?,
        scale,
    )
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating counters preserve their recorded f64 arithmetic"
)]
fn ratio_metric(
    numerator: OrderedNumber,
    denominator: OrderedNumber,
    scale: u128,
) -> Option<SearchMetricValue> {
    match (numerator, denominator) {
        (OrderedNumber::Integer(numerator), OrderedNumber::Integer(denominator))
            if numerator >= 0 && denominator > 0 =>
        {
            Some(SearchMetricValue::Exact {
                numerator: ProductFactors::two(u128::try_from(numerator).ok()?, scale),
                denominator: ProductFactors::one(u128::try_from(denominator).ok()?),
            })
        }
        (numerator, denominator) => {
            let denominator = denominator.as_f64();
            let ratio = numerator.as_f64() / denominator * scale as f64;
            (denominator > 0.0 && ratio.is_finite()).then_some(SearchMetricValue::Float(ratio))
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating gauges preserve their recorded f64 value"
)]
fn ordered_metric(
    value: OrderedNumber,
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    match value {
        OrderedNumber::Integer(value) if value >= 0 => Some(SearchMetricValue::Exact {
            numerator: ProductFactors::two(u128::try_from(value).ok()?, numerator_scale),
            denominator: ProductFactors::one(denominator_scale),
        }),
        OrderedNumber::Float(value) => {
            let converted = value * numerator_scale as f64 / denominator_scale as f64;
            converted
                .is_finite()
                .then_some(SearchMetricValue::Float(converted))
        }
        OrderedNumber::Integer(_) => None,
    }
}

fn compare_products(left: &[u128], right: &[u128]) -> Ordering {
    if let (Some(left), Some(right)) = (fixed_product_limbs(left), fixed_product_limbs(right)) {
        left.len.cmp(&right.len).then_with(|| {
            left.digits[..left.len]
                .iter()
                .rev()
                .cmp(right.digits[..right.len].iter().rev())
        })
    } else {
        let left = heap_product_limbs(left);
        let right = heap_product_limbs(right);
        left.len().cmp(&right.len()).then_with(|| left.cmp(&right))
    }
}

struct FixedProduct {
    digits: [u32; 16],
    len: usize,
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each cast selects one base-2^32 limb after shifting"
)]
fn fixed_product_limbs(factors: &[u128]) -> Option<FixedProduct> {
    if factors.len() > 4 {
        return None;
    }
    let mut product = FixedProduct {
        digits: [0; 16],
        len: 1,
    };
    product.digits[0] = 1;
    for factor in factors {
        let factor_digits = [
            *factor as u32,
            (*factor >> 32) as u32,
            (*factor >> 64) as u32,
            (*factor >> 96) as u32,
        ];
        let mut result_digits = [0_u32; 16];
        for left_index in 0..product.len {
            let left = product.digits[left_index];
            let mut carry = 0_u64;
            for (right_index, right) in factor_digits.iter().copied().enumerate() {
                let index = left_index + right_index;
                let slot = result_digits.get_mut(index)?;
                let value = u64::from(*slot) + u64::from(left) * u64::from(right) + carry;
                *slot = value as u32;
                carry = value >> 32;
            }
            let mut index = left_index + factor_digits.len();
            while carry > 0 {
                let slot = result_digits.get_mut(index)?;
                let value = u64::from(*slot) + carry;
                *slot = value as u32;
                carry = value >> 32;
                index += 1;
            }
        }
        let mut len = (product.len + factor_digits.len()).min(result_digits.len());
        while len > 1 && result_digits[len - 1] == 0 {
            len -= 1;
        }
        product = FixedProduct {
            digits: result_digits,
            len,
        };
    }
    Some(product)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each cast selects one base-2^32 limb after shifting"
)]
fn heap_product_limbs(factors: &[u128]) -> Vec<u32> {
    let mut product = vec![1_u32];
    for factor in factors {
        let digits = [
            *factor as u32,
            (*factor >> 32) as u32,
            (*factor >> 64) as u32,
            (*factor >> 96) as u32,
        ];
        let mut multiplied = vec![0_u32; product.len() + digits.len()];
        for (left_index, left) in product.iter().copied().enumerate() {
            let mut carry = 0_u64;
            for (right_index, right) in digits.iter().copied().enumerate() {
                let index = left_index + right_index;
                let value =
                    u64::from(multiplied[index]) + u64::from(left) * u64::from(right) + carry;
                multiplied[index] = value as u32;
                carry = value >> 32;
            }
            let mut index = left_index + digits.len();
            while carry > 0 {
                let value = u64::from(multiplied[index]) + carry;
                multiplied[index] = value as u32;
                carry = value >> 32;
                index += 1;
                if index == multiplied.len() && carry > 0 {
                    multiplied.push(0);
                }
            }
        }
        while multiplied.len() > 1 && multiplied.last() == Some(&0) {
            multiplied.pop();
        }
        product = multiplied;
    }
    product.iter().rev().copied().collect()
}

fn search_clause_columns(logical_name: &str, plan: &Plan, key: &str) -> Vec<&'static str> {
    let Some(field) = search_fields(logical_name)
        .iter()
        .find(|field| field.key == key)
    else {
        return Vec::new();
    };
    let wanted: &[&str] = if logical_name == "pg_store_plans" && key == "query_id" {
        plan_statement_query_id_columns(plan.type_id)
    } else {
        field.columns
    };
    wanted
        .iter()
        .filter_map(|name| plan.contract.column(name).map(|column| column.name))
        .collect()
}

const fn plan_statement_query_id_columns(type_id: u32) -> &'static [&'static str] {
    match type_id {
        1_004_001 => &["queryid_stat_statements"],
        _ => &["queryid"],
    }
}

fn search_clause_matches(
    logical_name: &str,
    plan: &Plan,
    row: &Row,
    dictionary: &Dictionary,
    process_users: Option<&ProcessUsers>,
    clause: &SearchClause,
) -> bool {
    if logical_name == "pg_store_plans"
        && plan.type_id == 1_004_001
        && clause.key == "query_id"
        && matches!(&clause.value, SearchValue::Identifier(value) if value == "0")
    {
        return false;
    }
    if logical_name == "os_process" {
        let name_matches = |uid_column| {
            process_users
                .and_then(|users| users.for_row(row, uid_column))
                .is_some_and(|name| search_value_matches(name, &clause.value))
        };
        match clause.key {
            "user" => return name_matches("uid"),
            "effective_user" => return name_matches("euid"),
            "text" => {
                if name_matches("uid") || name_matches("euid") {
                    return true;
                }
                return ["comm", "cmdline"].iter().any(|column| {
                    row.get(column)
                        .and_then(|value| searchable_text(value, dictionary))
                        .is_some_and(|text| search_value_matches(&text, &clause.value))
                });
            }
            _ => {}
        }
    }
    let columns = if logical_name == "pg_store_plans" && clause.key == "query_id" {
        plan_statement_query_id_columns(plan.type_id)
    } else {
        clause.columns
    };
    columns.iter().any(|column| {
        row.get(column)
            .and_then(|value| searchable_text(value, dictionary))
            .is_some_and(|text| search_value_matches(&text, &clause.value))
    })
}

fn search_matches(
    logical_name: &str,
    plan: &Plan,
    row: &Row,
    dictionary: &Dictionary,
    process_users: Option<&ProcessUsers>,
    search: &StructuredSearch,
) -> bool {
    search.matches_member(|clause| {
        search_clause_matches(logical_name, plan, row, dictionary, process_users, clause)
    })
}

fn search_value_matches(text: &str, value: &SearchValue) -> bool {
    match value {
        SearchValue::Identifier(wanted) => text == wanted,
        SearchValue::Pattern(pattern) => pattern.matches(text),
        SearchValue::Quantity(_) => false,
    }
}
fn searchable_text<'a>(value: &Cell, dictionary: &'a Dictionary) -> Option<Cow<'a, str>> {
    match value {
        Cell::I16(value) => Some(Cow::Owned(value.to_string())),
        Cell::I32(value) => Some(Cow::Owned(value.to_string())),
        Cell::I64(value) | Cell::Ts(value) => Some(Cow::Owned(value.to_string())),
        Cell::U32(value) => Some(Cow::Owned(value.to_string())),
        Cell::U64(value) => Some(Cow::Owned(value.to_string())),
        Cell::F64(value) if value.is_finite() => Some(Cow::Owned(value.to_string())),
        Cell::Bool(value) => Some(Cow::Owned(value.to_string())),
        Cell::StrId(id) => dictionary
            .resolve(*id)
            .and_then(|resolved| std::str::from_utf8(stored_bytes(resolved)).ok())
            .map(Cow::Borrowed),
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
        let mut pattern_index = 0;
        let mut candidate_index = 0;
        let mut star = None;
        let mut retry = 0;
        while candidate_index < candidate.len() {
            let Some(character) = candidate
                .get(candidate_index..)
                .and_then(|remaining| remaining.chars().next())
            else {
                return false;
            };
            match self.0.get(pattern_index) {
                Some(GlobToken::Literal(wanted)) if unicode_char_equal(*wanted, character) => {
                    pattern_index += 1;
                    candidate_index += character.len_utf8();
                }
                Some(GlobToken::Any) => {
                    pattern_index += 1;
                    candidate_index += character.len_utf8();
                }
                Some(GlobToken::Star) => {
                    star = Some(pattern_index);
                    pattern_index += 1;
                    retry = candidate_index;
                }
                _ if let Some(star_index) = star => {
                    let Some(retry_character) = candidate
                        .get(retry..)
                        .and_then(|remaining| remaining.chars().next())
                    else {
                        return false;
                    };
                    retry += retry_character.len_utf8();
                    candidate_index = retry;
                    pattern_index = star_index + 1;
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

const fn partition_context_index(
    layout_index: usize,
    source: PartitionSource,
    predecessor_count: usize,
) -> usize {
    layout_index * (predecessor_count + 1)
        + match source {
            PartitionSource::Current => 0,
            PartitionSource::Earlier(index) => predecessor_count - index,
        }
}

const fn timed_context_index(
    layout_index: usize,
    source_index: usize,
    source_count: usize,
) -> usize {
    layout_index * source_count + source_index
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

fn snapshot_binding(request: &SnapshotRequest, search: Option<&StructuredSearch>) -> u64 {
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
    if let Some(search) = search {
        hash_part(&mut hash, b"search", search.canonical().as_bytes());
    }
    hash_part(&mut hash, b"first-match", &[u8::from(request.first_match)]);
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
                RelationGroup::Tablespace => b"tablespace",
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

struct RetainedMoment {
    at: i64,
    segment_ids: BTreeSet<i64>,
}

#[derive(Default)]
struct RetainedMoments {
    current: Option<RetainedMoment>,
    previous: Option<RetainedMoment>,
}

type RetainedRelationMoments = BTreeMap<(u32, IdentityCell), RetainedMoments>;

fn located_moment(
    retained: &RetainedMoment,
    source_by_id: &HashMap<i64, PartitionSource>,
) -> Option<LocatedMoment> {
    let sources = retained
        .segment_ids
        .iter()
        .filter_map(|segment_id| source_by_id.get(segment_id).copied())
        .collect::<BTreeSet<_>>();
    (!sources.is_empty()).then_some(LocatedMoment {
        at: retained.at,
        sources,
    })
}

fn relation_preceding(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segments: Vec<SegmentRef>,
    current: &Segment,
    sections: &[SectionPlans],
    filters: &[crate::route::Filter],
    at: i64,
) -> Result<(Vec<SegmentRef>, RetainedRelationMoments), ApiError> {
    let requested_datid = filters
        .iter()
        .find(|filter| filter.column == "datid")
        .and_then(|filter| filter.value.parse::<u32>().ok())
        .map(IdentityCell::U32);
    let mut moments = BTreeMap::<(u32, IdentityCell), RetainedMoments>::new();
    if let Some(datid) = requested_datid.as_ref() {
        for plan in partitioned_plans(sections) {
            if plan.applies() && plan.timestamp.is_some() {
                moments.entry((plan.type_id, datid.clone())).or_default();
            }
        }
    }
    scan_relation_moments(
        current,
        sections,
        requested_datid.as_ref(),
        requested_datid.is_none(),
        at,
        &mut moments,
    )?;
    let compatible = partitioned_plans(sections)
        .map(|plan| plan.type_id)
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
    candidates.sort_unstable_by_key(|candidate| Reverse((candidate.max_ts(), candidate.id())));
    for candidate in &candidates {
        if !moments.is_empty()
            && moments.values().all(|samples| {
                samples
                    .previous
                    .as_ref()
                    .is_some_and(|previous| candidate.max_ts() < previous.at)
            })
        {
            break;
        }
        let segment = reader.open_segment(candidate)?;
        scan_relation_moments(
            &segment,
            sections,
            requested_datid.as_ref(),
            requested_datid.is_none(),
            at,
            &mut moments,
        )?;
    }

    let mut retained = HashSet::new();
    for samples in moments.values() {
        for sample in [&samples.current, &samples.previous].into_iter().flatten() {
            for segment_id in &sample.segment_ids {
                if *segment_id != segment_ref.id() {
                    retained.insert(*segment_id);
                }
            }
        }
    }
    let mut selected = candidates
        .into_iter()
        .filter(|candidate| retained.contains(&candidate.id()))
        .collect::<Vec<_>>();
    selected.sort_unstable_by_key(|candidate| Reverse(candidate.id()));
    Ok((selected, moments))
}

fn partitioned_plans(sections: &[SectionPlans]) -> impl Iterator<Item = &Plan> {
    sections
        .iter()
        .filter(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_some())
        .flat_map(|section| section.plans.iter())
}

fn scan_relation_moments(
    segment: &Segment,
    sections: &[SectionPlans],
    requested_datid: Option<&IdentityCell>,
    discover: bool,
    at: i64,
    moments: &mut BTreeMap<(u32, IdentityCell), RetainedMoments>,
) -> Result<(), ApiError> {
    for plan in partitioned_plans(sections) {
        if !plan.applies() {
            continue;
        }
        let Some(timestamp) = plan.timestamp else {
            continue;
        };
        if segment.rows_of(plan.type_id).is_none() {
            continue;
        }
        #[cfg(test)]
        RELATION_MOMENT_VISITS.set(RELATION_MOMENT_VISITS.get().saturating_add(1));
        segment.visit_rows(
            plan.type_id,
            &[timestamp, "datid"],
            0,
            usize::MAX,
            |_ordinal, row| {
                let (Some(stored), Some(partition)) =
                    (row_timestamp(&row, timestamp), row.get("datid"))
                else {
                    return true;
                };
                let partition = identity_cell(partition);
                if requested_datid.is_some_and(|requested| requested != &partition) {
                    return true;
                }
                let key = (plan.type_id, partition);
                if discover {
                    moments.entry(key.clone()).or_default();
                }
                if stored <= at
                    && let Some(selected) = moments.get_mut(&key)
                {
                    record_retained_moment(
                        selected,
                        RetainedMoment {
                            at: stored,
                            segment_ids: BTreeSet::from([segment.id()]),
                        },
                    );
                }
                true
            },
        )?;
    }
    Ok(())
}

fn record_retained_moment(moments: &mut RetainedMoments, sample: RetainedMoment) {
    let Some(current_at) = moments.current.as_ref().map(|current| current.at) else {
        moments.current = Some(sample);
        return;
    };
    match sample.at.cmp(&current_at) {
        Ordering::Equal => {
            if let Some(current) = moments.current.as_mut() {
                current.segment_ids.extend(sample.segment_ids);
            }
        }
        Ordering::Greater => {
            moments.previous = moments.current.replace(sample);
        }
        Ordering::Less => match moments.previous.as_mut() {
            Some(previous) => match sample.at.cmp(&previous.at) {
                Ordering::Equal => previous.segment_ids.extend(sample.segment_ids),
                Ordering::Greater => *previous = sample,
                Ordering::Less => {}
            },
            None => moments.previous = Some(sample),
        },
    }
}

struct ContributingMoment {
    at: i64,
    segment_ids: HashSet<i64>,
}

#[derive(Default)]
struct ContributingMoments {
    current: Option<ContributingMoment>,
    previous: Option<ContributingMoment>,
    pinned: bool,
}

fn preceding(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segments: Vec<SegmentRef>,
    current: &Segment,
    sections: &[SectionPlans],
    at: i64,
) -> Result<Vec<SegmentRef>, ApiError> {
    let layouts = sections
        .iter()
        .filter(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_none())
        .flat_map(|section| section.plans.iter())
        .filter(|plan| plan.applies())
        .filter_map(|plan| plan.timestamp.map(|timestamp| (plan.type_id, timestamp)))
        .collect::<BTreeMap<_, _>>();
    if layouts.is_empty() {
        return Ok(Vec::new());
    }
    let compatible = layouts.keys().copied().collect::<HashSet<_>>();
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
    let mut moments = layouts
        .keys()
        .map(|type_id| (*type_id, ContributingMoments::default()))
        .collect::<BTreeMap<_, _>>();
    scan_contributing_moments(current, &layouts, at, true, &mut moments)?;
    candidates.sort_unstable_by_key(|candidate| Reverse((candidate.max_ts(), candidate.id())));
    for candidate in &candidates {
        if moments.values().all(|samples| {
            samples
                .previous
                .as_ref()
                .is_some_and(|previous| candidate.max_ts() < previous.at)
        }) {
            break;
        }
        let segment = reader.open_segment(candidate)?;
        scan_contributing_moments(&segment, &layouts, at, false, &mut moments)?;
    }
    let retained = moments
        .values()
        .flat_map(|moments| [&moments.current, &moments.previous])
        .filter_map(Option::as_ref)
        .flat_map(|moment| moment.segment_ids.iter().copied())
        .filter(|segment_id| *segment_id != segment_ref.id())
        .collect::<HashSet<_>>();
    candidates.retain(|candidate| retained.contains(&candidate.id()));
    candidates.sort_unstable_by_key(|candidate| Reverse(candidate.id()));
    Ok(candidates)
}

fn scan_contributing_moments(
    segment: &Segment,
    layouts: &BTreeMap<u32, &'static str>,
    at: i64,
    pin_current: bool,
    moments: &mut BTreeMap<u32, ContributingMoments>,
) -> Result<(), ApiError> {
    for (&type_id, &timestamp) in layouts {
        if segment.rows_of(type_id).is_none() {
            continue;
        }
        segment.visit_rows(type_id, &[timestamp], 0, usize::MAX, |_ordinal, row| {
            if let Some(stored) = row_timestamp(&row, timestamp)
                && stored <= at
            {
                record_contributing_moment(
                    moments.entry(type_id).or_default(),
                    stored,
                    segment.id(),
                );
            }
            true
        })?;
    }
    if pin_current {
        for samples in moments.values_mut() {
            samples.pinned = samples.current.is_some();
        }
    }
    Ok(())
}

fn record_contributing_moment(moments: &mut ContributingMoments, at: i64, segment_id: i64) {
    let new_moment = || ContributingMoment {
        at,
        segment_ids: HashSet::from([segment_id]),
    };
    let Some(current_at) = moments.current.as_ref().map(|current| current.at) else {
        moments.current = Some(new_moment());
        return;
    };
    if moments.pinned && at > current_at {
        return;
    }
    match at.cmp(&current_at) {
        Ordering::Equal => {
            if let Some(current) = moments.current.as_mut() {
                current.segment_ids.insert(segment_id);
            }
        }
        Ordering::Greater => {
            moments.previous = moments.current.take();
            moments.current = Some(new_moment());
        }
        Ordering::Less => match moments.previous.as_mut() {
            Some(previous) => match at.cmp(&previous.at) {
                Ordering::Equal => {
                    previous.segment_ids.insert(segment_id);
                }
                Ordering::Greater => moments.previous = Some(new_moment()),
                Ordering::Less => {}
            },
            None => moments.previous = Some(new_moment()),
        },
    }
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

fn output_rate_fields(plan: &Plan) -> Vec<&str> {
    plan.fields
        .iter()
        .filter_map(|field| {
            let column = field.column?;
            let cumulative = plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
            let exact_plan_calls =
                matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001) && field.name == "calls";
            (cumulative && !exact_plan_calls).then_some(field.name.as_str())
        })
        .collect()
}

fn projected_rate_columns(plan: &Plan) -> Vec<&'static str> {
    plan.projection
        .iter()
        .copied()
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
