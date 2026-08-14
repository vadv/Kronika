//! Construction of the small presentation-series allowlist.

use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, Reader, ReaderError, Resolved, Segment, SegmentKind, SegmentRef};
use kronika_registry::instance_metadata::Environment;

use crate::detect::{FindingBuilder, finding_layout};
use crate::file::Index;
use crate::health::{SourcePenalty, Stall, health, overall_health, postgres_penalty};
use crate::series::{
    ActiveBackendPoint, HealthPoint, SeriesBlock, SeriesKey, SeriesKind, TransactionPoint,
    pg_activity_layout, pg_database_layout,
};

/// Reserved input-free layout for the derived OS health gauge.
pub const DERIVED_HEALTH_TYPE_ID: u32 = 0;
/// `type_id` of `instance_metadata`.
pub const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_002;
/// Previous `instance_metadata`, used only to retain OS-health readability.
pub const INSTANCE_METADATA_V1_TYPE_ID: u32 = 1_021_001;
/// `type_id` of `os_psi`.
pub const OS_PSI_TYPE_ID: u32 = 1_107_001;

const CPU: u32 = 0;
const MEMORY: u32 = 1;
const IO: u32 = 2;
const HOST: u32 = 0;
const POD: u32 = 1;
const CONTAINER: u32 = 3;

#[derive(Debug, Clone, Copy, Default)]
struct HealthSeed {
    os: Option<StallSnapshot>,
    postgres: Option<HealthPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotIdentity {
    environment: Option<u32>,
    boot_time: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StallSnapshot {
    timestamp: i64,
    identity: SnapshotIdentity,
    stall: Option<Stall>,
}

/// Why an allowlisted series could not be derived.
#[derive(Debug)]
pub enum BuildError {
    /// The production reader rejected an input body.
    Reader(ReaderError),
    /// A `PostgreSQL` state dictionary id had no value.
    UnresolvedState(u64),
    /// The one current metadata row was absent or malformed.
    InvalidMetadata,
    /// A `pg_log_errors.category` value was absent or outside its registry range.
    InvalidLogErrorCategory,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reader(error) => error.fmt(f),
            Self::UnresolvedState(id) => {
                write!(
                    f,
                    "pg_stat_activity state has unresolved dictionary id {id}"
                )
            }
            Self::InvalidMetadata => write!(f, "instance_metadata v2 must contain exactly one row"),
            Self::InvalidLogErrorCategory => {
                write!(f, "pg_log_errors.category must be between 0 and 10")
            }
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reader(error) => Some(error),
            Self::UnresolvedState(_) | Self::InvalidMetadata | Self::InvalidLogErrorCategory => {
                None
            }
        }
    }
}

impl From<ReaderError> for BuildError {
    fn from(error: ReaderError) -> Self {
        Self::Reader(error)
    }
}

fn predecessor_health_seed(
    reader: &Reader,
    predecessor: Option<&SegmentRef>,
    requested: &[SeriesKey],
) -> Result<HealthSeed, BuildError> {
    let needs_os = requested
        .iter()
        .any(|key| matches!(key.kind, SeriesKind::OsHealth | SeriesKind::OverallHealth));
    let needs_postgres = requested.contains(&SeriesKey::OVERALL_HEALTH);
    let Some(predecessor) = predecessor.filter(|_| needs_os || needs_postgres) else {
        return Ok(HealthSeed::default());
    };
    let has_psi = predecessor
        .sections()
        .iter()
        .any(|section| section.type_id == OS_PSI_TYPE_ID);
    let has_metadata = predecessor
        .sections()
        .iter()
        .any(|section| section.type_id == INSTANCE_METADATA_TYPE_ID);
    if !(needs_os && has_psi || needs_postgres && has_metadata) {
        return Ok(HealthSeed::default());
    }

    let segment = reader.open_segment(predecessor)?;
    let os = if needs_os && has_psi {
        last_stall_snapshot(&segment)?
    } else {
        None
    };
    let postgres = if needs_postgres && has_metadata {
        let metadata = health_metadata(&segment)?;
        if metadata.postgresql_enabled == Some(true) {
            let mut activity = BTreeMap::<u32, Vec<ActiveBackendPoint>>::new();
            for type_id in segment
                .type_ids()
                .filter(|type_id| pg_activity_layout(*type_id))
            {
                activity.insert(type_id, active_backend_points(&segment, type_id)?);
            }
            postgres_health_points(&metadata, &combined_active_points(&activity))
                .and_then(|points| points.last().copied())
        } else {
            None
        }
    } else {
        None
    };
    Ok(HealthSeed { os, postgres })
}

