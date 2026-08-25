//! Bounded `PostgreSQL` Vacuum episodes shared by HTTP and MCP.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Bound::Included;
use std::path::Path;
use std::time::Duration;

use hyper::StatusCode;
use kronika_reader::{Cell, Reader, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, TypeContract, contract, logical_section_name, registry};
use serde_json::{Map, Value, json};

use super::{ApiError, CachePolicy, ProductError, ResponseMeta};
use crate::product_semantics::{SemanticDefinition, SemanticPolicy, VacuumMovement, VacuumRisk};
use crate::route::VacuumRequest;

#[cfg(test)]
mod tests;

const VACUUM_SECTION: &str = "pg_stat_progress_vacuum";
const METADATA_SECTION: &str = "instance_metadata";
const PROCESS_SECTION: &str = "os_process";
const HOUR_US: i64 = 3_600_000_000;
const MAX_VACUUM_SAMPLES: usize = 500;
const MAX_VACUUM_IDENTITIES: usize = 256;
const MAX_VACUUM_SEGMENTS: usize = 64;
const MAX_VACUUM_FIELDS: usize = 32;
const ROW_CHUNK_ROWS: usize = 64;

pub(crate) const MAX_VACUUM_EPISODES: usize = 500;

pub(crate) const DEFAULT_VACUUM_FIELDS: &[&str] = &[
    "ts",
    "pid",
    "datid",
    "datname",
    "relid",
    "schemaname",
    "relname",
    "is_autovacuum",
    "phase",
    "heap_blks_total",
    "heap_blks_scanned",
    "heap_blks_vacuumed",
    "index_vacuum_count",
    "max_dead_tuples",
    "num_dead_tuples",
    "max_dead_tuple_bytes",
    "dead_tuple_bytes",
    "num_dead_item_ids",
    "indexes_total",
    "indexes_processed",
    "delay_time",
];

const MONOTONE_FIELDS: &[&str] = &[
    "index_vacuum_count",
    "heap_blks_scanned",
    "heap_blks_vacuumed",
];

pub(crate) struct PreparedVacuum {
    reader: Reader,
    segments: Vec<SegmentRef>,
    warnings: Vec<Value>,
    request: VacuumRequest,
    projected_fields: Vec<String>,
    policies: Policies,
    meta: ResponseMeta,
}

pub(super) fn prepare(root: &Path, request: VacuumRequest) -> Result<PreparedVacuum, ApiError> {
    validate_request(&request)?;
    let projected_fields = projected_fields(&request.fields)?;
    let policies = Policies::load()?;
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments((Included(request.from), Included(request.to)))?;
    let mut segments = listing
        .segments
        .into_iter()
        .filter(|segment| segment.max_ts() >= request.from && segment.min_ts() <= request.to)
        .collect::<Vec<_>>();
    segments.sort_by_key(|segment| (segment.min_ts(), segment.id()));
    if segments.len() > MAX_VACUUM_SEGMENTS {
        return Err(bound_error(
            "segment_bound_exceeded",
            format!(
                "the Vacuum interval intersects {} segments; at most {MAX_VACUUM_SEGMENTS} are admitted",
                segments.len()
            ),
            "from_us",
        ));
    }
    let cache = if segments
        .iter()
        .any(|segment| segment.kind() == SegmentKind::Active)
    {
        CachePolicy::NoStore
    } else {
        CachePolicy::Revalidate
    };
    let etag = super::weak_etag("vacuum", &format!("{request:?}"), &segments);
    let warnings = listing
        .warnings
        .iter()
        .map(super::catalog::warning_value)
        .collect();
    Ok(PreparedVacuum {
        reader,
        segments,
        warnings,
        request,
        projected_fields,
        policies,
        meta: ResponseMeta::ok_with_etag(cache, etag),
    })
}

impl PreparedVacuum {
    pub(super) fn meta(&self) -> ResponseMeta {
        self.meta.clone()
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        if cancelled() {
            return Ok(());
        }
        let result = match self.build(cancelled) {
            Ok(result) => result,
            Err(error) if error.code() == "cancelled" => return Ok(()),
            Err(error) => return Err(error),
        };
        if !cancelled() {
            emit(result);
        }
        Ok(())
    }

