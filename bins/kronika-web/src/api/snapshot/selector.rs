//! Sample selection shared by the nine current-state finders.

use std::path::Path;

use kronika_reader::{Cell, Reader, SegmentRef};
use kronika_registry::{contract, logical_section_name, registry};

use super::relation::{RelationKind, RelationRow};
use super::search::StructuredSearch;
use super::{PlainRowOut, PreparedSnapshot, ProcessRowOut};
use crate::api::time::SnapshotPoint;
use crate::api::{ApiError, Prepared};
use crate::route::{Order, RelationGroup, SnapshotRequest};

const SECOND_MICROS: i64 = 1_000_000;
const MIN_LOOKBACK_MICROS: i64 = 20 * SECOND_MICROS;
const DEFAULT_POSTGRESQL_CADENCE_SECONDS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SamplePolicy {
    logical_name: &'static str,
    fixed_cadence_seconds: Option<u64>,
}

const DERIVED_SORT_TOKENS: [(&str, &str); 7] = [
    (
        "derived_mean_exec_ms_per_call",
        "derived.mean_exec_ms_per_call",
    ),
    ("derived_rows_per_call", "derived.rows_per_call"),
    ("derived_blocks_per_call", "derived.blocks_per_call"),
    ("derived_hit_fraction", "derived.hit_pct"),
    ("derived_wal_per_call", "derived.wal_per_call"),
    ("derived_plan_time_fraction", "derived.plan_time_pct"),
    ("derived_cv", "derived.cv"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinderSurface {
    Processes,
    Tables,
    Indexes,
    Activity,
    Locks,
    Vacuum,
    Databases,
    Statements,
    Plans,
}

impl FinderSurface {
    pub(crate) const fn logical_name(self) -> &'static str {
        self.policy().logical_name
    }

    const fn policy(self) -> SamplePolicy {
        match self {
            Self::Processes => SamplePolicy {
                logical_name: "os_process",
                fixed_cadence_seconds: Some(5),
            },
            Self::Tables => SamplePolicy {
                logical_name: "pg_stat_user_tables",
                fixed_cadence_seconds: Some(300),
            },
            Self::Indexes => SamplePolicy {
                logical_name: "pg_stat_user_indexes",
                fixed_cadence_seconds: Some(300),
            },
            Self::Activity => SamplePolicy {
                logical_name: "pg_stat_activity",
                fixed_cadence_seconds: None,
            },
            Self::Locks => SamplePolicy {
                logical_name: "pg_locks",
                fixed_cadence_seconds: None,
            },
            Self::Vacuum => SamplePolicy {
                logical_name: "pg_stat_progress_vacuum",
                fixed_cadence_seconds: None,
            },
            Self::Databases => SamplePolicy {
                logical_name: "pg_stat_database",
                fixed_cadence_seconds: None,
            },
            Self::Statements => SamplePolicy {
                logical_name: "pg_stat_statements",
                fixed_cadence_seconds: None,
            },
            Self::Plans => SamplePolicy {
                logical_name: "pg_store_plans",
                fixed_cadence_seconds: None,
            },
        }
    }

    const fn relation_kind(self) -> Option<RelationKind> {
        match self {
            Self::Tables => Some(RelationKind::Tables),
            Self::Indexes => Some(RelationKind::Indexes),
            _ => None,
        }
    }

    fn order_token(self, group: Option<RelationGroup>, field: &str) -> Result<String, ApiError> {
        if let Some(kind) = self.relation_kind() {
            let group = group.ok_or_else(|| ApiError::BadFilter("group".to_owned()))?;
            return kind
                .sort_field_known(group, field)
                .then(|| field.to_owned())
                .ok_or_else(|| ApiError::NoSuchColumn(field.to_owned()));
        }
        if matches!(self, Self::Statements | Self::Plans)
            && let Some((_, token)) = DERIVED_SORT_TOKENS
                .iter()
                .find(|(public, _token)| *public == field)
        {
            return Ok((*token).to_owned());
        }
        registry()
            .iter()
            .filter(|layout| {
                logical_section_name(layout.type_id.get()) == Some(self.logical_name())
            })
            .any(|layout| layout.column(field).is_some())
            .then(|| field.to_owned())
            .ok_or_else(|| ApiError::NoSuchColumn(field.to_owned()))
    }
}

pub(crate) struct FinderOrder {
    pub(crate) field: String,
    pub(crate) direction: Order,
}

pub(crate) struct FinderQuery {
    pub(crate) surface: FinderSurface,
    pub(crate) point: SnapshotPoint,
    pub(crate) search: Option<StructuredSearch>,
    pub(crate) order: Option<FinderOrder>,
    pub(crate) group: Option<RelationGroup>,
    pub(crate) limit: usize,
}

pub(crate) struct FinderResult<R> {
    pub(crate) rows: Vec<R>,
    pub(crate) truncated: bool,
    pub(crate) as_of: Option<i64>,
}

impl<R> FinderResult<R> {
    const fn empty() -> Self {
        Self {
            rows: Vec::new(),
            truncated: false,
            as_of: None,
        }
    }
}

pub(crate) fn execute_processes(
    root: &Path,
    query: FinderQuery,
    cancelled: &impl Fn() -> bool,
) -> Result<FinderResult<ProcessRowOut>, ApiError> {
    if query.surface != FinderSurface::Processes {
        return Err(ApiError::BadFilter("surface".to_owned()));
    }
    let limit = query.limit;
    replay_source_change(|| {
        let Some(prepared) = prepare(root, &query, cancelled)? else {
            return Ok(FinderResult::empty());
        };
        prepared.compute_process_rows(limit, cancelled)
    })
}

pub(crate) fn execute_plain(
    root: &Path,
    query: FinderQuery,
    cancelled: &impl Fn() -> bool,
) -> Result<FinderResult<PlainRowOut>, ApiError> {
    if matches!(
        query.surface,
        FinderSurface::Processes | FinderSurface::Tables | FinderSurface::Indexes
    ) {
        return Err(ApiError::BadFilter("surface".to_owned()));
    }
    let limit = query.limit;
    replay_source_change(|| {
        let Some(prepared) = prepare(root, &query, cancelled)? else {
            return Ok(FinderResult::empty());
        };
        prepared.compute_plain_rows(limit, cancelled)
    })
}

pub(crate) fn execute_relation(
    root: &Path,
    query: FinderQuery,
    cancelled: &impl Fn() -> bool,
) -> Result<FinderResult<RelationRow>, ApiError> {
    if !matches!(
        query.surface,
        FinderSurface::Tables | FinderSurface::Indexes
    ) {
        return Err(ApiError::BadFilter("surface".to_owned()));
    }
    let limit = query.limit;
    replay_source_change(|| {
        let Some(prepared) = prepare(root, &query, cancelled)? else {
            return Ok(FinderResult::empty());
        };
        prepared.compute_relation_rows(limit, cancelled)
    })
}

fn replay_source_change<R>(
    mut execute: impl FnMut() -> Result<R, ApiError>,
) -> Result<R, ApiError> {
    match execute() {
        Err(error) if error.source_changed_during_read() => execute(),
        result => result,
    }
}

fn prepare(
    root: &Path,
    query: &FinderQuery,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<PreparedSnapshot>, ApiError> {
    if cancelled() {
        return Err(ApiError::Cancelled);
    }
    let reader = Reader::open(root)?;
    let (listing, requested_at, current_from) = match query.point {
        SnapshotPoint::LatestRecorded => (reader.catalog_segments(..)?, None, None),
        SnapshotPoint::At(at) => {
            let policy = query.surface.policy();
            let discovery = reader.catalog_discovery()?;
            let cadence = if let Some(cadence) = policy.fixed_cadence_seconds {
                cadence
            } else {
                let probe = discovery.clone().segments_with_predecessors_for(
                    at..=at,
                    &physical_type_ids("instance_metadata"),
                )?;
                recorded_postgresql_cadence(&reader, &probe.segments, at, cancelled)?
                    .unwrap_or(DEFAULT_POSTGRESQL_CADENCE_SECONDS)
            };
            let lookback = cadence_lookback(cadence)?;
            let from = at.checked_sub(lookback).unwrap_or(i64::MIN);
            (
                discovery.segments_with_predecessors_for(
                    from..=at,
                    &physical_type_ids(policy.logical_name),
                )?,
                Some(at),
                Some(from),
            )
        }
    };
    super::super::log_warnings(&listing.warnings);
    let clean = listing.warnings.is_empty();
    let by = query
        .order
        .as_ref()
        .map(|order| query.surface.order_token(query.group, &order.field))
        .transpose()?
        .into_iter()
        .collect();
    let direction = query
        .order
        .as_ref()
        .map_or(Order::Asc, |order| order.direction);
    let mut segments = listing.segments;
    let Some(index) = selected_segment(&segments, query.surface.logical_name()) else {
        return Ok(None);
    };
    let anchor = segments.remove(index);
    let at = requested_at.unwrap_or_else(|| anchor.max_ts());
    let request = SnapshotRequest {
        segment_id: anchor.id(),
        at,
        sections: vec![query.surface.logical_name().to_owned()],
        fields: Vec::new(),
        by,
        direction,
        group: query.group,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
    };
    let Prepared::Snapshot(mut prepared) =
        super::prepare_selected(reader, anchor, segments, clean, request)?
    else {
        return Err(ApiError::BadCursor);
    };
    prepared.current_from = current_from;
    prepared.with_search(query.search.clone()).map(Some)
}

fn selected_segment(segments: &[SegmentRef], logical_name: &str) -> Option<usize> {
    segments
        .iter()
        .enumerate()
        .filter(|(_index, segment)| {
            segment
                .sections()
                .iter()
                .any(|section| logical_section_name(section.type_id) == Some(logical_name))
        })
        .max_by_key(|(_index, segment)| segment.id())
        .map(|(index, _segment)| index)
}

fn physical_type_ids(logical_name: &str) -> Vec<u32> {
    registry()
        .iter()
        .filter(|layout| logical_section_name(layout.type_id.get()) == Some(logical_name))
        .map(|layout| layout.type_id.get())
        .collect()
}

fn cadence_lookback(cadence_seconds: u64) -> Result<i64, ApiError> {
    let cadence = i64::try_from(cadence_seconds)
        .ok()
        .and_then(|seconds| seconds.checked_mul(2_500_000))
        .ok_or_else(|| {
            ApiError::Unreadable(Box::new(std::io::Error::other(
                "recorded PostgreSQL interval is too large",
            )))
        })?;
    Ok(cadence.max(MIN_LOOKBACK_MICROS))
}

fn recorded_postgresql_cadence(
    reader: &Reader,
    segments: &[SegmentRef],
    at: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<u64>, ApiError> {
    let mut selected: Option<(i64, u64)> = None;
    for segment_ref in segments {
        if cancelled() {
            return Err(ApiError::Cancelled);
        }
        let segment = reader.open_segment(segment_ref)?;
        for (type_id, _rows) in segment.sections() {
            if logical_section_name(type_id) != Some("instance_metadata") {
                continue;
            }
            let Some(layout) = contract(type_id) else {
                continue;
            };
            let (Some(timestamp), Some(interval)) = (
                layout.column("ts"),
                layout.column("postgresql_interval_seconds"),
            ) else {
                continue;
            };
            segment.visit_rows(
                type_id,
                &[timestamp.name, interval.name],
                0,
                usize::MAX,
                |_ordinal, row| {
                    if cancelled() {
                        return false;
                    }
                    let (Some(Cell::Ts(timestamp)), Some(Cell::U64(seconds))) =
                        (row.get(timestamp.name), row.get(interval.name))
                    else {
                        return true;
                    };
                    if *timestamp <= at && *seconds > 0 {
                        let candidate = (*timestamp, *seconds);
                        if selected.is_none_or(|current| candidate.0 > current.0) {
                            selected = Some(candidate);
                        }
                    }
                    true
                },
            )?;
            if cancelled() {
                return Err(ApiError::Cancelled);
            }
        }
    }
    Ok(selected.map(|(_timestamp, seconds)| seconds))
}

#[cfg(test)]
mod tests;
