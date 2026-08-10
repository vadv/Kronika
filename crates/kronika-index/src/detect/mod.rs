//! Explicit finding comparisons over production-decoded rows.

mod direct;
mod spikes;

use std::collections::{BTreeMap, BTreeSet};

use kronika_reader::{Cell, Segment, SegmentRef};

use crate::Index;
use crate::build::BuildError;
use crate::findings::{Finding, FindingBlock, MAX_FINDINGS_PER_BLOCK, PriorValue};
use crate::series::{SeriesBlock, SeriesKey, SeriesKind};

use self::direct::CpuRaw;

const OS_PROCESS: u32 = 1_100_001;
const OS_CPU: u32 = 1_102_001;
const OS_MEMINFO: u32 = 1_104_001;
const OS_LOADAVG: u32 = 1_105_001;
const OS_VMSTAT: u32 = 1_106_001;
const OS_MOUNTINFO: u32 = 1_112_001;
const PG_LOG_SLOW_QUERIES: u32 = 2_004_001;
const FIFTEEN_MINUTES_US: i64 = 15 * 60 * 1_000_000;

const OVERALL_HEALTH_FIELD: u16 = 1;
const PROCESS_READ_BYTES_FIELD: u16 = 33;
const CPU_IDLE_FIELD: u16 = 5;
const MEM_AVAILABLE_FIELD: u16 = 3;
const LOAD1_FIELD: u16 = 1;
const OOM_KILL_FIELD: u16 = 11;
const MOUNT_FREE_BYTES_FIELD: u16 = 8;
const SLOW_QUERY_DURATION_FIELD: u16 = 6;
const DATABASE_DEADLOCKS_FIELD: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ProcessId {
    pid: i32,
    starttime: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StatementId {
    queryid: Option<i64>,
    userid: u32,
    dbid: u32,
    toplevel: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
struct ProcessRaw {
    timestamp: i64,
    read_bytes: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
struct StatementRaw {
    timestamp: i64,
    calls: i64,
    total_exec_time: f64,
}

#[derive(Debug)]
struct SpikeHistory<R> {
    before: Option<R>,
    values: Vec<PriorValue>,
}

impl<R> Default for SpikeHistory<R> {
    fn default() -> Self {
        Self {
            before: None,
            values: Vec::new(),
        }
    }
}

/// Temporary state used only while one IDX is being built.
#[derive(Debug)]
pub(crate) struct FindingBuilder {
    requested: BTreeSet<u32>,
    cutoff: i64,
    process: BTreeMap<ProcessId, SpikeHistory<ProcessRaw>>,
    statements: BTreeMap<u32, BTreeMap<StatementId, SpikeHistory<StatementRaw>>>,
    cpu_before: Option<CpuRaw>,
    oom_before: Option<(i64, Option<i64>)>,
    deadlocks_before: BTreeMap<(u32, u32), (i64, i64)>,
}

impl FindingBuilder {
    /// Discover only concrete series that occur in the target segment.
    pub(crate) fn new(segment: &Segment, requested: &[SeriesKey]) -> Result<Self, BuildError> {
        let requested: BTreeSet<u32> = requested
            .iter()
            .filter(|key| key.kind == SeriesKind::Findings)
            .map(|key| key.type_id)
            .filter(|type_id| finding_layout(*type_id))
            .collect();
        let mut builder = Self {
            requested,
            cutoff: segment.min_ts().saturating_sub(FIFTEEN_MINUTES_US),
            process: BTreeMap::new(),
            statements: BTreeMap::new(),
            cpu_before: None,
            oom_before: None,
            deadlocks_before: BTreeMap::new(),
        };
        builder.discover_processes(segment)?;
        builder.discover_statements(segment)?;
        Ok(builder)
    }

    /// Whether a prior segment can contribute a required predecessor.
    pub(crate) fn needs(&self, segment: &SegmentRef) -> bool {
        segment.sections().iter().any(|section| {
            self.requested.contains(&section.type_id)
                && needs_prior_rows(section.type_id)
                && section.rows != 0
        })
    }

    /// Consume relevant rows from one preceding finished ZMS.
    pub(crate) fn observe_prior(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if self.requested.contains(&OS_PROCESS) {
            self.observe_processes(segment, false, &mut Vec::new())?;
        }
        for type_id in statement_layouts() {
            if self.requested.contains(&type_id) {
                self.observe_statements(segment, type_id, false, &mut Vec::new())?;
            }
        }
        if self.requested.contains(&OS_CPU) {
            self.observe_prior_cpu(segment)?;
        }
        if self.requested.contains(&OS_VMSTAT) {
            self.observe_prior_oom(segment)?;
        }
        for type_id in database_layouts() {
            if self.requested.contains(&type_id) {
                self.observe_prior_deadlocks(segment, type_id)?;
            }
        }
        Ok(())
    }

    /// Build every requested finding block, including empty supported blocks.
    pub(crate) fn finish(
        mut self,
        segment: &Segment,
        index: &Index,
    ) -> Result<Vec<SeriesBlock>, BuildError> {
        let mut hits = BTreeMap::<u32, Vec<Finding>>::new();
        for type_id in &self.requested {
            hits.insert(*type_id, Vec::new());
        }

        self.find_cpu_and_online_count(segment, &mut hits)?;
        self.find_memory(segment, &mut hits)?;
        self.find_mounts(segment, &mut hits)?;
        self.find_slow_queries(segment, &mut hits)?;
        self.find_oom(segment, &mut hits)?;
        for type_id in database_layouts() {
            if self.requested.contains(&type_id) {
                self.find_deadlocks(segment, type_id, &mut hits)?;
            }
        }
        if self.requested.contains(&OS_PROCESS) {
            let mut process_hits = Vec::new();
            self.observe_processes(segment, true, &mut process_hits)?;
            hits.insert(OS_PROCESS, process_hits);
        }
        for type_id in statement_layouts() {
            if self.requested.contains(&type_id) {
                let mut statement_hits = Vec::new();
                self.observe_statements(segment, type_id, true, &mut statement_hits)?;
                hits.insert(type_id, statement_hits);
            }
        }
        self.find_active_backends(segment, &mut hits)?;
        self.find_overall_health(index, &mut hits);

        Ok(hits
            .into_iter()
            .map(|(type_id, findings)| SeriesBlock::Findings(block(type_id, findings)))
            .collect())
    }
}

pub(crate) const fn finding_layout(type_id: u32) -> bool {
    matches!(
        type_id,
        0 | OS_PROCESS
            | OS_CPU
            | OS_MEMINFO
            | OS_LOADAVG
            | OS_VMSTAT
            | OS_MOUNTINFO
            | PG_LOG_SLOW_QUERIES
            | 1_001_001..=1_001_003
            | 1_002_002..=1_002_006
            | 1_005_001..=1_005_004
    )
}

const fn needs_prior_rows(type_id: u32) -> bool {
    matches!(
        type_id,
        OS_PROCESS | OS_CPU | OS_VMSTAT | 1_002_002..=1_002_006 | 1_005_001..=1_005_004
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

const fn statement_layouts() -> [u32; 5] {
    [1_002_002, 1_002_003, 1_002_004, 1_002_005, 1_002_006]
}

const fn database_layouts() -> [u32; 4] {
    [1_005_001, 1_005_002, 1_005_003, 1_005_004]
}

const fn activity_layouts() -> [u32; 3] {
    [1_001_001, 1_001_002, 1_001_003]
}

#[cfg(test)]
mod tests;