    fn build(self, cancelled: &impl Fn() -> bool) -> Result<Value, ApiError> {
        let opened = self.open_segments(cancelled)?;

        let facts = collect_segment_facts(&opened, cancelled)?;
        let mut samples =
            collect_vacuum_samples(&opened, self.request.from, self.request.to, cancelled)?;
        admit_samples(&samples)?;
        attach_cadence(&mut samples, &facts);
        let actual_type_ids = samples
            .iter()
            .map(|sample| sample.row.type_id)
            .collect::<BTreeSet<_>>();
        let available_fields = available_fields(&actual_type_ids);
        let layouts = recorded_layouts(&actual_type_ids)?;
        let at_timestamp = samples
            .iter()
            .filter(|sample| sample.row.timestamp <= self.request.at)
            .map(|sample| sample.row.timestamp)
            .max();
        let mut episodes = build_episodes(samples, &self.policies)?;
        if episodes.len() > MAX_VACUUM_EPISODES || episodes.len() > self.request.page_size {
            return Err(bound_error(
                "whole_set_bound_exceeded",
                format!(
                    "the Vacuum result has {} episodes; page_size admits {}",
                    episodes.len(),
                    self.request.page_size.min(MAX_VACUUM_EPISODES)
                ),
                "page_size",
            ));
        }
        sort_episodes(&mut episodes, at_timestamp, &self.policies)?;
        let process = collect_process_enrichment(
            &opened,
            &episodes,
            &facts,
            self.request.from,
            self.request.to,
            self.request.at,
            cancelled,
        )?;
        if cancelled() {
            return Err(cancelled_error());
        }
        let episode_values = episodes
            .iter()
            .enumerate()
            .map(|(index, episode)| {
                episode_value(
                    episode,
                    at_timestamp,
                    &self.projected_fields,
                    &self.policies,
                    process.get(index),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut warnings = self.warnings;
        if episodes.iter().any(|episode| {
            episode
                .rows
                .iter()
                .any(|sample| sample.cadence_seconds.is_none())
        }) {
            warnings.push(json!({
                "code": "vacuum_cadence_unavailable",
                "message": "Recorded PostgreSQL cadence is unavailable for one or more Vacuum samples; their adjacency has no time-distance condition.",
            }));
        }
        let semantics = self.policies.serialized_definitions(&layouts)?;
        let anchor_segment = anchor_segment(&self.segments, self.request.at);
        let anchor_cadence = anchor_segment
            .and_then(|segment| facts.get(&segment.id()))
            .and_then(|facts| facts.cadence_seconds);
        let returned = episode_values.len();
        Ok(json!({
            "record": "vacuum",
            "anchor": {
                "hour_start_us": self.request.from.div_euclid(HOUR_US).saturating_mul(HOUR_US).to_string(),
                "from_us": self.request.from.to_string(),
                "to_us": self.request.to.to_string(),
                "requested_at_us": self.request.at.to_string(),
                "selected_at_us": at_timestamp.map(|timestamp| timestamp.to_string()),
                "segment_id": anchor_segment.map(|segment| segment.id().to_string()),
                "active_wal_position": anchor_segment.and_then(SegmentRef::active_position).map(|position| position.to_string()),
                "cadence_seconds": anchor_cadence,
                "segments": self.segments.iter().map(|segment| json!({
                    "id": segment.id().to_string(),
                    "active_wal_position": segment.active_position().map(|position| position.to_string()),
                })).collect::<Vec<_>>(),
            },
            "available_fields": available_fields,
            "episodes": episode_values,
            "semantics": semantics,
            "warnings": warnings,
            "page": {
                "returned": returned,
                "truncated": false,
                "next_cursor": Value::Null,
                "stop_reason": "complete",
            },
        }))
    }

    fn open_segments<'a>(
        &'a self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<(&'a SegmentRef, Segment)>, ApiError> {
        let mut opened = Vec::with_capacity(self.segments.len());
        for segment_ref in &self.segments {
            if cancelled() {
                return Err(cancelled_error());
            }
            opened.push((segment_ref, self.reader.open_segment(segment_ref)?));
        }
        Ok(opened)
    }
}

fn validate_request(request: &VacuumRequest) -> Result<(), ApiError> {
    if request.from < 0
        || request.to < 0
        || request.from > request.to
        || request.from.div_euclid(HOUR_US) != request.to.div_euclid(HOUR_US)
    {
        return Err(input_error(
            "invalid_vacuum_interval",
            "Vacuum intervals must be ordered, nonnegative, and contained in one UTC hour.",
            "to_us",
        ));
    }
    if !(request.from..=request.to).contains(&request.at) {
        return Err(input_error(
            "invalid_vacuum_interval",
            "The Vacuum observation time must be inside the requested interval.",
            "to_us",
        ));
    }
    if !(1..=MAX_VACUUM_EPISODES).contains(&request.page_size) {
        return Err(input_error(
            "invalid_vacuum_page_size",
            "Vacuum page_size must be between 1 and 500.",
            "page_size",
        ));
    }
    if request.fields.len() > MAX_VACUUM_FIELDS
        || request.fields.iter().any(String::is_empty)
        || request.fields.iter().collect::<HashSet<_>>().len() != request.fields.len()
    {
        return Err(input_error(
            "invalid_vacuum_fields",
            "Vacuum fields must be nonempty, unique, and contain at most 32 names.",
            "fields",
        ));
    }
    Ok(())
}

fn projected_fields(requested: &[String]) -> Result<Vec<String>, ApiError> {
    let fields = if requested.is_empty() {
        DEFAULT_VACUUM_FIELDS
            .iter()
            .map(|field| (*field).to_owned())
            .collect::<Vec<_>>()
    } else {
        requested.to_vec()
    };
    let contracts = vacuum_contracts();
    if let Some(field) = fields
        .iter()
        .find(|field| !contracts.iter().any(|item| item.column(field).is_some()))
    {
        return Err(input_error(
            "invalid_vacuum_fields",
            format!("Vacuum has no field {field:?}."),
            "fields",
        ));
    }
    Ok(fields)
}

fn vacuum_contracts() -> Vec<&'static TypeContract> {
    registry()
        .iter()
        .filter(|item| logical_section_name(item.type_id.get()) == Some(VACUUM_SECTION))
        .collect()
}

fn input_error(
    code: &'static str,
    message: impl Into<String>,
    parameter: &'static str,
) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code,
        message: message.into(),
        parameter: Some(parameter),
        retryable: false,
        status: StatusCode::BAD_REQUEST,
    }))
}

fn bound_error(
    code: &'static str,
    message: impl Into<String>,
    parameter: &'static str,
) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code,
        message: message.into(),
        parameter: Some(parameter),
        retryable: false,
        status: StatusCode::UNPROCESSABLE_ENTITY,
    }))
}

fn malformed(message: impl Into<String>) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code: "malformed_vacuum_history",
        message: message.into(),
        parameter: None,
        retryable: false,
        status: StatusCode::INTERNAL_SERVER_ERROR,
    }))
}

fn semantics_error(message: impl Into<String>) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code: "semantics_unreadable",
        message: message.into(),
        parameter: None,
        retryable: false,
        status: StatusCode::INTERNAL_SERVER_ERROR,
    }))
}

fn cancelled_error() -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code: "cancelled",
        message: "the Vacuum product read was cancelled".to_owned(),
        parameter: None,
        retryable: false,
        status: StatusCode::REQUEST_TIMEOUT,
    }))
}

#[derive(Clone)]
struct NamedRow {
    segment_id: i64,
    logical_name: &'static str,
    type_id: u32,
    ordinal: u64,
    timestamp: i64,
    values: Map<String, Value>,
}

impl NamedRow {
    fn value(&self, field: &str) -> Option<&Value> {
        self.values.get(field)
    }

    fn integer(&self, field: &str) -> Result<Option<i128>, ApiError> {
        value_integer(self.value(field), field)
    }

