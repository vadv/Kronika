mod direct;

use std::collections::{BTreeMap, BTreeSet};

use kronika_reader::{Cell, Segment, SegmentRef};

use crate::Index;
use crate::build::{ActiveBackendSample, BuildError};
use crate::findings::{
    Finding, FindingBlock, FindingKind, MAX_FINDINGS_PER_BLOCK, MAX_LOG_ERROR_CATEGORY,
    PG_LOG_ERRORS_TYPE_ID,
};
use crate::series::{SeriesBlock, SeriesKey, SeriesKind};
use crate::{SemanticBoundary, SemanticDefinition, SemanticOperator, SemanticOrigin, SemanticUnit};

use self::direct::CpuRaw;

const OS_CPU: u32 = 1_102_001;
const OS_MEMINFO: u32 = 1_104_001;
const OS_LOADAVG: u32 = 1_105_001;
const OS_VMSTAT: u32 = 1_106_001;
const OS_MOUNTINFO: u32 = 1_112_002;
const PG_LOCKS_V1: u32 = 1_011_001;
const PG_LOCKS_V2: u32 = 1_011_002;
const PG_STAT_ARCHIVER: u32 = 1_008_001;
const OS_CGROUP_MEMORY_V1: u32 = 1_202_001;
const OS_CGROUP_MEMORY_V2: u32 = 1_202_002;
const PG_LOG_SLOW_QUERIES: u32 = 2_004_001;
const PG_LOG_EVENT_LAYOUTS: [u32; 6] = [
    PG_LOG_ERRORS_TYPE_ID,
    2_002_001,
    2_003_001,
    PG_LOG_SLOW_QUERIES,
    2_005_002,
    2_006_001,
];
const FIFTEEN_MINUTES_US: i64 = 15 * 60 * 1_000_000;

const OVERALL_HEALTH_FIELD: u16 = 1;
const CPU_IDLE_FIELD: u16 = 5;
const MEM_AVAILABLE_FIELD: u16 = 3;
const LOAD1_FIELD: u16 = 1;
const OOM_KILL_FIELD: u16 = 11;
const MOUNT_FREE_BYTES_FIELD: u16 = 9;
const SLOW_QUERY_DURATION_FIELD: u16 = 6;
const LOCKS_BLOCKED_BY_FIELD: u16 = 2;
const DATABASE_DEADLOCKS_FIELD: u16 = 16;
const FROZEN_XID_AGE_FIELD: u16 = 20;
const MIN_MXID_AGE_FIELD: u16 = 21;
const CHECKSUM_FAILURES_FIELD: u16 = 25;
const SESSIONS_FATAL_FIELD: u16 = 32;
const SESSIONS_KILLED_FIELD: u16 = 33;
const ARCHIVER_FAILED_COUNT_FIELD: u16 = 4;
const LOG_ERROR_CATEGORY_FIELD: u16 = 4;
const ACTIVITY_STATE_V1_FIELD: u16 = 7;
const ACTIVITY_STATE_V2_FIELD: u16 = 8;
const CGROUP_OOM_KILL_V1_FIELD: u16 = 12;
const CGROUP_OOM_KILL_V2_FIELD: u16 = 13;
const DATA_CORRUPTION_CATEGORY: u8 = 5;
const PERCENT_SCALE: u8 = 100;
const CPU_BUSY_PERCENT: u8 = 80;
const LOAD_PER_CPU: u32 = 2;
const MEMORY_AVAILABLE_PERCENT: u8 = 10;
const MOUNT_USED_PERCENT: u8 = 90;
const SLOW_QUERY_DURATION_MS: i64 = 5_000;
const ACTIVE_BACKENDS_PER_CPU: u32 = 2;
const OVERALL_HEALTH_BOUNDARY: u8 = 50;
/// `PostgreSQL`'s own `vacuum_failsafe_age` / `vacuum_multixact_failsafe_age` default.
const WRAPAROUND_AGE_THRESHOLD: i64 = 1_600_000_000;
const EVENT_TIMESTAMP_FIELD: u16 = 0;