/// Return the complete current allowlist for a captured segment.
#[must_use]
pub fn keys(segment: &Segment) -> Vec<SeriesKey> {
    let mut keys = vec![
        SeriesKey::OS_HEALTH,
        SeriesKey::OVERALL_HEALTH,
        SeriesKey::POSTGRES_HEALTH,
        SeriesKey {
            kind: SeriesKind::Findings,
            type_id: DERIVED_HEALTH_TYPE_ID,
        },
    ];
    for type_id in segment.type_ids() {
        if pg_database_layout(type_id) {
            keys.push(SeriesKey {
                kind: SeriesKind::PgTransactionsPerSecond,
                type_id,
            });
        } else if pg_activity_layout(type_id) {
            keys.push(SeriesKey {
                kind: SeriesKind::PgActiveBackends,
                type_id,
            });
        }
        if finding_layout(type_id) {
            keys.push(SeriesKey {
                kind: SeriesKind::Findings,
                type_id,
            });
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Build every allowlisted presentation series present in a segment.
///
/// # Errors
///
/// Returns a production-reader or dictionary-resolution failure.
pub fn build(segment: &Segment) -> Result<Index, BuildError> {
    build_selected(segment, &keys(segment))
}

/// Build selected allowlisted series for a captured active response.
///
/// # Errors
///
/// Returns a production-reader or dictionary-resolution failure.
pub fn build_selected(segment: &Segment, requested: &[SeriesKey]) -> Result<Index, BuildError> {
    let finder = FindingBuilder::new(segment, requested)?;
    let mut index = build_selected_series(segment, requested, HealthSeed::default())?;
    index.blocks.extend(finder.finish(segment, &index)?);
    index.blocks.sort_by_key(SeriesBlock::key);
    Ok(index)
}

/// Build the complete index while reading earlier finished ZMS through the
/// same production reader when a comparison needs prior values.
///
/// # Errors
///
/// Returns a production-reader, dictionary-resolution, or build failure.
pub fn build_from_reader(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segment: &Segment,
) -> Result<Index, BuildError> {
    let requested = keys(segment);
    build_selected_from_reader(reader, segment_ref, segment, &requested)
}

pub(crate) fn build_selected_from_reader(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segment: &Segment,
    requested: &[SeriesKey],
) -> Result<Index, BuildError> {
    let mut finder = FindingBuilder::new(segment, requested)?;
    let listing = reader.catalog_segments(..segment_ref.min_ts())?;
    let predecessor = listing
        .segments
        .iter()
        .filter(|prior| prior.kind() == SegmentKind::Finished)
        .max_by_key(|prior| (prior.max_ts(), prior.id()));
    let health_seed = predecessor_health_seed(reader, predecessor, requested)?;
    // Include one segment before the 15-minute comparison window.
    let mut priors: Vec<_> = listing
        .segments
        .into_iter()
        .filter(|prior| prior.kind() == SegmentKind::Finished)
        .filter(|prior| finder.needs(prior))
        .collect();
    priors.sort_by_key(SegmentRef::min_ts);
    let inside = priors
        .iter()
        .position(|prior| prior.max_ts() >= finder.window_start())
        .unwrap_or(priors.len());
    let from = inside.saturating_sub(1);
    for prior_ref in priors.drain(from..) {
        let prior = reader.open_segment(&prior_ref)?;
        finder.observe_prior(&prior)?;
    }
    let mut index = build_selected_series(segment, requested, health_seed)?;
    index.blocks.extend(finder.finish(segment, &index)?);
    index.blocks.sort_by_key(SeriesBlock::key);
    Ok(index)
}

fn build_selected_series(
    segment: &Segment,
    requested: &[SeriesKey],
    health_seed: HealthSeed,
) -> Result<Index, BuildError> {
    let mut requested = requested.to_vec();
    requested.sort_unstable();
    requested.dedup();
    let mut blocks = Vec::with_capacity(requested.len());
    let wants_health = requested.iter().any(|key| {
        matches!(
            key.kind,
            SeriesKind::OsHealth | SeriesKind::OverallHealth | SeriesKind::PostgresHealth
        )
    });
    let metadata = wants_health.then(|| health_metadata(segment)).transpose()?;
    let os_points = if wants_health {
        health_points(segment, health_seed.os)?
    } else {
        Vec::new()
    };
    let needs_pg_health = requested.iter().any(|key| {
        matches!(
            key.kind,
            SeriesKind::PostgresHealth | SeriesKind::OverallHealth
        )
    });
    let mut activity = BTreeMap::<u32, Vec<ActiveBackendPoint>>::new();
    for type_id in segment
        .type_ids()
        .filter(|type_id| pg_activity_layout(*type_id))
    {
        let raw_requested = requested.contains(&SeriesKey {
            kind: SeriesKind::PgActiveBackends,
            type_id,
        });
        if raw_requested || needs_pg_health {
            activity.insert(type_id, active_backend_points(segment, type_id)?);
        }
    }
    let combined_active = combined_active_points(&activity);
    let postgres_points = metadata
        .as_ref()
        .and_then(|metadata| postgres_health_points(metadata, &combined_active));

    for key in requested {
        match key.kind {
            SeriesKind::OsHealth if key == SeriesKey::OS_HEALTH => {
                blocks.push(SeriesBlock::OsHealth(os_points.clone()));
            }
            SeriesKind::OverallHealth if key == SeriesKey::OVERALL_HEALTH => {
                let metadata = metadata.as_ref().ok_or(BuildError::InvalidMetadata)?;
                blocks.push(SeriesBlock::OverallHealth(overall_points(
                    &os_points,
                    postgres_points.as_deref(),
                    health_seed.postgres,
                    metadata,
                )));
            }
            SeriesKind::PostgresHealth if key == SeriesKey::POSTGRES_HEALTH => {
                if let Some(points) = &postgres_points {
                    blocks.push(SeriesBlock::PostgresHealth(points.clone()));
                }
            }
            SeriesKind::PgTransactionsPerSecond
                if pg_database_layout(key.type_id) && segment.rows_of(key.type_id).is_some() =>
            {
                blocks.push(SeriesBlock::PgTransactions {
                    type_id: key.type_id,
                    points: transaction_points(segment, key.type_id)?,
                });
            }
            SeriesKind::PgActiveBackends
                if pg_activity_layout(key.type_id) && segment.rows_of(key.type_id).is_some() =>
            {
                blocks.push(SeriesBlock::PgActiveBackends {
                    type_id: key.type_id,
                    points: activity.remove(&key.type_id).unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    blocks.sort_by_key(SeriesBlock::key);
    Ok(Index { blocks })
}

#[derive(Debug, Clone, Copy)]
struct HealthMetadata {
    timestamp: i64,
    postgresql_enabled: Option<bool>,
    postgresql_effective_cpus: Option<u32>,
    postgresql_interval_seconds: u64,
}

fn health_metadata(segment: &Segment) -> Result<HealthMetadata, BuildError> {
    if segment.rows_of(INSTANCE_METADATA_TYPE_ID).is_none() {
        return Ok(HealthMetadata {
            timestamp: segment.min_ts(),
            postgresql_enabled: None,
            postgresql_effective_cpus: None,
            postgresql_interval_seconds: 0,
        });
    }
    let mut facts = None;
    let mut rows = 0_usize;
    segment.visit_rows(
        INSTANCE_METADATA_TYPE_ID,
        &[
            "ts",
            "postgresql_enabled",
            "postgresql_effective_cpus",
            "postgresql_interval_seconds",
        ],
        0,
        usize::MAX,
        |_ordinal, row| {
            rows = rows.saturating_add(1);
            let capacity = match row.get("postgresql_effective_cpus") {
                Some(Cell::U32(value)) => Some(*value),
                Some(Cell::Null) => None,
                _ => return true,
            };
            if let (
                Some(Cell::Ts(timestamp)),
                Some(Cell::Bool(postgresql_enabled)),
                Some(Cell::U64(postgresql_interval_seconds)),
            ) = (
                row.get("ts"),
                row.get("postgresql_enabled"),
                row.get("postgresql_interval_seconds"),
            ) {
                facts = Some(HealthMetadata {
                    timestamp: *timestamp,
                    postgresql_enabled: Some(*postgresql_enabled),
                    postgresql_effective_cpus: capacity,
                    postgresql_interval_seconds: *postgresql_interval_seconds,
                });
            }
            true
        },
    )?;
    if rows != 1 {
        return Err(BuildError::InvalidMetadata);
    }
    facts.ok_or(BuildError::InvalidMetadata)
}

fn combined_active_points(
    activity: &BTreeMap<u32, Vec<ActiveBackendPoint>>,
) -> Vec<(i64, Option<u32>)> {
    let mut counts = BTreeMap::<i64, Option<u32>>::new();
    for points in activity.values() {
        for point in points {
            counts
                .entry(point.timestamp)
                .and_modify(|count| *count = None)
                .or_insert(Some(point.count));
        }
    }
    counts.into_iter().collect()
}

fn postgres_health_points(
    metadata: &HealthMetadata,
    active: &[(i64, Option<u32>)],
) -> Option<Vec<HealthPoint>> {
    if metadata.postgresql_enabled != Some(true) {
        return None;
    }
    if active.is_empty() {
        return Some(vec![HealthPoint {
            timestamp: metadata.timestamp,
            value: None,
        }]);
    }
    Some(
        active
            .iter()
            .map(|(timestamp, active)| HealthPoint {
                timestamp: *timestamp,
                value: active
                    .zip(metadata.postgresql_effective_cpus)
                    .and_then(|(active, cpus)| postgres_penalty(active, cpus))
                    .map(|penalty| 100_u8.saturating_sub(penalty)),
            })
            .collect(),
    )
}

fn overall_points(
    os: &[HealthPoint],
    postgres: Option<&[HealthPoint]>,
    predecessor_postgres: Option<HealthPoint>,
    metadata: &HealthMetadata,
) -> Vec<HealthPoint> {
    let postgres_interval = metadata
        .postgresql_interval_seconds
        .checked_mul(1_000_000)
        .and_then(|value| i64::try_from(value).ok());
    os.iter()
        .map(|point| {
            let postgres_penalty = match metadata.postgresql_enabled {
                Some(false) => SourcePenalty::Disabled,
                Some(true) => {
                    latest_postgres_point(postgres, predecessor_postgres, point.timestamp)
                        .filter(|candidate| {
                            postgres_interval.is_some_and(|interval| {
                                point.timestamp.saturating_sub(candidate.timestamp) <= interval
                            })
                        })
                        .and_then(|candidate| candidate.value)
                        .map_or(SourcePenalty::Unknown, |health| {
                            SourcePenalty::Known(100_u8.saturating_sub(health))
                        })
                }
                None => SourcePenalty::Unknown,
            };
            HealthPoint {
                timestamp: point.timestamp,
                value: overall_health(point.value, postgres_penalty),
            }
        })
        .collect()
}

fn latest_postgres_point(
    current: Option<&[HealthPoint]>,
    predecessor: Option<HealthPoint>,
    timestamp: i64,
) -> Option<HealthPoint> {
    current
        .and_then(|points| {
            points
                .iter()
                .rev()
                .find(|candidate| candidate.timestamp <= timestamp)
                .copied()
        })
        .or_else(|| predecessor.filter(|candidate| candidate.timestamp <= timestamp))
}

fn transaction_points(
    segment: &Segment,
    type_id: u32,
) -> Result<Vec<TransactionPoint>, ReaderError> {
    let mut previous: BTreeMap<u32, (i64, i128)> = BTreeMap::new();
    let mut points = Vec::new();
    segment.visit_rows(
        type_id,
        &["ts", "datid", "xact_commit", "xact_rollback"],
        0,
        usize::MAX,
        |_ordinal, row| {
            let (
                Some(Cell::Ts(timestamp)),
                Some(Cell::U32(datid)),
                Some(Cell::I64(commit)),
                Some(Cell::I64(rollback)),
            ) = (
                row.get("ts"),
                row.get("datid"),
                row.get("xact_commit"),
                row.get("xact_rollback"),
            )
            else {
                return true;
            };
            let total = i128::from(*commit) + i128::from(*rollback);
            let value = previous.get(datid).and_then(|(before_ts, before)| {
                transaction_rate(*before_ts, *before, *timestamp, total)
            });
            previous.insert(*datid, (*timestamp, total));
            points.push(TransactionPoint {
                timestamp: *timestamp,
                datid: *datid,
                value,
            });
            true
        },
    )?;
    points.sort_by_key(|point| (point.datid, point.timestamp));
    Ok(points)
}

fn transaction_rate(before_ts: i64, before: i128, timestamp: i64, total: i128) -> Option<f64> {
    let elapsed = timestamp.checked_sub(before_ts)?;
    let delta = total.checked_sub(before)?;
    if elapsed <= 0 || delta < 0 {
        return None;
    }
    let rate = integer_as_f64(delta)? * 1_000_000.0 / integer_as_f64(i128::from(elapsed))?;
    (rate.is_finite() && rate >= 0.0).then_some(rate)
}

pub(crate) fn integer_as_f64(value: i128) -> Option<f64> {
    let mut value = u128::try_from(value).ok()?;
    let mut words = [0_u32; 4];
    for word in words.iter_mut().rev() {
        *word = u32::try_from(value & u128::from(u32::MAX)).ok()?;
        value >>= 32;
    }
    Some(words.into_iter().fold(0.0, |number, word| {
        number.mul_add(4_294_967_296.0, f64::from(word))
    }))
}

fn active_backend_points(
    segment: &Segment,
    type_id: u32,
) -> Result<Vec<ActiveBackendPoint>, BuildError> {
    let mut ids = HashSet::new();
    segment.visit_rows(type_id, &["state"], 0, usize::MAX, |_ordinal, row| {
        if let Some(Cell::StrId(id)) = row.get("state") {
            ids.insert(*id);
        }
        true
    })?;
    let dictionary = segment.dictionary_for(&ids)?;
    let mut active_ids = HashSet::new();
    for id in ids {
        match dictionary.resolve(id) {
            Some(Resolved::Str(b"active")) => {
                active_ids.insert(id);
            }
            Some(Resolved::Str(_) | Resolved::Blob(_)) => {}
            None => return Err(BuildError::UnresolvedState(id)),
        }
    }

    let mut counts = BTreeMap::<i64, u32>::new();
    segment.visit_rows(type_id, &["ts", "state"], 0, usize::MAX, |_ordinal, row| {
        let Some(Cell::Ts(timestamp)) = row.get("ts") else {
            return true;
        };
        let count = counts.entry(*timestamp).or_default();
        if row
            .get("state")
            .is_some_and(|cell| matches!(cell, Cell::StrId(id) if active_ids.contains(id)))
        {
            *count = count.saturating_add(1);
        }
        true
    })?;
    Ok(counts
        .into_iter()
        .map(|(timestamp, count)| ActiveBackendPoint { timestamp, count })
        .collect())
}

#[derive(Debug, Default)]
struct PartialStall {
    cpu: Option<i64>,
    memory: Option<i64>,
    io: Option<i64>,
}

/// Visit full-resolution derived OS health points in timestamp order.
///
/// # Errors
///
/// Returns a production-reader failure for either input section.
pub fn visit_health_points(
    segment: &Segment,
    keep_going: impl FnMut() -> bool,
    visitor: impl FnMut(HealthPoint) -> bool,
) -> Result<(), ReaderError> {
    visit_health_points_with_seed(segment, None, keep_going, visitor)
}

fn visit_health_points_with_seed(
    segment: &Segment,
    mut seed: Option<StallSnapshot>,
    keep_going: impl FnMut() -> bool,
    mut visitor: impl FnMut(HealthPoint) -> bool,
) -> Result<(), ReaderError> {
    let mut previous = None;
    visit_stall_snapshots(segment, keep_going, |snapshot| {
        if let Some(seed) = seed.take()
            && seed.identity == snapshot.identity
        {
            previous = seed.stall.map(|stall| (seed.timestamp, stall));
        }
        let value = previous.and_then(|(before_ts, before)| {
            snapshot
                .stall
                .and_then(|after| health(before, before_ts, after, snapshot.timestamp))
        });
        previous = snapshot.stall.map(|stall| (snapshot.timestamp, stall));
        visitor(HealthPoint {
            timestamp: snapshot.timestamp,
            value,
        })
    })
}

fn visit_stall_snapshots(
    segment: &Segment,
    mut keep_going: impl FnMut() -> bool,
    mut visitor: impl FnMut(StallSnapshot) -> bool,
) -> Result<(), ReaderError> {
    if segment.rows_of(OS_PSI_TYPE_ID).is_none() || !keep_going() {
        return Ok(());
    }
    let mut running = true;
    let mut environment = None;
    let mut boot_time = None;
    let metadata_type_id = if segment.rows_of(INSTANCE_METADATA_TYPE_ID).is_some() {
        Some(INSTANCE_METADATA_TYPE_ID)
    } else if segment.rows_of(INSTANCE_METADATA_V1_TYPE_ID).is_some() {
        Some(INSTANCE_METADATA_V1_TYPE_ID)
    } else {
        None
    };
    if let Some(metadata_type_id) = metadata_type_id {
        segment.visit_rows(
            metadata_type_id,
            &["environment", "btime"],
            0,
            usize::MAX,
            |_ordinal, row| {
                running = keep_going();
                if !running {
                    return false;
                }
                if let Some(Cell::U32(value)) = row.get("environment") {
                    environment = Some(*value);
                }
                if let Some(Cell::Ts(value)) = row.get("btime") {
                    boot_time = Some(*value);
                }
                true
            },
        )?;
    }
    if !running {
        return Ok(());
    }
    let mut snapshots: BTreeMap<i64, PartialStall> = BTreeMap::new();
    segment.visit_rows(
        OS_PSI_TYPE_ID,
        &["ts", "resource", "some_total", "scope"],
        0,
        usize::MAX,
        |_ordinal, row| {
            running = keep_going();
            if !running {
                return false;
            }
            let Some(Cell::Ts(ts)) = row.get("ts") else {
                return true;
            };
            let snapshot = snapshots.entry(*ts).or_default();
            let (Some(Cell::U32(resource)), Some(Cell::I64(total)), Some(Cell::U32(scope))) =
                (row.get("resource"), row.get("some_total"), row.get("scope"))
            else {
                return true;
            };
            let matching_scope = match environment {
                Some(value) if value == u32::from(Environment::Machine.as_u8()) => *scope == HOST,
                Some(value) if value == u32::from(Environment::Container.as_u8()) => {
                    matches!(*scope, POD | CONTAINER)
                }
                _ => false,
            };
            if matching_scope {
                match *resource {
                    CPU => snapshot.cpu = Some(*total),
                    MEMORY => snapshot.memory = Some(*total),
                    IO => snapshot.io = Some(*total),
                    _ => {}
                }
            }
            true
        },
    )?;
    if !running {
        return Ok(());
    }

    for (timestamp, snapshot) in snapshots {
        if !keep_going() {
            break;
        }
        let current = match (snapshot.cpu, snapshot.memory, snapshot.io) {
            (Some(cpu), Some(memory), Some(io)) => Some(Stall { cpu, memory, io }),
            _ => None,
        };
        if !visitor(StallSnapshot {
            timestamp,
            identity: SnapshotIdentity {
                environment,
                boot_time,
            },
            stall: current,
        }) {
            break;
        }
    }
    Ok(())
}

fn last_stall_snapshot(segment: &Segment) -> Result<Option<StallSnapshot>, ReaderError> {
    let mut last = None;
    visit_stall_snapshots(
        segment,
        || true,
        |snapshot| {
            last = Some(snapshot);
            true
        },
    )?;
    Ok(last)
}

fn health_points(
    segment: &Segment,
    seed: Option<StallSnapshot>,
) -> Result<Vec<HealthPoint>, ReaderError> {
    let mut points = Vec::new();
    visit_health_points_with_seed(
        segment,
        seed,
        || true,
        |point| {
            points.push(point);
            true
        },
    )?;
    Ok(points)
}

#[cfg(test)]
mod tests;