    fn text(&self, field: &str) -> Result<Option<&str>, ApiError> {
        match self.value(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(value)),
            Some(_) => Err(malformed(format!(
                "a {name} row has a non-textual {field}",
                name = self.logical_name
            ))),
        }
    }

    fn value_object(&self) -> Value {
        json!({
            "segment_id": self.segment_id.to_string(),
            "logical_name": self.logical_name,
            "type_id": self.type_id.to_string(),
            "ordinal": self.ordinal.to_string(),
            "timestamp": self.timestamp.to_string(),
            "values": self.values,
        })
    }

    fn projected_value(&self, fields: &[String]) -> Value {
        let mut values = Map::new();
        let mut unavailable = Vec::new();
        for field in fields {
            if let Some(value) = self.values.get(field) {
                values.insert(field.clone(), value.clone());
            } else {
                values.insert(field.clone(), Value::Null);
                unavailable.push(field.clone());
            }
        }
        json!({
            "segment_id": self.segment_id.to_string(),
            "logical_name": self.logical_name,
            "type_id": self.type_id.to_string(),
            "ordinal": self.ordinal.to_string(),
            "timestamp": self.timestamp.to_string(),
            "values": values,
            "unavailable_fields": unavailable,
        })
    }
}

#[derive(Clone)]
struct Sample {
    row: NamedRow,
    key: EpisodeKey,
    cadence_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EpisodeKey {
    type_id: u32,
    pid: i32,
    datid: u32,
    relid: u32,
}

#[derive(Clone, Copy, Default)]
struct SegmentFacts {
    cadence_seconds: Option<u64>,
    clock_ticks_per_sec: Option<i64>,
}

fn collect_segment_facts(
    opened: &[(&SegmentRef, Segment)],
    cancelled: &impl Fn() -> bool,
) -> Result<HashMap<i64, SegmentFacts>, ApiError> {
    let mut result = HashMap::new();
    for (segment_ref, segment) in opened {
        if cancelled() {
            return Err(cancelled_error());
        }
        let mut newest: Option<NamedRow> = None;
        visit_named_rows(
            segment,
            segment_ref.id(),
            METADATA_SECTION,
            None,
            None,
            cancelled,
            |row| {
                if newest
                    .as_ref()
                    .is_none_or(|candidate| row.timestamp >= candidate.timestamp)
                {
                    newest = Some(row);
                }
                Ok(true)
            },
        )?;
        let facts = newest.map_or(Ok(SegmentFacts::default()), |row| {
            let cadence_seconds = row
                .integer("postgresql_interval_seconds")?
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| *value > 0);
            let clock_ticks_per_sec = row
                .integer("clock_ticks_per_sec")?
                .and_then(|value| i64::try_from(value).ok())
                .filter(|value| *value > 0);
            Ok::<_, ApiError>(SegmentFacts {
                cadence_seconds,
                clock_ticks_per_sec,
            })
        })?;
        result.insert(segment_ref.id(), facts);
    }
    Ok(result)
}