/// Descriptor for the exact recorded lock boundary evaluated by this module.
pub const LOCKS_BLOCKED_BY_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_locks.blocked_by_nonempty",
    logical_name: Some("pg_locks"),
    field: Some("blocked_by"),
    origin: SemanticOrigin::KronikaDerived,
    unit: None,
    formula: None,
    operands: &["blocked_by"],
    boundary: Some(SemanticBoundary::Nonempty),
};

const CPU_BUSY_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.os_cpu.cpu_busy",
    logical_name: Some("os_cpu"),
    field: Some("idle"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Percent),
    formula: Some("100 * busy_ticks / total_ticks"),
    operands: &[
        "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal",
    ],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Gte,
        numerator: CPU_BUSY_PERCENT as i64,
        denominator: 1,
    }),
};

const LOAD_PER_CPU_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.os_loadavg.load_per_cpu",
    logical_name: Some("os_loadavg"),
    field: Some("load1"),
    origin: SemanticOrigin::KronikaDerived,
    unit: None,
    formula: Some("load1 / online_cpu_count"),
    operands: &["load1", "online_cpu_count"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Gte,
        numerator: LOAD_PER_CPU as i64,
        denominator: 1,
    }),
};

const MEMORY_AVAILABLE_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.os_meminfo.memory_available",
    logical_name: Some("os_meminfo"),
    field: Some("mem_available"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Percent),
    formula: Some("100 * mem_available / mem_total"),
    operands: &["mem_available", "mem_total"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Lte,
        numerator: MEMORY_AVAILABLE_PERCENT as i64,
        denominator: 1,
    }),
};

const MOUNT_USED_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.os_mountinfo.mount_used",
    logical_name: Some("os_mountinfo"),
    field: Some("free_bytes"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Percent),
    formula: Some("100 * (total_bytes - free_bytes) / total_bytes"),
    operands: &["total_bytes", "free_bytes"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Gte,
        numerator: MOUNT_USED_PERCENT as i64,
        denominator: 1,
    }),
};

const SLOW_QUERY_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_log_slow_queries.duration",
    logical_name: Some("pg_log_slow_queries"),
    field: Some("max_duration_ms"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Milliseconds),
    formula: None,
    operands: &["max_duration_ms"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Gte,
        numerator: SLOW_QUERY_DURATION_MS,
        denominator: 1,
    }),
};

const OOM_KILL_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.os_vmstat.oom_kill_increase",
    logical_name: Some("os_vmstat"),
    field: Some("oom_kill"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["oom_kill"],
    boundary: Some(SemanticBoundary::Increase),
};

const ARCHIVER_FAILURE_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_archiver.failed_count_increase",
    logical_name: Some("pg_stat_archiver"),
    field: Some("failed_count"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["failed_count"],
    boundary: Some(SemanticBoundary::Increase),
};

const DATABASE_DEADLOCK_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_database.deadlocks_increase",
    logical_name: Some("pg_stat_database"),
    field: Some("deadlocks"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["deadlocks"],
    boundary: Some(SemanticBoundary::Increase),
};

const DATABASE_CHECKSUM_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_database.checksum_failures_increase",
    logical_name: Some("pg_stat_database"),
    field: Some("checksum_failures"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["checksum_failures"],
    boundary: Some(SemanticBoundary::Increase),
};

const DATABASE_FATAL_SESSION_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_database.sessions_fatal_increase",
    logical_name: Some("pg_stat_database"),
    field: Some("sessions_fatal"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["sessions_fatal"],
    boundary: Some(SemanticBoundary::Increase),
};

const DATABASE_KILLED_SESSION_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_database.sessions_killed_increase",
    logical_name: Some("pg_stat_database"),
    field: Some("sessions_killed"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["sessions_killed"],
    boundary: Some(SemanticBoundary::Increase),
};

