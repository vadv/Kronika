//! Explicit finding comparisons over production-decoded rows.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use kronika_reader::{Cell, Resolved, Segment, SegmentRef};

use crate::Index;
use crate::build::{BuildError, INSTANCE_METADATA_TYPE_ID, integer_as_f64};
use crate::findings::{
    Finding, FindingBlock, FindingKind, MAX_FINDINGS_PER_BLOCK, PriorValue, is_upward_spike,
    select_baseline,
};
use crate::series::{SeriesBlock, SeriesKey, SeriesKind};

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

#[derive(Debug, Clone, Copy)]
struct CpuRaw {
    timestamp: i64,
    counters: [i64; 8],
}

#[derive(Debug, Default)]
struct CpuSnapshot {
    aggregate: Option<(u32, CpuRaw)>,
    online: u32,
}

#[derive(Debug, Clone, Copy)]
struct ActiveSnapshot {
    type_id: u32,
    row_ordinal: u32,
    count: u32,
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

    fn discover_processes(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_PROCESS) || segment.rows_of(OS_PROCESS).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            OS_PROCESS,
            &["pid", "starttime"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if let (Some(Cell::I32(pid)), Some(Cell::Ts(starttime))) =
                    (row.get("pid"), row.get("starttime"))
                {
                    self.process
                        .entry(ProcessId {
                            pid: *pid,
                            starttime: *starttime,
                        })
                        .or_default();
                }
                true
            },
        )?;
        Ok(())
    }

    fn discover_statements(&mut self, segment: &Segment) -> Result<(), BuildError> {
        for type_id in statement_layouts() {
            if !self.requested.contains(&type_id) || segment.rows_of(type_id).is_none() {
                continue;
            }
            let columns = statement_columns(type_id);
            let identities = self.statements.entry(type_id).or_default();
            segment.visit_rows(type_id, columns, 0, usize::MAX, |_ordinal, row| {
                if let Some(identity) = statement_identity(type_id, &row) {
                    identities.entry(identity).or_default();
                }
                true
            })?;
        }
        Ok(())
    }

    fn observe_processes(
        &mut self,
        segment: &Segment,
        current: bool,
        hits: &mut Vec<Finding>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(OS_PROCESS).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            OS_PROCESS,
            &["ts", "pid", "starttime", "read_bytes"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (Some(Cell::Ts(timestamp)), Some(Cell::I32(pid)), Some(Cell::Ts(starttime))) =
                    (row.get("ts"), row.get("pid"), row.get("starttime"))
                else {
                    return true;
                };
                let identity = ProcessId {
                    pid: *pid,
                    starttime: *starttime,
                };
                let Some(history) = self.process.get_mut(&identity) else {
                    return true;
                };
                let read_bytes = optional_i64(row.get("read_bytes"));
                let raw = ProcessRaw {
                    timestamp: *timestamp,
                    read_bytes,
                };
                let value = history.before.and_then(|before| process_rate(before, raw));
                history.before = Some(raw);
                if let Some(value) = value {
                    if current {
                        if baseline_is_spike(&history.values, *timestamp, value)
                            && let Some(row_ordinal) = u32::try_from(ordinal).ok()
                        {
                            hits.push(Finding {
                                kind: FindingKind::Spike,
                                field_ordinal: PROCESS_READ_BYTES_FIELD,
                                row_ordinal,
                                timestamp: *timestamp,
                            });
                        }
                        history.values.push(PriorValue {
                            timestamp: *timestamp,
                            value,
                        });
                    } else {
                        push_prior(&mut history.values, self.cutoff, *timestamp, value);
                    }
                }
                true
            },
        )?;
        Ok(())
    }

    fn observe_statements(
        &mut self,
        segment: &Segment,
        type_id: u32,
        current: bool,
        hits: &mut Vec<Finding>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let columns = statement_value_columns(type_id);
        let Some(identities) = self.statements.get_mut(&type_id) else {
            return Ok(());
        };
        segment.visit_rows(type_id, columns, 0, usize::MAX, |ordinal, row| {
            let Some(identity) = statement_identity(type_id, &row) else {
                return true;
            };
            let Some(history) = identities.get_mut(&identity) else {
                return true;
            };
            let (
                Some(Cell::Ts(timestamp)),
                Some(Cell::I64(calls)),
                Some(Cell::F64(total_exec_time)),
            ) = (row.get("ts"), row.get("calls"), row.get("total_exec_time"))
            else {
                return true;
            };
            let raw = StatementRaw {
                timestamp: *timestamp,
                calls: *calls,
                total_exec_time: *total_exec_time,
            };
            let value = history
                .before
                .and_then(|before| statement_average(before, raw));
            history.before = Some(raw);
            if let Some(value) = value {
                if current {
                    if baseline_is_spike(&history.values, *timestamp, value)
                        && let Some(row_ordinal) = u32::try_from(ordinal).ok()
                    {
                        hits.push(Finding {
                            kind: FindingKind::Spike,
                            field_ordinal: statement_total_time_field(type_id),
                            row_ordinal,
                            timestamp: *timestamp,
                        });
                    }
                    history.values.push(PriorValue {
                        timestamp: *timestamp,
                        value,
                    });
                } else {
                    push_prior(&mut history.values, self.cutoff, *timestamp, value);
                }
            }
            true
        })?;
        Ok(())
    }

    fn observe_prior_cpu(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if segment.rows_of(OS_CPU).is_none() {
            return Ok(());
        }
        segment.visit_rows(OS_CPU, cpu_columns(), 0, usize::MAX, |_ordinal, row| {
            if matches!(row.get("scope"), Some(Cell::U32(0)))
                && matches!(row.get("cpu_id"), Some(Cell::I32(-1)))
                && let Some(raw) = cpu_raw(&row)
            {
                self.cpu_before = Some(raw);
            }
            true
        })?;
        Ok(())
    }

    fn observe_prior_oom(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if segment.rows_of(OS_VMSTAT).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            OS_VMSTAT,
            &["ts", "oom_kill", "scope"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if matches!(row.get("scope"), Some(Cell::U32(0)))
                    && let Some(Cell::Ts(timestamp)) = row.get("ts")
                {
                    self.oom_before = Some((*timestamp, optional_i64(row.get("oom_kill"))));
                }
                true
            },
        )?;
        Ok(())
    }

    fn observe_prior_deadlocks(
        &mut self,
        segment: &Segment,
        type_id: u32,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            type_id,
            &["ts", "datid", "deadlocks"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::U32(datid)),
                    Some(Cell::I64(deadlocks)),
                ) = (row.get("ts"), row.get("datid"), row.get("deadlocks"))
                {
                    self.deadlocks_before
                        .insert((type_id, *datid), (*timestamp, *deadlocks));
                }
                true
            },
        )?;
        Ok(())
    }

    fn find_cpu_and_online_count(
        &mut self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(OS_CPU).is_none()
            || !(self.requested.contains(&OS_CPU) || self.requested.contains(&OS_LOADAVG))
        {
            return Ok(());
        }
        let mut snapshots = BTreeMap::<i64, CpuSnapshot>::new();
        segment.visit_rows(OS_CPU, cpu_columns(), 0, usize::MAX, |ordinal, row| {
            let Some(Cell::Ts(timestamp)) = row.get("ts") else {
                return true;
            };
            if !matches!(row.get("scope"), Some(Cell::U32(0))) {
                return true;
            }
            let snapshot = snapshots.entry(*timestamp).or_default();
            match row.get("cpu_id") {
                Some(Cell::I32(-1)) => {
                    if let (Some(raw), Some(row_ordinal)) =
                        (cpu_raw(&row), u32::try_from(ordinal).ok())
                    {
                        snapshot.aggregate = Some((row_ordinal, raw));
                    }
                }
                Some(Cell::I32(cpu_id)) if *cpu_id >= 0 => {
                    snapshot.online = snapshot.online.saturating_add(1);
                }
                _ => {}
            }
            true
        })?;

        if self.requested.contains(&OS_CPU) {
            let cpu_hits = hits.entry(OS_CPU).or_default();
            for snapshot in snapshots.values() {
                let Some((row_ordinal, current)) = snapshot.aggregate else {
                    continue;
                };
                if self
                    .cpu_before
                    .is_some_and(|before| cpu_busy_at_least_80(before, current))
                {
                    cpu_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        field_ordinal: CPU_IDLE_FIELD,
                        row_ordinal,
                        timestamp: current.timestamp,
                    });
                }
                self.cpu_before = Some(current);
            }
        }
        self.find_load_with_cpus(segment, &snapshots, hits)
    }

    fn find_load_with_cpus(
        &self,
        segment: &Segment,
        snapshots: &BTreeMap<i64, CpuSnapshot>,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_LOADAVG) || segment.rows_of(OS_LOADAVG).is_none() {
            return Ok(());
        }
        let load_hits = hits.entry(OS_LOADAVG).or_default();
        segment.visit_rows(
            OS_LOADAVG,
            &["ts", "load1", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::F64(load1)),
                    Some(Cell::U32(0)),
                    Some(row_ordinal),
                ) = (
                    row.get("ts"),
                    row.get("load1"),
                    row.get("scope"),
                    u32::try_from(ordinal).ok(),
                )
                else {
                    return true;
                };
                let online = snapshots
                    .get(timestamp)
                    .map_or(0, |snapshot| snapshot.online);
                if online != 0 && load1.is_finite() && *load1 >= 2.0 * f64::from(online) {
                    load_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        field_ordinal: LOAD1_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                    });
                }
                true
            },
        )?;
        Ok(())
    }

    fn find_memory(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_MEMINFO) || segment.rows_of(OS_MEMINFO).is_none() {
            return Ok(());
        }
        let memory_hits = hits.entry(OS_MEMINFO).or_default();
        segment.visit_rows(
            OS_MEMINFO,
            &["ts", "mem_total", "mem_available", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::I64(total)),
                    Some(Cell::I64(available)),
                    Some(Cell::U32(0)),
                    Some(row_ordinal),
                ) = (
                    row.get("ts"),
                    row.get("mem_total"),
                    row.get("mem_available"),
                    row.get("scope"),
                    u32::try_from(ordinal).ok(),
                )
                else {
                    return true;
                };
                if *total > 0
                    && *available >= 0
                    && available <= total
                    && i128::from(*available) * 100 <= i128::from(*total) * 10
                {
                    memory_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        field_ordinal: MEM_AVAILABLE_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                    });
                }
                true
            },
        )?;
        Ok(())
    }

    fn find_mounts(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_MOUNTINFO) || segment.rows_of(OS_MOUNTINFO).is_none() {
            return Ok(());
        }
        let mount_hits = hits.entry(OS_MOUNTINFO).or_default();
        segment.visit_rows(
            OS_MOUNTINFO,
            &["ts", "total_bytes", "free_bytes", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::I64(total)),
                    Some(Cell::I64(free)),
                    Some(Cell::U32(0)),
                    Some(row_ordinal),
                ) = (
                    row.get("ts"),
                    row.get("total_bytes"),
                    row.get("free_bytes"),
                    row.get("scope"),
                    u32::try_from(ordinal).ok(),
                )
                else {
                    return true;
                };
                if *total > 0
                    && *free >= 0
                    && free <= total
                    && i128::from(*total - *free) * 100 >= i128::from(*total) * 90
                {
                    mount_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        field_ordinal: MOUNT_FREE_BYTES_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                    });
                }
                true
            },
        )?;
        Ok(())
    }

    fn find_slow_queries(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&PG_LOG_SLOW_QUERIES)
            || segment.rows_of(PG_LOG_SLOW_QUERIES).is_none()
        {
            return Ok(());
        }
        let query_hits = hits.entry(PG_LOG_SLOW_QUERIES).or_default();
        segment.visit_rows(
            PG_LOG_SLOW_QUERIES,
            &["ts", "max_duration_ms"],
            0,
            usize::MAX,
            |ordinal, row| {
                if let (Some(Cell::Ts(timestamp)), Some(Cell::F64(duration)), Some(row_ordinal)) = (
                    row.get("ts"),
                    row.get("max_duration_ms"),
                    u32::try_from(ordinal).ok(),
                ) && duration.is_finite()
                    && *duration >= 5_000.0
                {
                    query_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        field_ordinal: SLOW_QUERY_DURATION_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                    });
                }
                true
            },
        )?;
        Ok(())
    }

    fn find_oom(
        &mut self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_VMSTAT) || segment.rows_of(OS_VMSTAT).is_none() {
            return Ok(());
        }
        let oom_hits = hits.entry(OS_VMSTAT).or_default();
        segment.visit_rows(
            OS_VMSTAT,
            &["ts", "oom_kill", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (Some(Cell::Ts(timestamp)), Some(Cell::U32(0))) =
                    (row.get("ts"), row.get("scope"))
                else {
                    return true;
                };
                let current = optional_i64(row.get("oom_kill"));
                if let (Some((before_ts, Some(before))), Some(after), Some(row_ordinal)) =
                    (self.oom_before, current, u32::try_from(ordinal).ok())
                    && *timestamp > before_ts
                    && after > before
                {
                    oom_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        field_ordinal: OOM_KILL_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                    });
                }
                self.oom_before = Some((*timestamp, current));
                true
            },
        )?;
        Ok(())
    }

    fn find_deadlocks(
        &mut self,
        segment: &Segment,
        type_id: u32,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let database_hits = hits.entry(type_id).or_default();
        segment.visit_rows(
            type_id,
            &["ts", "datid", "deadlocks"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::U32(datid)),
                    Some(Cell::I64(deadlocks)),
                ) = (row.get("ts"), row.get("datid"), row.get("deadlocks"))
                else {
                    return true;
                };
                let key = (type_id, *datid);
                if let (Some((before_ts, before)), Some(row_ordinal)) = (
                    self.deadlocks_before.get(&key).copied(),
                    u32::try_from(ordinal).ok(),
                ) && *timestamp > before_ts
                    && *deadlocks > before
                {
                    database_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        field_ordinal: DATABASE_DEADLOCKS_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                    });
                }
                self.deadlocks_before
                    .insert(key, (*timestamp, *deadlocks));
                true
            },
        )?;
        Ok(())
    }

    fn find_active_backends(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        let requested: Vec<u32> = activity_layouts()
            .into_iter()
            .filter(|type_id| self.requested.contains(type_id))
            .collect();
        if requested.is_empty() {
            return Ok(());
        }
        let Some(cpus) = postgres_cpus(segment)? else {
            return Ok(());
        };
        let mut combined = BTreeMap::<i64, Option<ActiveSnapshot>>::new();
        for type_id in requested {
            for (timestamp, row_ordinal, count) in active_snapshots(segment, type_id)? {
                combined
                    .entry(timestamp)
                    .and_modify(|sample| *sample = None)
                    .or_insert(Some(ActiveSnapshot {
                        type_id,
                        row_ordinal,
                        count,
                    }));
            }
        }
        let service_slots = cpus.saturating_mul(2);
        for (timestamp, sample) in combined {
            if let Some(sample) = sample
                && sample.count > service_slots
            {
                hits.entry(sample.type_id).or_default().push(Finding {
                    kind: FindingKind::KnownBad,
                    field_ordinal: activity_state_field(sample.type_id),
                    row_ordinal: sample.row_ordinal,
                    timestamp,
                });
            }
        }
        Ok(())
    }

    fn find_overall_health(&self, index: &Index, hits: &mut BTreeMap<u32, Vec<Finding>>) {
        if !self.requested.contains(&0) {
            return;
        }
        let health_hits = hits.entry(0).or_default();
        for block in &index.blocks {
            if let SeriesBlock::OverallHealth(points) = block {
                for (ordinal, point) in points.iter().enumerate() {
                    if point.value.is_some_and(|value| value < 50)
                        && let Some(row_ordinal) = u32::try_from(ordinal).ok()
                    {
                        health_hits.push(Finding {
                            kind: FindingKind::KnownBad,
                            field_ordinal: OVERALL_HEALTH_FIELD,
                            row_ordinal,
                            timestamp: point.timestamp,
                        });
                    }
                }
            }
        }
    }
}