fn collect_vacuum_samples(
    opened: &[(&SegmentRef, Segment)],
    from: i64,
    to: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Sample>, ApiError> {
    let mut samples = Vec::new();
    for (segment_ref, segment) in opened {
        visit_named_rows(
            segment,
            segment_ref.id(),
            VACUUM_SECTION,
            Some((from, to)),
            None,
            cancelled,
            |row| {
                if samples.len() >= MAX_VACUUM_SAMPLES {
                    return Err(bound_error(
                        "sample_bound_exceeded",
                        format!(
                            "the Vacuum interval contains more than {MAX_VACUUM_SAMPLES} native samples"
                        ),
                        "from_us",
                    ));
                }
                let pid = positive_i32(&row, "pid")?;
                let datid = unsigned_u32(&row, "datid")?;
                let relid = unsigned_u32(&row, "relid")?;
                samples.push(Sample {
                    key: EpisodeKey {
                        type_id: row.type_id,
                        pid,
                        datid,
                        relid,
                    },
                    row,
                    cadence_seconds: None,
                });
                Ok(true)
            },
        )?;
    }
    Ok(samples)
}

fn admit_samples(samples: &[Sample]) -> Result<(), ApiError> {
    if samples.len() > MAX_VACUUM_SAMPLES {
        return Err(bound_error(
            "sample_bound_exceeded",
            format!(
                "the Vacuum interval contains {} native samples; at most {MAX_VACUUM_SAMPLES} are admitted",
                samples.len()
            ),
            "from_us",
        ));
    }
    let identities = samples
        .iter()
        .map(|sample| &sample.key)
        .collect::<BTreeSet<_>>();
    if identities.len() > MAX_VACUUM_IDENTITIES {
        return Err(bound_error(
            "entity_bound_exceeded",
            format!(
                "the Vacuum interval contains {} physical identities; at most {MAX_VACUUM_IDENTITIES} are admitted",
                identities.len()
            ),
            "from_us",
        ));
    }
    let mut locators = HashSet::new();
    for sample in samples {
        if !locators.insert((
            sample.row.segment_id,
            sample.row.type_id,
            sample.row.ordinal,
        )) {
            return Err(malformed(
                "the Vacuum interval repeats a physical row locator",
            ));
        }
    }
    Ok(())
}

fn attach_cadence(samples: &mut [Sample], facts: &HashMap<i64, SegmentFacts>) {
    for sample in samples {
        sample.cadence_seconds = facts
            .get(&sample.row.segment_id)
            .and_then(|facts| facts.cadence_seconds);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the row visitor keeps source, interval, identity, and cancellation explicit"
)]
fn visit_named_rows(
    segment: &Segment,
    segment_id: i64,
    logical_name: &'static str,
    window: Option<(i64, i64)>,
    wanted_pids: Option<&HashSet<i32>>,
    cancelled: &impl Fn() -> bool,
    mut visit: impl FnMut(NamedRow) -> Result<bool, ApiError>,
) -> Result<(), ApiError> {
    for (type_id, _section) in segment.layouts(logical_name) {
        if cancelled() {
            return Err(cancelled_error());
        }
        let Some(item) = contract(type_id) else {
            return Err(ApiError::NoSuchSection);
        };
        let projection = item
            .columns
            .iter()
            .map(|column| column.name)
            .collect::<Vec<_>>();
        let timestamp = item
            .columns
            .iter()
            .find(|column| column.class == ColumnClass::Timestamp)
            .map(|column| column.name)
            .ok_or_else(|| malformed(format!("{logical_name} has no timestamp column")))?;
        let mut chunk = Vec::<(u64, Row)>::with_capacity(ROW_CHUNK_ROWS);
        let mut failure = None;
        let mut stopped = false;
        segment.visit_rows(type_id, &projection, 0, usize::MAX, |ordinal, row| {
            if cancelled() {
                stopped = true;
                return false;
            }
            let Some(Cell::Ts(timestamp_value)) = row.get(timestamp) else {
                failure = Some(malformed(format!(
                    "a {logical_name} row has no valid timestamp"
                )));
                return false;
            };
            let timestamp_value = *timestamp_value;
            if window.is_some_and(|(from, to)| !(from..=to).contains(&timestamp_value)) {
                return true;
            }
            if let Some(wanted) = wanted_pids {
                let pid = match row.get("pid") {
                    Some(Cell::I32(value)) => *value,
                    _ => return true,
                };
                if !wanted.contains(&pid) {
                    return true;
                }
            }
            chunk.push((ordinal, row));
            if chunk.len() < ROW_CHUNK_ROWS {
                return true;
            }
            match visit_chunk(
                segment,
                segment_id,
                logical_name,
                type_id,
                timestamp,
                &mut chunk,
                &mut visit,
            ) {
                Ok(connected) => stopped = !connected,
                Err(error) => failure = Some(error),
            }
            !stopped && failure.is_none()
        })?;
        if failure.is_none() && !stopped && !chunk.is_empty() {
            stopped = !visit_chunk(
                segment,
                segment_id,
                logical_name,
                type_id,
                timestamp,
                &mut chunk,
                &mut visit,
            )?;
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if cancelled() {
            return Err(cancelled_error());
        }
        if stopped {
            return Ok(());
        }
    }
    Ok(())
}

fn visit_chunk(
    segment: &Segment,
    segment_id: i64,
    logical_name: &'static str,
    type_id: u32,
    timestamp_column: &str,
    chunk: &mut Vec<(u64, Row)>,
    visit: &mut impl FnMut(NamedRow) -> Result<bool, ApiError>,
) -> Result<bool, ApiError> {
    let dictionary = super::query::chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        let timestamp = match row.get(timestamp_column) {
            Some(Cell::Ts(value)) => *value,
            _ => {
                return Err(malformed(format!(
                    "a {logical_name} row lost its timestamp"
                )));
            }
        };
        let mut values = Map::new();
        for (field, cell) in row.iter() {
            values.insert(field.to_owned(), super::render::cell(cell, &dictionary)?);
        }
        if !visit(NamedRow {
            segment_id,
            logical_name,
            type_id,
            ordinal,
            timestamp,
            values,
        })? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn positive_i32(row: &NamedRow, field: &str) -> Result<i32, ApiError> {
    row.integer(field)?
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| malformed(format!("a {} row has no valid {field}", row.logical_name)))
}

fn unsigned_u32(row: &NamedRow, field: &str) -> Result<u32, ApiError> {
    row.integer(field)?
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| malformed(format!("a {} row has no valid {field}", row.logical_name)))
}

fn value_integer(value: Option<&Value>, field: &str) -> Result<Option<i128>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .map(Some)
        .ok_or_else(|| malformed(format!("Vacuum field {field} is not an integer")))
}

fn value_f64(value: Option<&Value>, field: &str) -> Result<Option<f64>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .filter(|value| value.is_finite())
        .map(Some)
        .ok_or_else(|| malformed(format!("Vacuum field {field} is not finite")))
}

struct Policies {
    adjacency_factor: f64,
    no_movement_samples: usize,
    movements: BTreeMap<String, VacuumMovement>,
    default_risk: VacuumRisk,
    risk_order: BTreeMap<VacuumRisk, usize>,
    phase_risks: BTreeMap<String, VacuumRisk>,
    definitions: [&'static SemanticDefinition; 3],
}

impl Policies {
    fn load() -> Result<Self, ApiError> {
        let adjacency = semantic("vacuum.episode_adjacency")?;
        let no_movement = semantic("vacuum.no_movement")?;
        let risk = semantic("vacuum.phase_risk")?;
        let SemanticPolicy::VacuumEpisode { adjacency_factor } = &adjacency.policy else {
            return Err(semantics_error(
                "Vacuum adjacency policy has the wrong kind",
            ));
        };
        if !adjacency_factor.is_finite() || *adjacency_factor <= 0.0 {
            return Err(semantics_error("Vacuum adjacency factor is invalid"));
        }
        let SemanticPolicy::VacuumNoMovement { samples, phases } = &no_movement.policy else {
            return Err(semantics_error(
                "Vacuum no-movement policy has the wrong kind",
            ));
        };
        let no_movement_samples = usize::try_from(*samples)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| semantics_error("Vacuum no-movement sample count is invalid"))?;
        let movements = phases
            .iter()
            .cloned()
            .map(|movement| (movement.phase.clone(), movement))
            .collect();
        let SemanticPolicy::VacuumRisk {
            default,
            order,
            phases,
        } = &risk.policy
        else {
            return Err(semantics_error("Vacuum risk policy has the wrong kind"));
        };
        let risk_order = order
            .iter()
            .copied()
            .enumerate()
            .map(|(position, value)| (value, position))
            .collect();
        Ok(Self {
            adjacency_factor: *adjacency_factor,
            no_movement_samples,
            movements,
            default_risk: *default,
            risk_order,
            phase_risks: phases.clone(),
            definitions: [adjacency, no_movement, risk],
        })
    }

    fn risk(&self, phase: &str) -> VacuumRisk {
        self.phase_risks
            .get(phase)
            .copied()
            .unwrap_or(self.default_risk)
    }