const DATABASE_XID_AGE_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_database.frozen_xid_age",
    logical_name: Some("pg_stat_database"),
    field: Some("frozen_xid_age"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["frozen_xid_age"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Gte,
        numerator: WRAPAROUND_AGE_THRESHOLD,
        denominator: 1,
    }),
};

const DATABASE_MXID_AGE_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_database.min_mxid_age",
    logical_name: Some("pg_stat_database"),
    field: Some("min_mxid_age"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["min_mxid_age"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Gte,
        numerator: WRAPAROUND_AGE_THRESHOLD,
        denominator: 1,
    }),
};

const CGROUP_OOM_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.os_cgroup_memory.oom_kill_increase",
    logical_name: Some("os_cgroup_memory"),
    field: Some("oom_kill"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Count),
    formula: None,
    operands: &["oom_kill"],
    boundary: Some(SemanticBoundary::Increase),
};

const ACTIVE_BACKENDS_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_stat_activity.active_backends_per_cpu",
    logical_name: Some("pg_stat_activity"),
    field: Some("state"),
    origin: SemanticOrigin::KronikaDerived,
    unit: None,
    formula: Some("active_backends / effective_postgres_cpu"),
    operands: &["active_backends", "effective_postgres_cpu"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Gt,
        numerator: ACTIVE_BACKENDS_PER_CPU as i64,
        denominator: 1,
    }),
};

const OVERALL_HEALTH_FINDING_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.health.overall_health",
    logical_name: Some("health"),
    field: Some("overall_health"),
    origin: SemanticOrigin::KronikaDerived,
    unit: Some(SemanticUnit::Percent),
    formula: None,
    operands: &["overall_health"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Lt,
        numerator: OVERALL_HEALTH_BOUNDARY as i64,
        denominator: 1,
    }),
};

const DATA_CORRUPTION_SEMANTIC: SemanticDefinition = SemanticDefinition {
    id: "finding.pg_log_errors.data_corruption_category",
    logical_name: Some("pg_log_errors"),
    field: Some("category"),
    origin: SemanticOrigin::KronikaDerived,
    unit: None,
    formula: None,
    operands: &["category"],
    boundary: Some(SemanticBoundary::Compare {
        operator: SemanticOperator::Eq,
        numerator: DATA_CORRUPTION_CATEGORY as i64,
        denominator: 1,
    }),
};

/// Every accepted known-bad boundary evaluated by `kronika-index`.
pub const FINDING_SEMANTICS: &[SemanticDefinition] = &[
    CPU_BUSY_SEMANTIC,
    LOAD_PER_CPU_SEMANTIC,
    MEMORY_AVAILABLE_SEMANTIC,
    MOUNT_USED_SEMANTIC,
    SLOW_QUERY_SEMANTIC,
    OOM_KILL_SEMANTIC,
    ARCHIVER_FAILURE_SEMANTIC,
    DATABASE_DEADLOCK_SEMANTIC,
    DATABASE_CHECKSUM_SEMANTIC,
    DATABASE_FATAL_SESSION_SEMANTIC,
    DATABASE_KILLED_SESSION_SEMANTIC,
    DATABASE_XID_AGE_SEMANTIC,
    DATABASE_MXID_AGE_SEMANTIC,
    CGROUP_OOM_SEMANTIC,
    LOCKS_BLOCKED_BY_SEMANTIC,
    ACTIVE_BACKENDS_SEMANTIC,
    OVERALL_HEALTH_FINDING_SEMANTIC,
    DATA_CORRUPTION_SEMANTIC,
];

#[derive(Debug)]
pub(crate) struct FindingBuilder {
    requested: BTreeSet<u32>,
    cutoff: i64,
    cpu_before: Option<CpuRaw>,
    oom_before: Option<(i64, Option<i64>)>,
    archiver_before: Option<(i64, i64)>,
    deadlocks_before: BTreeMap<(u32, u32), (i64, i64)>,
    checksum_failures_before: BTreeMap<(u32, u32), (i64, Option<i64>)>,
    sessions_before: BTreeMap<(u32, u32), (i64, i64, i64)>,
    cgroup_oom_before: BTreeMap<(u32, u64), (i64, i64)>,
}

