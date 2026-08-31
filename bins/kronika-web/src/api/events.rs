//! Shared recorded-event query, decoding, grouping, and result contract.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use kronika_reader::{Cell, Reader, Row, Segment, SegmentKind, SegmentRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::query::{Plan, plans, streaming_chunk_dictionary, validate_row_dictionary};
use super::render::{cell, record};
use super::row_key::{self, DetailLocator};
use super::time::TimeRange;
use super::{ApiError, CachePolicy, ResponseMeta, log_warnings, weak_etag};
use crate::route::{DataRequest, Filter, SegmentRequest};

mod group;

#[cfg(test)]
use group::event_collator;
use group::{EventGroups, SlowThreshold};
#[cfg(test)]
use group::{group_events, slow_threshold_ms};

pub(crate) const MAX_EVENTS_WINDOW_MICROS: i64 = 3_600_000_000;
pub(crate) const MAX_EVENTS_LIMIT: usize = 5_000;
const ROW_CHUNK_ROWS: usize = 512;
const MINUTE_COLUMNS: usize = 60;
const MINUTE_MICROS: i64 = 60_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventsRepresentation {
    Groups,
    Occurrences,
}

impl EventsRepresentation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Groups => "groups",
            Self::Occurrences => "occurrences",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, JsonSchema)]
pub(crate) enum EventSource {
    #[serde(rename = "pg_log_errors")]
    Errors,
    #[serde(rename = "pg_log_checkpoints")]
    Checkpoints,
    #[serde(rename = "pg_log_autovacuum")]
    Autovacuum,
    #[serde(rename = "pg_log_slow_queries")]
    SlowQueries,
    #[serde(rename = "pg_log_lock_waits")]
    LockWaits,
    #[serde(rename = "pg_log_temp_files")]
    TempFiles,
    #[serde(rename = "pg_log_lifecycle")]
    Lifecycle,
    #[serde(rename = "pgbouncer_events")]
    Pgbouncer,
}

impl EventSource {
    const GROUPS: [Self; 7] = [
        Self::Errors,
        Self::Checkpoints,
        Self::Autovacuum,
        Self::SlowQueries,
        Self::LockWaits,
        Self::Lifecycle,
        Self::Pgbouncer,
    ];
    const OCCURRENCES: [Self; 8] = [
        Self::Errors,
        Self::Checkpoints,
        Self::Autovacuum,
        Self::SlowQueries,
        Self::LockWaits,
        Self::TempFiles,
        Self::Lifecycle,
        Self::Pgbouncer,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Errors => "pg_log_errors",
            Self::Checkpoints => "pg_log_checkpoints",
            Self::Autovacuum => "pg_log_autovacuum",
            Self::SlowQueries => "pg_log_slow_queries",
            Self::LockWaits => "pg_log_lock_waits",
            Self::TempFiles => "pg_log_temp_files",
            Self::Lifecycle => "pg_log_lifecycle",
            Self::Pgbouncer => "pgbouncer_events",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::OCCURRENCES
            .into_iter()
            .find(|source| source.as_str() == name)
    }