/// Whether one physical layout has an explicit finding comparison.
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

fn process_rate(before: ProcessRaw, current: ProcessRaw) -> Option<f64> {
    let elapsed = current.timestamp.checked_sub(before.timestamp)?;
    let delta = current.read_bytes?.checked_sub(before.read_bytes?)?;
    if elapsed <= 0 || delta < 0 {
        return None;
    }
    let value =
        integer_as_f64(i128::from(delta))? * 1_000_000.0 / integer_as_f64(i128::from(elapsed))?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn statement_average(before: StatementRaw, current: StatementRaw) -> Option<f64> {
    if current.timestamp <= before.timestamp
        || !before.total_exec_time.is_finite()
        || !current.total_exec_time.is_finite()
    {
        return None;
    }
    let calls = current.calls.checked_sub(before.calls)?;
    let total = current.total_exec_time - before.total_exec_time;
    if calls <= 0 || !total.is_finite() || total < 0.0 {
        return None;
    }
    let value = total / integer_as_f64(i128::from(calls))?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn baseline_is_spike(history: &[PriorValue], timestamp: i64, current: f64) -> bool {
    let Some(selected) = select_baseline(history, timestamp) else {
        return false;
    };
    let values: Vec<f64> = selected.iter().map(|point| point.value).collect();
    is_upward_spike(current, &values)
}

fn push_prior(history: &mut Vec<PriorValue>, cutoff: i64, timestamp: i64, value: f64) {
    if timestamp < cutoff
        && history.len() == 5
        && history.iter().all(|point| point.timestamp < cutoff)
    {
        history.remove(0);
    }
    history.push(PriorValue { timestamp, value });
}

fn cpu_raw(row: &kronika_reader::Row) -> Option<CpuRaw> {
    let Some(Cell::Ts(timestamp)) = row.get("ts") else {
        return None;
    };
    let mut counters = [0_i64; 8];
    for (at, name) in [
        "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal",
    ]
    .into_iter()
    .enumerate()
    {
        let Some(Cell::I64(value)) = row.get(name) else {
            return None;
        };
        counters[at] = *value;
    }
    Some(CpuRaw {
        timestamp: *timestamp,
        counters,
    })
}

fn cpu_busy_at_least_80(before: CpuRaw, current: CpuRaw) -> bool {
    if current.timestamp <= before.timestamp {
        return false;
    }
    let mut deltas = [0_i128; 8];
    for (at, (after, before)) in current
        .counters
        .into_iter()
        .zip(before.counters)
        .enumerate()
    {
        let delta = i128::from(after) - i128::from(before);
        if delta < 0 {
            return false;
        }
        deltas[at] = delta;
    }
    let busy = deltas[0] + deltas[1] + deltas[2] + deltas[5] + deltas[6] + deltas[7];
    let total: i128 = deltas.into_iter().sum();
    total > 0 && busy * 100 >= total * 80
}

const fn cpu_columns() -> &'static [&'static str] {
    &[
        "ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal",
        "scope",
    ]
}

const fn optional_i64(cell: Option<&Cell>) -> Option<i64> {
    match cell {
        Some(Cell::I64(value)) => Some(*value),
        _ => None,
    }
}

fn statement_identity(type_id: u32, row: &kronika_reader::Row) -> Option<StatementId> {
    let queryid = match row.get("queryid")? {
        Cell::I64(value) => Some(*value),
        Cell::Null => None,
        _ => return None,
    };
    let (Some(Cell::U32(userid)), Some(Cell::U32(dbid))) = (row.get("userid"), row.get("dbid"))
    else {
        return None;
    };
    let toplevel = if type_id == 1_002_002 {
        None
    } else {
        match row.get("toplevel") {
            Some(Cell::Bool(value)) => Some(*value),
            _ => return None,
        }
    };
    Some(StatementId {
        queryid,
        userid: *userid,
        dbid: *dbid,
        toplevel,
    })
}

const fn statement_columns(type_id: u32) -> &'static [&'static str] {
    if type_id == 1_002_002 {
        &["queryid", "userid", "dbid"]
    } else {
        &["queryid", "userid", "dbid", "toplevel"]
    }
}