impl FindingBuilder {
    /// Discover only concrete series that occur in the target segment.
    pub(crate) fn new(segment: &Segment, requested: &[SeriesKey]) -> Self {
        let requested: BTreeSet<u32> = requested
            .iter()
            .filter(|key| key.kind == SeriesKind::Findings)
            .map(|key| key.type_id)
            .filter(|type_id| finding_layout(*type_id))
            .collect();
        Self {
            requested,
            cutoff: segment.min_ts().saturating_sub(FIFTEEN_MINUTES_US),
            cpu_before: None,
            oom_before: None,
            archiver_before: None,
            deadlocks_before: BTreeMap::new(),
            checksum_failures_before: BTreeMap::new(),
            sessions_before: BTreeMap::new(),
            cgroup_oom_before: BTreeMap::new(),
        }
    }

    /// Earliest preferred timestamp for adjacent counter inputs.
    pub(crate) const fn window_start(&self) -> i64 {
        self.cutoff
    }

    /// Whether a prior segment can contribute a required predecessor.
    pub(crate) fn needs(&self, segment: &SegmentRef) -> bool {
        segment.sections().iter().any(|section| {
            self.requested.contains(&section.type_id)
                && needs_prior_rows(section.type_id)
                && section.rows != 0
        })
    }

    pub(crate) fn observe_prior(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if self.requested.contains(&OS_CPU) {
            self.observe_prior_cpu(segment)?;
        }
        if self.requested.contains(&OS_VMSTAT) {
            self.observe_prior_oom(segment)?;
        }
        if self.requested.contains(&PG_STAT_ARCHIVER) {
            self.observe_prior_archiver(segment)?;
        }
        for type_id in database_layouts() {
            if self.requested.contains(&type_id) {
                self.observe_prior_database_counters(segment, type_id)?;
            }
        }
        for type_id in cgroup_memory_layouts() {
            if self.requested.contains(&type_id) {
                self.observe_prior_cgroup_oom(segment, type_id)?;
            }
        }
        Ok(())
    }

    fn find_log_events(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        for type_id in PG_LOG_EVENT_LAYOUTS {
            if !self.requested.contains(&type_id) || segment.rows_of(type_id).is_none() {
                continue;
            }
            let event_hits = hits.entry(type_id).or_default();
            let fields: &[&str] = if type_id == PG_LOG_ERRORS_TYPE_ID {
                &["ts", "category"]
            } else {
                &["ts"]
            };
            let mut invalid_category = false;
            segment.visit_rows(type_id, fields, 0, usize::MAX, |ordinal, row| {
                if let (Some(Cell::Ts(timestamp)), Some(row_ordinal)) =
                    (row.get("ts"), u32::try_from(ordinal).ok())
                {
                    let category = if type_id == PG_LOG_ERRORS_TYPE_ID {
                        let Some(Cell::U32(value)) = row.get("category") else {
                            invalid_category = true;
                            return false;
                        };
                        let Ok(value @ 0..=MAX_LOG_ERROR_CATEGORY) = u8::try_from(*value) else {
                            invalid_category = true;
                            return false;
                        };
                        Some(value)
                    } else {
                        None
                    };
                    event_hits.push(Finding {
                        kind: FindingKind::Event,
                        field_ordinal: EVENT_TIMESTAMP_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                        category,
                    });
                    if category == Some(DATA_CORRUPTION_CATEGORY) {
                        event_hits.push(Finding {
                            kind: FindingKind::KnownBad,
                            field_ordinal: LOG_ERROR_CATEGORY_FIELD,
                            row_ordinal,
                            timestamp: *timestamp,
                            category,
                        });
                    }
                }
                true
            })?;
            if invalid_category {
                return Err(BuildError::InvalidLogErrorCategory);
            }
        }
        Ok(())
    }

