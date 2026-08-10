//! Explicit known-bad comparisons.

use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, Resolved, Segment};

use crate::Index;
use crate::build::{BuildError, INSTANCE_METADATA_TYPE_ID};
use crate::findings::{Finding, FindingKind};
use crate::series::SeriesBlock;

use super::{
    CPU_IDLE_FIELD, DATABASE_DEADLOCKS_FIELD, FindingBuilder, LOAD1_FIELD, MEM_AVAILABLE_FIELD,
    MOUNT_FREE_BYTES_FIELD, OOM_KILL_FIELD, OS_CPU, OS_LOADAVG, OS_MEMINFO, OS_MOUNTINFO,
    OS_VMSTAT, OVERALL_HEALTH_FIELD, PG_LOG_SLOW_QUERIES, SLOW_QUERY_DURATION_FIELD,
    activity_layouts, optional_i64,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct CpuRaw {
    pub(super) timestamp: i64,
    pub(super) counters: [i64; 8],
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

impl FindingBuilder {
    pub(super) fn observe_prior_cpu(&mut self, segment: &Segment) -> Result<(), BuildError> {
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

    pub(super) fn observe_prior_oom(&mut self, segment: &Segment) -> Result<(), BuildError> {
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

    pub(super) fn observe_prior_deadlocks(
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

    pub(super) fn find_cpu_and_online_count(
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
                        category: None,
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
                        category: None,
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

    pub(super) fn find_memory(
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
                        category: None,
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

    pub(super) fn find_mounts(
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
                        category: None,
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

    pub(super) fn find_slow_queries(
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
                        category: None,
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

    pub(super) fn find_oom(
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
                        category: None,
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

    pub(super) fn find_deadlocks(
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
                        category: None,
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

    pub(super) fn find_active_backends(
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
                    category: None,
                    field_ordinal: activity_state_field(sample.type_id),
                    row_ordinal: sample.row_ordinal,
                    timestamp,
                });
            }
        }
        Ok(())
    }

    pub(super) fn find_overall_health(
        &self,
        index: &Index,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) {
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
                            category: None,
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

pub(super) fn cpu_busy_at_least_80(before: CpuRaw, current: CpuRaw) -> bool {
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