    fn risk_position(&self, risk: VacuumRisk) -> usize {
        self.risk_order.get(&risk).copied().unwrap_or(usize::MAX)
    }

    fn serialized_definitions(&self, layouts: &[Value]) -> Result<Vec<Value>, ApiError> {
        let mut result = self
            .definitions
            .iter()
            .map(|definition| {
                let mut value = serde_json::to_value(definition)?;
                value
                    .as_object_mut()
                    .ok_or_else(|| semantics_error("a Vacuum semantic is not an object"))?
                    .insert("source".to_owned(), json!("kronika_product_registry"));
                Ok(value)
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        for layout in layouts {
            result.push(json!({
                "id": format!("layout.{}", layout.get("type_id").and_then(Value::as_str).unwrap_or("unknown")),
                "origin": "recorded",
                "source": "kronika_registry",
                "logical_name": VACUUM_SECTION,
                "type_id": layout.get("type_id").cloned().unwrap_or(Value::Null),
                "layout": layout,
            }));
        }
        Ok(result)
    }
}

fn semantic(id: &str) -> Result<&'static SemanticDefinition, ApiError> {
    crate::product_semantics::get(id)
        .map_err(|error| semantics_error(error.to_string()))?
        .ok_or_else(|| semantics_error(format!("missing accepted semantic {id}")))
}

struct Episode {
    key: EpisodeKey,
    rows: Vec<Sample>,
    phases: Vec<PhaseSpan>,
}

#[derive(Clone)]
struct PhaseSpan {
    first: usize,
    last: usize,
    name: String,
    cycle: Option<i128>,
    no_movement: Option<NoMovement>,
}

#[derive(Clone)]
struct NoMovement {
    field: String,
    samples: usize,
    span_us: i64,
}

fn build_episodes(samples: Vec<Sample>, policies: &Policies) -> Result<Vec<Episode>, ApiError> {
    let mut streams = BTreeMap::<EpisodeKey, Vec<Sample>>::new();
    for sample in samples {
        streams.entry(sample.key.clone()).or_default().push(sample);
    }
    let mut episodes = Vec::new();
    for (key, mut stream) in streams {
        stream.sort_by_key(|sample| {
            (
                sample.row.timestamp,
                sample.row.segment_id,
                sample.row.ordinal,
            )
        });
        let mut current = Vec::new();
        for sample in stream {
            let continues = current
                .last()
                .map(|previous| continues(previous, &sample, policies.adjacency_factor))
                .transpose()?
                .unwrap_or(false);
            if !continues && !current.is_empty() {
                episodes.push(finish_episode(
                    key.clone(),
                    std::mem::take(&mut current),
                    policies,
                )?);
            }
            current.push(sample);
        }
        if !current.is_empty() {
            episodes.push(finish_episode(key, current, policies)?);
        }
    }
    Ok(episodes)
}

fn continues(previous: &Sample, current: &Sample, factor: f64) -> Result<bool, ApiError> {
    if let Some(seconds) = current.cadence_seconds {
        let allowed = Duration::from_secs(seconds).mul_f64(factor);
        let allowed_us = i64::try_from(allowed.as_micros())
            .map_err(|_overflow| semantics_error("Vacuum adjacency duration is too large"))?;
        if current.row.timestamp.saturating_sub(previous.row.timestamp) > allowed_us {
            return Ok(false);
        }
    }
    for field in MONOTONE_FIELDS {
        if let (Some(before), Some(after)) =
            (previous.row.integer(field)?, current.row.integer(field)?)
            && after < before
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn finish_episode(
    key: EpisodeKey,
    rows: Vec<Sample>,
    policies: &Policies,
) -> Result<Episode, ApiError> {
    if rows.is_empty() {
        return Err(malformed("a Vacuum episode has no samples"));
    }
    let mut phases = Vec::new();
    let mut first = 0;
    while first < rows.len() {
        let name = required_phase(&rows[first])?.to_owned();
        let cycle = rows[first].row.integer("index_vacuum_count")?;
        let mut last = first;
        while last + 1 < rows.len()
            && required_phase(&rows[last + 1])? == name
            && rows[last + 1].row.integer("index_vacuum_count")? == cycle
        {
            last += 1;
        }
        let no_movement = no_movement(&rows[first..=last], &name, policies)?;
        phases.push(PhaseSpan {
            first,
            last,
            name,
            cycle,
            no_movement,
        });
        first = last + 1;
    }
    Ok(Episode { key, rows, phases })
}

fn required_phase(sample: &Sample) -> Result<&str, ApiError> {
    sample
        .row
        .text("phase")?
        .ok_or_else(|| malformed("a Vacuum sample has no phase"))
}

fn no_movement(
    rows: &[Sample],
    phase: &str,
    policies: &Policies,
) -> Result<Option<NoMovement>, ApiError> {
    let Some(last) = rows.last() else {
        return Ok(None);
    };
    let Some(movement) = policies.movements.get(phase) else {
        return Ok(None);
    };
    if movement
        .unavailable_type_ids
        .iter()
        .any(|type_id| type_id == &last.row.type_id.to_string())
    {
        return Ok(None);
    }
    let start = if movement.field == "phase" {
        0
    } else {
        let Some(reading) = last.row.integer(&movement.field)? else {
            return Ok(None);
        };
        let mut start = rows.len().saturating_sub(1);
        while start > 0 && rows[start - 1].row.integer(&movement.field)? == Some(reading) {
            start -= 1;
        }
        start
    };
    let still = &rows[start..];
    if still.len() < policies.no_movement_samples {
        return Ok(None);
    }
    Ok(Some(NoMovement {
        field: movement.field.clone(),
        samples: still.len(),
        span_us: last.row.timestamp.saturating_sub(still[0].row.timestamp),
    }))
}

fn sort_episodes(
    episodes: &mut [Episode],
    at_timestamp: Option<i64>,
    policies: &Policies,
) -> Result<(), ApiError> {
    let mut failure = None;
    episodes.sort_by(|left, right| {
        if failure.is_some() {
            return Ordering::Equal;
        }
        match compare_episodes(left, right, at_timestamp, policies) {
            Ok(ordering) => ordering,
            Err(error) => {
                failure = Some(error);
                Ordering::Equal
            }
        }
    });
    failure.map_or(Ok(()), Err)
}

fn compare_episodes(
    left: &Episode,
    right: &Episode,
    at_timestamp: Option<i64>,
    policies: &Policies,
) -> Result<Ordering, ApiError> {
    let left_last = episode_last(left)?;
    let right_last = episode_last(right)?;
    let left_at = at_timestamp == Some(left_last.row.timestamp);
    let right_at = at_timestamp == Some(right_last.row.timestamp);
    let active = right_at.cmp(&left_at);
    if active != Ordering::Equal {
        return Ok(active);
    }
    if left_at {
        let risk = policies
            .risk_position(policies.risk(required_phase(left_last)?))
            .cmp(&policies.risk_position(policies.risk(required_phase(right_last)?)));
        if risk != Ordering::Equal {
            return Ok(risk);
        }
        let span = phase_span_us(right)?.cmp(&phase_span_us(left)?);
        if span != Ordering::Equal {
            return Ok(span);
        }
        let cycle = right_last
            .row
            .integer("index_vacuum_count")?
            .unwrap_or(0)
            .cmp(&left_last.row.integer("index_vacuum_count")?.unwrap_or(0));
        if cycle != Ordering::Equal {
            return Ok(cycle);
        }
    }
    Ok(right_last
        .row
        .timestamp
        .cmp(&left_last.row.timestamp)
        .then_with(|| left.key.cmp(&right.key))
        .then_with(|| {
            episode_first(left)
                .map_or(i64::MIN, |sample| sample.row.timestamp)
                .cmp(&episode_first(right).map_or(i64::MIN, |sample| sample.row.timestamp))
        }))
}

fn episode_first(episode: &Episode) -> Option<&Sample> {
    episode.rows.first()
}

fn episode_last(episode: &Episode) -> Result<&Sample, ApiError> {
    episode
        .rows
        .last()
        .ok_or_else(|| malformed("a Vacuum episode has no latest sample"))
}

fn trailing_phase(episode: &Episode) -> Result<&PhaseSpan, ApiError> {
    episode
        .phases
        .last()
        .ok_or_else(|| malformed("a Vacuum episode has no phase history"))
}

fn phase_span_us(episode: &Episode) -> Result<i64, ApiError> {
    let phase = trailing_phase(episode)?;
    Ok(episode.rows[phase.last]
        .row
        .timestamp
        .saturating_sub(episode.rows[phase.first].row.timestamp))
}

#[derive(Default)]
struct ProcessEnrichment {
    current: Option<NamedRow>,
    before: Option<NamedRow>,
    after: Option<NamedRow>,
    clock_ticks_per_sec: Option<i64>,
}

struct ProcessTarget {
    pid: i32,
    first_at: i64,
    last_at: i64,
    current_at: i64,
    enrichment: ProcessEnrichment,
}

fn collect_process_enrichment(
    opened: &[(&SegmentRef, Segment)],
    episodes: &[Episode],
    facts: &HashMap<i64, SegmentFacts>,
    from: i64,
    to: i64,
    at: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<ProcessEnrichment>, ApiError> {
    let mut targets = episodes
        .iter()
        .map(|episode| {
            let first = episode_first(episode)
                .ok_or_else(|| malformed("a Vacuum episode has no first sample"))?;
            let last = episode_last(episode)?;
            Ok(ProcessTarget {
                pid: episode.key.pid,
                first_at: first.row.timestamp,
                last_at: last.row.timestamp,
                current_at: at,
                enrichment: ProcessEnrichment::default(),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let wanted_pids = targets
        .iter()
        .map(|target| target.pid)
        .collect::<HashSet<_>>();
    if wanted_pids.is_empty() {
        return Ok(Vec::new());
    }
    for (segment_ref, segment) in opened {
        visit_named_rows(
            segment,
            segment_ref.id(),
            PROCESS_SECTION,
            Some((from, to)),
            Some(&wanted_pids),
            cancelled,
            |row| {
                let pid = positive_i32(&row, "pid")?;
                for target in targets.iter_mut().filter(|target| target.pid == pid) {
                    update_latest(&mut target.enrichment.current, &row, target.current_at);
                    update_latest(&mut target.enrichment.before, &row, target.first_at);
                    update_latest(&mut target.enrichment.after, &row, target.last_at);
                }
                Ok(true)
            },
        )?;
    }
    Ok(targets
        .into_iter()
        .map(|mut target| {
            target.enrichment.clock_ticks_per_sec = target
                .enrichment
                .after
                .as_ref()
                .and_then(|row| facts.get(&row.segment_id))
                .and_then(|facts| facts.clock_ticks_per_sec);
            target.enrichment
        })
        .collect())
}

fn update_latest(stored: &mut Option<NamedRow>, candidate: &NamedRow, at: i64) {
    if candidate.timestamp <= at
        && stored
            .as_ref()
            .is_none_or(|current| candidate.timestamp > current.timestamp)
    {
        *stored = Some(candidate.clone());
    }
}

fn episode_value(
    episode: &Episode,
    at_timestamp: Option<i64>,
    projected: &[String],
    policies: &Policies,
    process: Option<&ProcessEnrichment>,
) -> Result<Value, ApiError> {
    let first =
        episode_first(episode).ok_or_else(|| malformed("a Vacuum episode has no first sample"))?;
    let last = episode_last(episode)?;
    let phase = trailing_phase(episode)?;
    let at_sample = at_timestamp == Some(last.row.timestamp);
    let phases = episode
        .phases
        .iter()
        .map(|span| phase_value(episode, span, policies))
        .collect::<Result<Vec<_>, _>>()?;
    let latest_phase = phases
        .last()
        .cloned()
        .ok_or_else(|| malformed("a Vacuum episode has no phase history"))?;
    let samples = episode
        .rows
        .iter()
        .map(|sample| sample_value(sample, policies))
        .collect::<Result<Vec<_>, _>>()?;
    let sample_locators = episode
        .rows
        .iter()
        .map(|sample| {
            json!({
                "segment_id": sample.row.segment_id.to_string(),
                "type_id": sample.row.type_id.to_string(),
                "row_ordinal": sample.row.ordinal.to_string(),
                "timestamp_us": sample.row.timestamp.to_string(),
            })
        })
        .collect::<Vec<_>>();
    let relation = relation_value(last)?;
    let progress = progress_value(episode)?;
    let index_cycles = index_cycles(episode)?;
    let delay_delta_ms = delay_delta(episode)?;
    let process = process_value(process)?;
    Ok(json!({
        "identity": {
            "type_id": episode.key.type_id.to_string(),
            "pid": episode.key.pid,
            "datid": episode.key.datid,
            "relid": episode.key.relid,
        },
        "first_at_us": first.row.timestamp.to_string(),
        "last_at_us": last.row.timestamp.to_string(),
        "span_us": last.row.timestamp.saturating_sub(first.row.timestamp).to_string(),
        "sample_count": episode.rows.len(),
        "observation": {
            "kind": if at_sample { "at_sample" } else { "last_recorded" },
            "at_sample": at_sample,
            "timestamp_us": last.row.timestamp.to_string(),
        },
        "relation": relation,
        "phase": latest_phase,
        "phase_history": phases,
        "index_cycles": index_cycles,
        "progress": progress,
        "delay_delta_ms": delay_delta_ms,
        "latest_row": last.row.projected_value(projected),
        "samples": samples,
        "sample_locators": sample_locators,
        "process": process,
        "trailing_phase_sample_count": phase.last.saturating_sub(phase.first).saturating_add(1),
    }))
}

fn phase_value(
    episode: &Episode,
    phase: &PhaseSpan,
    policies: &Policies,
) -> Result<Value, ApiError> {
    let first = episode
        .rows
        .get(phase.first)
        .ok_or_else(|| malformed("a Vacuum phase has no first sample"))?;
    let last = episode
        .rows
        .get(phase.last)
        .ok_or_else(|| malformed("a Vacuum phase has no latest sample"))?;
    Ok(json!({
        "name": phase.name,
        "risk": policies.risk(&phase.name),
        "first_at_us": first.row.timestamp.to_string(),
        "last_at_us": last.row.timestamp.to_string(),
        "span_us": last.row.timestamp.saturating_sub(first.row.timestamp).to_string(),
        "sample_count": phase.last.saturating_sub(phase.first).saturating_add(1),
        "index_vacuum_count": phase.cycle.map(|value| value.to_string()),
        "no_movement": phase.no_movement.as_ref().map(|reading| json!({
            "field": reading.field,
            "samples": reading.samples,
            "span_us": reading.span_us.to_string(),
        })),
    }))
}

fn sample_value(sample: &Sample, policies: &Policies) -> Result<Value, ApiError> {
    let phase = required_phase(sample)?;
    Ok(json!({
        "segment_id": sample.row.segment_id.to_string(),
        "type_id": sample.row.type_id.to_string(),
        "ordinal": sample.row.ordinal.to_string(),
        "timestamp": sample.row.timestamp.to_string(),
        "values": sample.row.values,
        "phase": phase,
        "risk": policies.risk(phase),
        "cadence_seconds": sample.cadence_seconds,
        "index_vacuum_count": sample.row.value("index_vacuum_count").cloned().unwrap_or(Value::Null),
    }))
}

fn relation_value(last: &Sample) -> Result<Value, ApiError> {
    let is_autovacuum = match last.row.value("is_autovacuum") {
        Some(Value::Bool(value)) => *value,
        _ => return Err(malformed("a Vacuum sample has no autovacuum kind")),
    };
    Ok(json!({
        "database": last.row.value("datname").cloned().unwrap_or(Value::Null),
        "schema": last.row.value("schemaname").cloned().unwrap_or(Value::Null),
        "name": last.row.value("relname").cloned().unwrap_or(Value::Null),
        "relid": last.key.relid.to_string(),
        "is_autovacuum": is_autovacuum,
    }))
}

fn progress_value(episode: &Episode) -> Result<Value, ApiError> {
    let mut heap_scan = Vec::new();
    for sample in &episode.rows {
        let (Some(scanned), Some(total)) = (
            sample.row.integer("heap_blks_scanned")?,
            sample.row.integer("heap_blks_total")?,
        ) else {
            continue;
        };
        if total <= 0 {
            continue;
        }
        let scanned_f64 = scanned
            .to_string()
            .parse::<f64>()
            .map_err(|_error| malformed("a Vacuum progress value cannot be represented"))?;
        let total_f64 = total
            .to_string()
            .parse::<f64>()
            .map_err(|_error| malformed("a Vacuum total cannot be represented"))?;
        heap_scan.push(json!({
            "timestamp_us": sample.row.timestamp.to_string(),
            "heap_blks_scanned": sample.row.value("heap_blks_scanned").cloned().unwrap_or(Value::Null),
            "heap_blks_total": sample.row.value("heap_blks_total").cloned().unwrap_or(Value::Null),
            "percent": (100.0 * scanned_f64 / total_f64).clamp(0.0, 100.0),
        }));
    }
    let last = episode_last(episode)?;
    let has_index_progress = last.row.values.contains_key("indexes_total")
        && last.row.values.contains_key("indexes_processed");
    let phase = required_phase(last)?;
    let applicable = matches!(phase, "vacuuming indexes" | "cleaning up indexes");
    let index = has_index_progress.then(|| {
        json!({
            "applicable": applicable,
            "processed": last.row.value("indexes_processed").cloned().unwrap_or(Value::Null),
            "total": last.row.value("indexes_total").cloned().unwrap_or(Value::Null),
        })
    });
    Ok(json!({
        "heap_scan": heap_scan,
        "index": index,
    }))
}

fn index_cycles(episode: &Episode) -> Result<Vec<Value>, ApiError> {
    let mut result = Vec::new();
    let mut first = 0;
    while first < episode.rows.len() {
        let cycle = episode.rows[first].row.integer("index_vacuum_count")?;
        let mut last = first;
        while last + 1 < episode.rows.len()
            && episode.rows[last + 1].row.integer("index_vacuum_count")? == cycle
        {
            last += 1;
        }
        result.push(json!({
            "index_vacuum_count": cycle.map(|value| value.to_string()),
            "first_at_us": episode.rows[first].row.timestamp.to_string(),
            "last_at_us": episode.rows[last].row.timestamp.to_string(),
            "sample_count": last.saturating_sub(first).saturating_add(1),
        }));
        first = last + 1;
    }
    Ok(result)
}

fn delay_delta(episode: &Episode) -> Result<Option<f64>, ApiError> {
    let Some(last) = episode.rows.last() else {
        return Ok(None);
    };
    if last.row.type_id != 1_012_006 {
        return Ok(None);
    }
    let Some(previous) = episode.rows.get(episode.rows.len().saturating_sub(2)) else {
        return Ok(None);
    };
    let (Some(before), Some(after)) = (
        value_f64(previous.row.value("delay_time"), "delay_time")?,
        value_f64(last.row.value("delay_time"), "delay_time")?,
    ) else {
        return Ok(None);
    };
    Ok((after >= before).then_some(after - before))
}

fn process_value(process: Option<&ProcessEnrichment>) -> Result<Value, ApiError> {
    let Some(process) = process else {
        return Ok(json!({"current_row": Value::Null, "load": Value::Null}));
    };
    let load = process_load(process)?;
    Ok(json!({
        "current_row": process.current.as_ref().map(NamedRow::value_object),
        "load": load,
    }))
}

fn process_load(process: &ProcessEnrichment) -> Result<Value, ApiError> {
    let (Some(before), Some(after)) = (&process.before, &process.after) else {
        return Ok(Value::Null);
    };
    if after.timestamp <= before.timestamp {
        return Ok(Value::Null);
    }
    let delta = |field| exact_delta(before, after, field);
    let cpu_ticks = match (delta("utime")?, delta("stime")?) {
        (Some(user), Some(system)) => user.checked_add(system),
        _ => None,
    };
    let block_wait_ticks = delta("blkdelay_ticks")?;
    let run_delay_ns = delta("rundelay_ns")?;
    let read_bytes = delta("read_bytes")?;
    let write_bytes = delta("write_bytes")?;
    let major_faults = delta("majflt")?;
    let ticks_per_second = process.clock_ticks_per_sec.filter(|value| *value > 0);
    let cpu_ms = cpu_ticks
        .zip(ticks_per_second)
        .map(|(ticks, scale)| {
            Ok::<_, ApiError>(
                integer_f64(ticks, "process CPU ticks")?
                    / integer_f64(i128::from(scale), "process clock ticks")?
                    * 1_000.0,
            )
        })
        .transpose()?;
    let block_wait_ms = block_wait_ticks
        .zip(ticks_per_second)
        .map(|(ticks, scale)| {
            Ok::<_, ApiError>(
                integer_f64(ticks, "process block-wait ticks")?
                    / integer_f64(i128::from(scale), "process clock ticks")?
                    * 1_000.0,
            )
        })
        .transpose()?;
    let span_seconds = integer_f64(
        i128::from(after.timestamp.saturating_sub(before.timestamp)),
        "process sample span",
    )? / 1_000_000.0;
    let cpu_share_percent = cpu_ms
        .filter(|_value| span_seconds > 0.0)
        .map(|value| (value / 1_000.0 / span_seconds * 100.0).clamp(0.0, 100.0));
    Ok(json!({
        "before_at_us": before.timestamp.to_string(),
        "after_at_us": after.timestamp.to_string(),
        "cpu_ticks": cpu_ticks.map(|value| value.to_string()),
        "cpu_ms": cpu_ms,
        "cpu_share_percent": cpu_share_percent,
        "block_wait_ticks": block_wait_ticks.map(|value| value.to_string()),
        "block_wait_ms": block_wait_ms,
        "run_delay_ns": run_delay_ns.map(|value| value.to_string()),
        "read_bytes": read_bytes.map(|value| value.to_string()),
        "write_bytes": write_bytes.map(|value| value.to_string()),
        "major_faults": major_faults.map(|value| value.to_string()),
    }))
}

fn integer_f64(value: i128, field: &str) -> Result<f64, ApiError> {
    value
        .to_string()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| malformed(format!("{field} cannot be represented")))
}

fn exact_delta(before: &NamedRow, after: &NamedRow, field: &str) -> Result<Option<i128>, ApiError> {
    let (Some(before), Some(after)) = (before.integer(field)?, after.integer(field)?) else {
        return Ok(None);
    };
    Ok(after.checked_sub(before).filter(|delta| *delta >= 0))
}

fn available_fields(type_ids: &BTreeSet<u32>) -> Vec<&'static str> {
    let mut seen = HashSet::new();
    vacuum_contracts()
        .into_iter()
        .filter(|item| type_ids.contains(&item.type_id.get()))
        .flat_map(|item| item.columns.iter().map(|column| column.name))
        .filter(|field| seen.insert(*field))
        .collect()
}

fn recorded_layouts(type_ids: &BTreeSet<u32>) -> Result<Vec<Value>, ApiError> {
    type_ids
        .iter()
        .map(|type_id| {
            let item = contract(*type_id).ok_or(ApiError::NoSuchSection)?;
            let fields = item
                .columns
                .iter()
                .map(|column| (column.name, Some(column)))
                .collect::<Vec<_>>();
            Ok(super::render::projected_layout(
                VACUUM_SECTION,
                item,
                &fields,
            ))
        })
        .collect()
}

fn anchor_segment(segments: &[SegmentRef], at: i64) -> Option<&SegmentRef> {
    segments
        .iter()
        .filter(|segment| segment.min_ts() <= at)
        .max_by_key(|segment| (segment.min_ts(), segment.id()))
        .or_else(|| segments.first())
}