const fn statement_value_columns(type_id: u32) -> &'static [&'static str] {
    if type_id == 1_002_002 {
        &[
            "ts",
            "queryid",
            "userid",
            "dbid",
            "calls",
            "total_exec_time",
        ]
    } else {
        &[
            "ts",
            "queryid",
            "userid",
            "dbid",
            "toplevel",
            "calls",
            "total_exec_time",
        ]
    }
}

const fn statement_total_time_field(type_id: u32) -> u16 {
    if type_id == 1_002_002 { 10 } else { 11 }
}

const fn activity_state_field(type_id: u32) -> u16 {
    if type_id == 1_001_001 { 7 } else { 8 }
}

fn postgres_cpus(segment: &Segment) -> Result<Option<u32>, BuildError> {
    if segment.rows_of(INSTANCE_METADATA_TYPE_ID).is_none() {
        return Ok(None);
    }
    let mut value = None;
    let mut rows = 0_u32;
    segment.visit_rows(
        INSTANCE_METADATA_TYPE_ID,
        &["postgresql_enabled", "postgresql_effective_cpus"],
        0,
        usize::MAX,
        |_ordinal, row| {
            rows = rows.saturating_add(1);
            value = match (
                row.get("postgresql_enabled"),
                row.get("postgresql_effective_cpus"),
            ) {
                (Some(Cell::Bool(true)), Some(Cell::U32(cpus))) if *cpus > 0 => Some(*cpus),
                _ => None,
            };
            true
        },
    )?;
    Ok((rows == 1).then_some(value).flatten())
}

fn active_snapshots(segment: &Segment, type_id: u32) -> Result<Vec<(i64, u32, u32)>, BuildError> {
    if segment.rows_of(type_id).is_none() {
        return Ok(Vec::new());
    }
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
    let mut snapshots = BTreeMap::<i64, (Option<u32>, u32)>::new();
    segment.visit_rows(type_id, &["ts", "state"], 0, usize::MAX, |ordinal, row| {
        let Some(Cell::Ts(timestamp)) = row.get("ts") else {
            return true;
        };
        let sample = snapshots.entry(*timestamp).or_default();
        if row
            .get("state")
            .is_some_and(|cell| matches!(cell, Cell::StrId(id) if active_ids.contains(id)))
        {
            sample.0 = sample.0.or_else(|| u32::try_from(ordinal).ok());
            sample.1 = sample.1.saturating_add(1);
        }
        true
    })?;
    Ok(snapshots
        .into_iter()
        .filter_map(|(timestamp, (ordinal, count))| {
            ordinal.map(|ordinal| (timestamp, ordinal, count))
        })
        .collect())
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