    /// Build every requested finding block, including empty supported blocks.
    pub(crate) fn finish(
        mut self,
        segment: &Segment,
        index: &Index,
        active_samples: &BTreeMap<u32, Vec<ActiveBackendSample>>,
        postgres_cpus: Option<u32>,
    ) -> Result<Vec<SeriesBlock>, BuildError> {
        let mut hits = BTreeMap::<u32, Vec<Finding>>::new();
        for type_id in &self.requested {
            hits.insert(*type_id, Vec::new());
        }

        self.find_log_events(segment, &mut hits)?;
        self.find_cpu_and_online_count(segment, &mut hits)?;
        self.find_memory(segment, &mut hits)?;
        self.find_mounts(segment, &mut hits)?;
        self.find_slow_queries(segment, &mut hits)?;
        self.find_oom(segment, &mut hits)?;
        self.find_archiver_failures(segment, &mut hits)?;
        for type_id in database_layouts() {
            if self.requested.contains(&type_id) {
                self.find_database_counters(segment, type_id, &mut hits)?;
            }
        }
        for type_id in pg_locks_layouts() {
            if self.requested.contains(&type_id) {
                self.find_lock_contention(segment, type_id, &mut hits)?;
            }
        }
        for type_id in cgroup_memory_layouts() {
            if self.requested.contains(&type_id) {
                self.find_cgroup_oom(segment, type_id, &mut hits)?;
            }
        }
        self.find_active_backends(active_samples, postgres_cpus, &mut hits);
        self.find_overall_health(index, &mut hits);

        Ok(hits
            .into_iter()
            .map(|(type_id, findings)| SeriesBlock::Findings(block(type_id, findings)))
            .collect())
    }
}

pub(crate) fn finding_layout(type_id: u32) -> bool {
    PG_LOG_EVENT_LAYOUTS.contains(&type_id)
        || matches!(
            type_id,
            0 | OS_CPU
                | OS_MEMINFO
                | OS_LOADAVG
                | OS_VMSTAT
                | OS_MOUNTINFO
                | PG_LOCKS_V1
                | PG_LOCKS_V2
                | PG_STAT_ARCHIVER
                | OS_CGROUP_MEMORY_V1
                | OS_CGROUP_MEMORY_V2
                | 1_001_001
                | 1_001_002
                | 1_001_004
                | 1_005_001..=1_005_004
        )
}