    const fn group_fields(self) -> &'static [&'static str] {
        match self {
            Self::Errors => &[
                "severity", "category", "sqlstate", "pattern", "count", "database", "username",
            ],
            Self::Checkpoints => &[
                "phase",
                "reason",
                "seconds_apart",
                "buffers_written",
                "sync_ms",
            ],
            Self::Autovacuum => &[
                "kind",
                "relation",
                "tuples_removed",
                "tuples_dead_not_removable",
                "elapsed_ms",
            ],
            Self::SlowQueries => &["pattern", "count", "max_duration_ms", "total_duration_ms"],
            Self::LockWaits => &["kind", "pid", "lock_target", "duration_ms", "holding_pids"],
            Self::Lifecycle => &["kind", "pid", "signal", "shutdown_mode"],
            Self::Pgbouncer => &["level", "database", "text"],
            Self::TempFiles => &[],
        }
    }

    const fn occurrence_fields(self) -> &'static [&'static str] {
        match self {
            Self::Errors => &[
                "system_identifier",
                "source_file",
                "severity",
                "category",
                "sqlstate",
                "pattern",
                "count",
                "database",
                "username",
            ],
            Self::Checkpoints => &[
                "system_identifier",
                "source_file",
                "phase",
                "seconds_apart",
                "buffers_written",
                "write_ms",
                "sync_ms",
                "total_ms",
                "distance_kb",
                "estimate_kb",
                "wal_added",
                "wal_removed",
                "wal_recycled",
                "sync_files",
                "longest_sync_ms",
                "average_sync_ms",
            ],
            Self::Autovacuum => &[
                "system_identifier",
                "source_file",
                "kind",
                "relation",
                "index_scans",
                "pages_removed",
                "pages_remaining",
                "tuples_removed",
                "tuples_remaining",
                "tuples_dead_not_removable",
                "elapsed_ms",
                "buffer_hits",
                "buffer_misses",
                "buffer_dirtied",
                "avg_read_rate_mbs",
                "avg_write_rate_mbs",
                "cpu_user_ms",
                "cpu_system_ms",
                "wal_records",
                "wal_fpi",
                "wal_bytes",
            ],
            Self::SlowQueries => &[
                "system_identifier",
                "source_file",
                "pattern",
                "count",
                "max_duration_ms",
                "total_duration_ms",
            ],
            Self::LockWaits => &[
                "system_identifier",
                "source_file",
                "kind",
                "pid",
                "lock_mode",
                "lock_target",
                "duration_ms",
                "holding_pids",
                "wait_queue",
            ],
            Self::TempFiles => &["system_identifier", "source_file", "path", "size_bytes"],
            Self::Lifecycle => &[
                "system_identifier",
                "source_file",
                "kind",
                "pid",
                "signal",
                "shutdown_mode",
            ],
            Self::Pgbouncer => &[
                "source_file",
                "level",
                "database",
                "username",
                "host",
                "text",
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventsQuery {
    pub(crate) range: TimeRange,
    pub(crate) sources: Vec<EventSource>,
    pub(crate) representation: EventsRepresentation,
    pub(crate) limit: usize,
}

impl EventsQuery {
    pub(crate) fn normalize(
        range: TimeRange,
        requested: Option<Vec<String>>,
        representation: EventsRepresentation,
        limit: usize,
    ) -> Result<Self, EventsQueryError> {
        if !(1..=MAX_EVENTS_LIMIT).contains(&limit) {
            return Err(EventsQueryError::Limit(limit));
        }
        let valid = match representation {
            EventsRepresentation::Groups => &EventSource::GROUPS[..],
            EventsRepresentation::Occurrences => &EventSource::OCCURRENCES[..],
        };
        let mut sources = Vec::new();
        for source in requested.unwrap_or_else(|| {
            valid
                .iter()
                .map(|source| source.as_str().to_owned())
                .collect()
        }) {
            let parsed = EventSource::parse(&source)
                .filter(|source| valid.contains(source))
                .ok_or_else(|| EventsQueryError::Source {
                    source,
                    valid: valid
                        .iter()
                        .map(|source| source.as_str().to_owned())
                        .collect(),
                })?;
            if !sources.contains(&parsed) {
                sources.push(parsed);
            }
        }
        Ok(Self {
            range,
            sources,
            representation,
            limit,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventsQueryError {
    Limit(usize),
    Source { source: String, valid: Vec<String> },
}

impl EventsQueryError {
    pub(crate) fn valid_options(&self) -> Vec<String> {
        match self {
            Self::Source { valid, .. } => valid.clone(),
            Self::Limit(_) => Vec::new(),
        }
    }
}

impl std::fmt::Display for EventsQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limit(limit) => write!(
                f,
                "limit must be between 1 and {MAX_EVENTS_LIMIT}, got {limit}"
            ),
            Self::Source { source, valid } => {
                write!(f, "unknown source {source:?}: valid sources are {valid:?}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "representation", rename_all = "snake_case")]
pub(crate) enum EventsResult {
    Groups {
        groups: Vec<EventGroup>,
        truncated: bool,
    },
    Occurrences {
        occurrences: Vec<EventOccurrence>,
        truncated: bool,
    },
}

impl EventsResult {
    pub(crate) const fn representation(&self) -> EventsRepresentation {
        match self {
            Self::Groups { .. } => EventsRepresentation::Groups,
            Self::Occurrences { .. } => EventsRepresentation::Occurrences,
        }
    }

    pub(crate) const fn truncated(&self) -> bool {
        match self {
            Self::Groups { truncated, .. } | Self::Occurrences { truncated, .. } => *truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventDataRow {
    pub(crate) segment_id: i64,
    pub(crate) type_id: u32,
    pub(crate) row_ordinal: u64,
    pub(crate) timestamp: i64,
    pub(crate) identity: row_key::RowIdentity,
    pub(crate) values: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventTier {
    Critical,
    Notable,
    Routine,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventGroup {
    pub(crate) key: String,
    pub(crate) section: String,
    pub(crate) tier: EventTier,
    pub(crate) label: Option<String>,
    pub(crate) count: f64,
    pub(crate) first_ts: i64,
    pub(crate) last_ts: i64,
    pub(crate) representative_ts: i64,
    pub(crate) minutes: Vec<f64>,
    pub(crate) stat: EventStat,
    #[serde(rename = "detail_locator")]
    pub(crate) detail_locator: DetailLocator,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind")]
pub(crate) enum EventStat {
    #[serde(rename = "pg.errors")]
    Errors {
        severity: f64,
        category: Option<f64>,
        sqlstate: Option<String>,
        database: Option<String>,
        username: Option<String>,
    },
    #[serde(rename = "pg.slow", rename_all = "camelCase")]
    Slow {
        max_ms: f64,
        total_ms: f64,
        threshold_ms: Option<f64>,
    },
    #[serde(rename = "pg.autovacuum", rename_all = "camelCase")]
    Autovacuum {
        analyze: bool,
        runs: usize,
        total_ms: Option<f64>,
        tuples_removed: Option<f64>,
        tuples_dead: Option<f64>,
    },
    #[serde(rename = "pg.checkpoints", rename_all = "camelCase")]
    Checkpoints {
        completes: usize,
        timed: usize,
        requested: usize,
        max_sync_ms: Option<f64>,
        buffers: Option<f64>,
    },
    #[serde(rename = "pg.checkpoint_warning", rename_all = "camelCase")]
    CheckpointWarning { seconds_apart: Option<f64> },
    #[serde(rename = "pg.locks", rename_all = "camelCase")]
    Locks {
        holders: Option<String>,
        acquired: bool,
        waiters: usize,
        max_ms: Option<f64>,
        targets: Vec<String>,
    },
    #[serde(rename = "pg.lifecycle")]
    Lifecycle {
        lifecycle: f64,
        pid: Option<f64>,
        signal: Option<f64>,
        mode: Option<String>,
    },
    #[serde(rename = "pgbouncer.events")]
    Pgbouncer {
        level: f64,
        database: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct EventOccurrence {
    #[serde(flatten)]
    pub(crate) fields: Map<String, Value>,
    pub(crate) source: String,
    pub(crate) detail_locator: DetailLocator,
}

pub(crate) struct PreparedEvents {
    reader: Reader,
    segments: Vec<SegmentRef>,
    query: EventsQuery,
    meta: ResponseMeta,
}

struct RetainedOccurrence {
    source: EventSource,
    row: EventDataRow,
}

struct OccurrenceAccumulator {
    limit: usize,
    encounters: Vec<u64>,
    rows: BTreeMap<(i64, usize, u64), RetainedOccurrence>,
}

impl OccurrenceAccumulator {
    fn new(query: &EventsQuery) -> Self {
        Self {
            limit: query.limit,
            encounters: vec![0; query.sources.len()],
            rows: BTreeMap::new(),
        }
    }

    fn observe(&mut self, source_rank: usize, source: EventSource, row: EventDataRow) {
        let encounter = self.encounters[source_rank];
        self.encounters[source_rank] = encounter.saturating_add(1);
        let key = (row.timestamp, source_rank, encounter);
        let capacity = self.limit.saturating_add(1);
        if self.rows.len() == capacity {
            if self
                .rows
                .last_key_value()
                .is_some_and(|(last, _row)| key >= *last)
            {
                return;
            }
            self.rows.pop_last();
        }
        self.rows.insert(key, RetainedOccurrence { source, row });
    }

    fn finish(self) -> Result<EventsResult, ApiError> {
        let mut seen = HashSet::new();
        for retained in self.rows.values() {
            let row = &retained.row;
            let identity = serde_json::to_string(&row.identity)?;
            if !seen.insert((
                retained.source,
                row.segment_id,
                row.type_id,
                row.timestamp,
                identity,
            )) {
                return Err(ApiError::BadLocator(format!(
                    "cannot emit detail_ref: {} has a non-unique identity at timestamp {} in segment {}",
                    retained.source.as_str(),
                    row.timestamp,
                    row.segment_id,
                )));
            }
        }

        let truncated = self.rows.len() > self.limit;
        let occurrences = self
            .rows
            .into_values()
            .take(self.limit)
            .map(|retained| occurrence(retained.source, retained.row))
            .collect();
        Ok(EventsResult::Occurrences {
            occurrences,
            truncated,
        })
    }

    #[cfg(test)]
    fn retained_rows(&self) -> usize {
        self.rows.len()
    }
}

pub(crate) fn prepare(root: &Path, query: EventsQuery) -> Result<PreparedEvents, ApiError> {
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments(query.range.from..query.range.to_exclusive)?;
    log_warnings(&listing.warnings);
    let mut segments = listing.segments;
    segments.retain(|segment| {
        segment.max_ts() >= query.range.from && segment.min_ts() < query.range.to_exclusive
    });
    segments.sort_by_key(SegmentRef::min_ts);
    let shape = format!("{query:?}");
    let etag = weak_etag("events", &shape, &segments);
    let cache = if etag.is_some() {
        CachePolicy::Immutable
    } else if segments
        .iter()
        .any(|segment| segment.kind() == SegmentKind::Active)
    {
        CachePolicy::NoStore
    } else {
        CachePolicy::Revalidate
    };
    Ok(PreparedEvents {
        reader,
        segments,
        query,
        meta: ResponseMeta::ok_with_etag(cache, etag),
    })
}

impl PreparedEvents {
    pub(crate) fn meta(&self) -> ResponseMeta {
        self.meta.clone()
    }

    pub(crate) fn execute(self, cancelled: &impl Fn() -> bool) -> Result<EventsResult, ApiError> {
        match self.query.representation {
            EventsRepresentation::Groups => self.execute_groups(cancelled),
            EventsRepresentation::Occurrences => self.execute_occurrences(cancelled),
        }
    }

    fn execute_groups(self, cancelled: &impl Fn() -> bool) -> Result<EventsResult, ApiError> {
        let mut groups = EventGroups::new(self.query.range.from);
        let needs_threshold = self.query.representation == EventsRepresentation::Groups
            && self.query.sources.contains(&EventSource::SlowQueries);
        let mut threshold = SlowThreshold::default();

        for segment_ref in &self.segments {
            if cancelled() {
                return Err(ApiError::Cancelled);
            }
            if !carries_selected(segment_ref, &self.query.sources, needs_threshold) {
                continue;
            }
            let segment = self.reader.open_segment(segment_ref)?;
            for source in &self.query.sources {
                let mut observe = |row| {
                    groups.observe(*source, row);
                    Ok(())
                };
                collect_section(
                    &segment,
                    segment_ref.id(),
                    source.as_str(),
                    source.group_fields(),
                    &[],
                    self.query.range,
                    &mut observe,
                    cancelled,
                )?;
            }
            if needs_threshold {
                let mut observe = |row| {
                    threshold.observe(&row);
                    Ok(())
                };
                collect_section(
                    &segment,
                    segment_ref.id(),
                    "pg_settings",
                    &["name", "setting", "unit"],
                    &[Filter {
                        column: "name".to_owned(),
                        value: "log_min_duration_statement".to_owned(),
                    }],
                    self.query.range,
                    &mut observe,
                    cancelled,
                )?;
            }
        }
        if cancelled() {
            return Err(ApiError::Cancelled);
        }
        groups_result(&self.query, groups, threshold.finish())
    }

    fn execute_occurrences(self, cancelled: &impl Fn() -> bool) -> Result<EventsResult, ApiError> {
        let mut occurrences = OccurrenceAccumulator::new(&self.query);
        for segment_ref in &self.segments {
            if cancelled() {
                return Err(ApiError::Cancelled);
            }
            if !carries_selected(segment_ref, &self.query.sources, false) {
                continue;
            }
            let segment = self.reader.open_segment(segment_ref)?;
            for (source_rank, source) in self.query.sources.iter().copied().enumerate() {
                let mut observe = |row| {
                    occurrences.observe(source_rank, source, row);
                    Ok(())
                };
                collect_section(
                    &segment,
                    segment_ref.id(),
                    source.as_str(),
                    source.occurrence_fields(),
                    &[],
                    self.query.range,
                    &mut observe,
                    cancelled,
                )?;
            }
        }
        if cancelled() {
            return Err(ApiError::Cancelled);
        }
        occurrences.finish()
    }

    pub(crate) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let result = self.execute(cancelled)?;
        if cancelled()
            || !emit(record(json!({
                "record": "events",
                "representation": result.representation().as_str(),
                "truncated": result.truncated(),
            }))?)
        {
            return Ok(());
        }
        match result {
            EventsResult::Groups { groups, .. } => emit_groups(groups, emit),
            EventsResult::Occurrences { occurrences, .. } => emit_occurrences(occurrences, emit),
        }
    }
}

fn emit_groups(
    groups: Vec<EventGroup>,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
) -> Result<(), ApiError> {
    for group in groups {
        let value = public_item("event_group", &group, &group.detail_locator)?;
        if !emit(record(value)?) {
            break;
        }
    }
    Ok(())
}

fn emit_occurrences(
    occurrences: Vec<EventOccurrence>,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
) -> Result<(), ApiError> {
    for occurrence in occurrences {
        let value = public_item("event_occurrence", &occurrence, &occurrence.detail_locator)?;
        if !emit(record(value)?) {
            break;
        }
    }
    Ok(())
}

fn public_item<T: Serialize>(
    record_name: &str,
    item: &T,
    locator: &DetailLocator,
) -> Result<Value, ApiError> {
    let mut value = serde_json::to_value(item)?;
    let Value::Object(ref mut object) = value else {
        return Err(ApiError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "event item did not serialize as an object",
        ))));
    };
    if object.remove("detail_locator").is_none() {
        return Err(ApiError::BadLocator(
            "event item has no detail locator".to_owned(),
        ));
    }
    object.insert(
        "detail_ref".to_owned(),
        Value::String(locator.detail_ref().map_err(ApiError::BadLocator)?),
    );
    object.insert("record".to_owned(), json!(record_name));
    Ok(value)
}

fn carries_selected(segment: &SegmentRef, sources: &[EventSource], settings: bool) -> bool {
    segment.sections().iter().any(|section| {
        let name = kronika_registry::logical_section_name(section.type_id);
        (settings && name == Some("pg_settings"))
            || sources.iter().any(|source| name == Some(source.as_str()))
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one low-level section scan receives its exact storage coordinates and sinks"
)]
fn collect_section(
    segment: &Segment,
    segment_id: i64,
    logical_name: &str,
    fields: &[&str],
    filters: &[Filter],
    range: TimeRange,
    output: &mut impl FnMut(EventDataRow) -> Result<(), ApiError>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let request = DataRequest {
        segment: SegmentRequest {
            segment_id,
            section: logical_name.to_owned(),
        },
        fields: fields.iter().map(|field| (*field).to_owned()).collect(),
        filters: filters.to_vec(),
        type_id: None,
        after: None,
    };
    let section_plans = match plans(segment, &request, true) {
        Ok(section_plans) => section_plans,
        Err(ApiError::NoSuchSection) => return Ok(()),
        Err(error) => return Err(error),
    };
    for plan in &section_plans {
        collect_plan(segment, segment_id, plan, range, output, cancelled)?;
    }
    Ok(())
}

fn collect_plan(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    range: TimeRange,
    output: &mut impl FnMut(EventDataRow) -> Result<(), ApiError>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    if !plan.applies() {
        return Ok(());
    }
    let Some(timestamp_column) = plan.timestamp else {
        return Ok(());
    };
    let mut chunk: Vec<(u64, Row)> = Vec::with_capacity(ROW_CHUNK_ROWS);
    let mut failure = None;
    let mut was_cancelled = false;
    segment.visit_rows(
        plan.type_id,
        &plan.projection,
        plan.start_row,
        usize::MAX,
        |ordinal, row| {
            if cancelled() {
                was_cancelled = true;
                return false;
            }
            if !row
                .get(timestamp_column)
                .is_some_and(|cell| matches!(cell, Cell::Ts(at) if range.contains(*at)))
            {
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
                output,
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
    if was_cancelled {
        return Err(ApiError::Cancelled);
    }
    if !chunk.is_empty() {
        append_chunk(
            segment,
            segment_id,
            plan,
            timestamp_column,
            &mut chunk,
            output,
        )?;
    }
    Ok(())
}

fn append_chunk(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    timestamp_column: &str,
    chunk: &mut Vec<(u64, Row)>,
    output: &mut impl FnMut(EventDataRow) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let dictionary = streaming_chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        validate_row_dictionary(&row, &dictionary)?;
        if !plan.matches(&row, &dictionary) {
            continue;
        }
        let Some(Cell::Ts(at)) = row.get(timestamp_column) else {
            continue;
        };
        let identity = row_key::identity(plan.type_id, &row).map_err(|error| {
            ApiError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })?;
        let mut values = Map::new();
        for field in &plan.fields {
            values.insert(
                field.name.clone(),
                field
                    .column
                    .and_then(|name| row.get(name))
                    .map_or(Ok(Value::Null), |value| cell(value, &dictionary))?,
            );
        }
        output(EventDataRow {
            segment_id,
            type_id: plan.type_id,
            row_ordinal: ordinal,
            timestamp: *at,
            identity,
            values,
        })?;
    }
    Ok(())
}

fn groups_result(
    query: &EventsQuery,
    groups: EventGroups,
    threshold_ms: Option<f64>,
) -> Result<EventsResult, ApiError> {
    let mut groups = groups.finish(threshold_ms)?;
    let mut seen = HashSet::new();
    for group in &groups {
        let locator = &group.detail_locator;
        let identity = serde_json::to_string(&locator.identity)?;
        if !seen.insert((
            locator.section.as_str(),
            locator.segment_id,
            locator.type_id,
            locator.at,
            identity,
        )) {
            return Err(ApiError::BadLocator(format!(
                "cannot emit detail_ref: {} has a non-unique identity at timestamp {} in segment {}",
                locator.section, locator.at, locator.segment_id,
            )));
        }
    }
    let truncated = groups.len() > query.limit;
    groups.truncate(query.limit);
    Ok(EventsResult::Groups { groups, truncated })
}

fn occurrence(source: EventSource, row: EventDataRow) -> EventOccurrence {
    let mut fields = row.values;
    let detail_locator = row_key::detail_locator(
        source.as_str(),
        row.segment_id,
        row.timestamp,
        row.type_id,
        row.row_ordinal,
        row.identity,
    );
    fields.retain(|field, _| !row_key::is_detail_text(source.as_str(), field));
    label_event_fields(source.as_str(), &mut fields);
    EventOccurrence {
        fields,
        source: source.as_str().to_owned(),
        detail_locator,
    }
}

#[cfg(test)]
fn occurrences_result(query: &EventsQuery, by_source: Vec<Vec<EventDataRow>>) -> EventsResult {
    let mut occurrences = OccurrenceAccumulator::new(query);
    for (source_rank, (source, rows)) in query.sources.iter().copied().zip(by_source).enumerate() {
        for row in rows {
            occurrences.observe(source_rank, source, row);
        }
    }
    occurrences.finish().expect("valid occurrence fixture")
}

pub(crate) fn label_event_fields(section: &str, fields: &mut Map<String, Value>) {
    for (field, labels) in event_labels(section) {
        add_map_label(fields, field, labels);
    }
}

fn event_labels(section: &str) -> &'static [(&'static str, &'static [&'static str])] {
    match section {
        "pg_log_errors" => &[
            ("severity", &["error", "fatal", "panic", "warning", "log"]),
            (
                "category",
                &[
                    "lock",
                    "constraint",
                    "serialization",
                    "timeout",
                    "resource",
                    "data_corruption",
                    "system",
                    "connection",
                    "auth",
                    "syntax",
                    "other",
                ],
            ),
        ],
        "pg_log_checkpoints" => &[("phase", &["started", "completed", "too_frequent"])],
        "pg_log_autovacuum" => &[("kind", &["vacuum", "analyze"])],
        "pg_log_lock_waits" => &[("kind", &["waiting", "acquired"])],
        "pg_log_lifecycle" => &[("kind", &["crash", "shutdown", "ready"])],
        "pgbouncer_events" => &[(
            "level",
            &["fatal", "error", "warning", "log", "debug", "noise"],
        )],
        _ => &[],
    }
}

fn add_map_label(fields: &mut Map<String, Value>, field: &str, labels: &[&str]) {
    let Some(index) = fields
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return;
    };
    if let Some(label) = labels.get(index) {
        fields.insert(format!("{field}_label"), json!(label));
    }
}

#[cfg(test)]
mod tests;