/// Accepted known-bad descriptor for one physical finding locator.
#[must_use]
pub const fn finding_semantic(type_id: u32, field_ordinal: u16) -> Option<SemanticDefinition> {
    match (type_id, field_ordinal) {
        (0, OVERALL_HEALTH_FIELD) => Some(OVERALL_HEALTH_FINDING_SEMANTIC),
        (OS_CPU, CPU_IDLE_FIELD) => Some(CPU_BUSY_SEMANTIC),
        (OS_LOADAVG, LOAD1_FIELD) => Some(LOAD_PER_CPU_SEMANTIC),
        (OS_MEMINFO, MEM_AVAILABLE_FIELD) => Some(MEMORY_AVAILABLE_SEMANTIC),
        (OS_VMSTAT, OOM_KILL_FIELD) => Some(OOM_KILL_SEMANTIC),
        (OS_MOUNTINFO, MOUNT_FREE_BYTES_FIELD) => Some(MOUNT_USED_SEMANTIC),
        (PG_STAT_ARCHIVER, ARCHIVER_FAILED_COUNT_FIELD) => Some(ARCHIVER_FAILURE_SEMANTIC),
        (PG_LOG_SLOW_QUERIES, SLOW_QUERY_DURATION_FIELD) => Some(SLOW_QUERY_SEMANTIC),
        (PG_LOG_ERRORS_TYPE_ID, LOG_ERROR_CATEGORY_FIELD) => Some(DATA_CORRUPTION_SEMANTIC),
        (PG_LOCKS_V1 | PG_LOCKS_V2, LOCKS_BLOCKED_BY_FIELD) => Some(LOCKS_BLOCKED_BY_SEMANTIC),
        (OS_CGROUP_MEMORY_V1, CGROUP_OOM_KILL_V1_FIELD)
        | (OS_CGROUP_MEMORY_V2, CGROUP_OOM_KILL_V2_FIELD) => Some(CGROUP_OOM_SEMANTIC),
        (1_001_001, ACTIVITY_STATE_V1_FIELD) | (1_001_002 | 1_001_004, ACTIVITY_STATE_V2_FIELD) => {
            Some(ACTIVE_BACKENDS_SEMANTIC)
        }
        (1_005_001..=1_005_004, DATABASE_DEADLOCKS_FIELD) => Some(DATABASE_DEADLOCK_SEMANTIC),
        (1_005_001..=1_005_004, FROZEN_XID_AGE_FIELD) => Some(DATABASE_XID_AGE_SEMANTIC),
        (1_005_001..=1_005_004, MIN_MXID_AGE_FIELD) => Some(DATABASE_MXID_AGE_SEMANTIC),
        (1_005_002..=1_005_004, CHECKSUM_FAILURES_FIELD) => Some(DATABASE_CHECKSUM_SEMANTIC),
        (1_005_003 | 1_005_004, SESSIONS_FATAL_FIELD) => Some(DATABASE_FATAL_SESSION_SEMANTIC),
        (1_005_003 | 1_005_004, SESSIONS_KILLED_FIELD) => Some(DATABASE_KILLED_SESSION_SEMANTIC),
        _ => None,
    }
}

const fn needs_prior_rows(type_id: u32) -> bool {
    matches!(
        type_id,
        OS_CPU
            | OS_VMSTAT
            | PG_STAT_ARCHIVER
            | OS_CGROUP_MEMORY_V1
            | OS_CGROUP_MEMORY_V2
            | 1_005_001..=1_005_004
    )
}

fn block(type_id: u32, mut findings: Vec<Finding>) -> FindingBlock {
    findings.sort_unstable_by_key(|finding| {
        (
            finding.timestamp,
            finding.row_ordinal,
            finding.field_ordinal,
            finding.kind,
        )
    });
    findings.dedup();
    let total_hits = u32::try_from(findings.len()).unwrap_or(u32::MAX);
    findings.truncate(MAX_FINDINGS_PER_BLOCK);
    FindingBlock {
        type_id,
        total_hits,
        truncated: usize::try_from(total_hits).map_or(true, |total| total > findings.len()),
        findings,
    }
}

const fn optional_i64(cell: Option<&Cell>) -> Option<i64> {
    match cell {
        Some(Cell::I64(value)) => Some(*value),
        _ => None,
    }
}

const fn database_layouts() -> [u32; 4] {
    [1_005_001, 1_005_002, 1_005_003, 1_005_004]
}

const fn pg_locks_layouts() -> [u32; 2] {
    [PG_LOCKS_V1, PG_LOCKS_V2]
}

/// `checksum_failures` is absent on the base `1_005_001` layout.
pub(super) const fn has_checksum(type_id: u32) -> bool {
    matches!(type_id, 1_005_002..=1_005_004)
}

/// `sessions_fatal` / `sessions_killed` arrived with PG14.
pub(super) const fn has_sessions(type_id: u32) -> bool {
    matches!(type_id, 1_005_003 | 1_005_004)
}

const fn cgroup_memory_layouts() -> [u32; 2] {
    [OS_CGROUP_MEMORY_V1, OS_CGROUP_MEMORY_V2]
}

const fn activity_layouts() -> [u32; 3] {
    [1_001_001, 1_001_002, 1_001_004]
}

#[cfg(test)]
mod tests;
