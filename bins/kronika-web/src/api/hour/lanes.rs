//! Computes normalized timeline lanes from stored samples.

use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment};
use kronika_registry::{contract, logical_section_name};

use crate::api::ApiError;

#[cfg(test)]
mod tests;

pub(super) struct LanePoint {
    pub(super) key: &'static str,
    pub(super) ts: i64,
    pub(super) value: Option<f64>,
}

#[derive(Default)]
struct Counters {
    busy_ticks: BTreeMap<i64, i64>,
    stall_cpu: BTreeMap<i64, i64>,
    stall_io: BTreeMap<i64, i64>,
    memory: BTreeMap<i64, f64>,
    disk_busy: BTreeMap<i64, i64>,
    disk_queue: BTreeMap<i64, i64>,
    net_rx: BTreeMap<i64, i64>,
    net_tx: BTreeMap<i64, i64>,
    net_drop: BTreeMap<i64, i64>,
    net_errors: BTreeMap<i64, i64>,
    swap: BTreeMap<i64, i64>,
    oom: BTreeMap<i64, i64>,
    running: BTreeMap<i64, f64>,
    waiting: BTreeMap<i64, f64>,
    oldest_xact: BTreeMap<i64, f64>,
}

/// Counter state is carried across segment boundaries.
#[derive(Default)]
pub(super) struct State {
    counters: Counters,
}

pub(super) fn collect(
    segment: &Segment,
    ticks_per_second: i64,
    cpu_count: i64,
    state: &mut State,
) -> Result<Vec<LanePoint>, ApiError> {
    for (type_id, _rows) in segment.sections() {
        let Some(name) = logical_section_name(type_id) else {
            continue;
        };
        match name {
            "os_cpu" => read_cpu(segment, type_id, &mut state.counters)?,
            "os_psi" => read_psi(segment, type_id, &mut state.counters)?,
            "os_meminfo" => read_memory(segment, type_id, &mut state.counters)?,
            "os_diskstats" => read_disk(segment, type_id, &mut state.counters)?,
            "os_netdev" => read_network(segment, type_id, &mut state.counters)?,
            "os_vmstat" => read_vmstat(segment, type_id, &mut state.counters)?,
            "pg_stat_activity" => read_activity(segment, type_id, &mut state.counters)?,
            _other => {}
        }
    }
    Ok(current_points(
        &state.counters,
        ticks_per_second,
        cpu_count,
        segment.min_ts(),
        segment.max_ts(),
    ))
}

fn read_cpu(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), ApiError> {
    const FIELDS: [&str; 6] = ["ts", "cpu_id", "user", "nice", "system", "irq"];
    let projection = with_columns(type_id, &FIELDS, &["softirq", "guest", "guest_nice"]);
    let names: Vec<&'static str> = projection.clone();
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let (Some(ts), Some(cpu_id)) = (timestamp(&row, "ts"), number(&row, "cpu_id")) else {
            return true;
        };
        // Ignore the aggregate row; the per-CPU rows are summed below.
        if cpu_id < 0.0 {
            return true;
        }
        let busy = ["user", "nice", "system", "irq", "softirq"]
            .iter()
            .filter_map(|name| number(&row, name))
            .sum::<f64>()
            - number(&row, "guest").unwrap_or(0.0)
            - number(&row, "guest_nice").unwrap_or(0.0);
        #[expect(clippy::cast_possible_truncation, reason = "tick counters stay small")]
        let busy = busy.max(0.0) as i64;
        counters
            .busy_ticks
            .entry(ts)
            .and_modify(|total| *total += busy)
            .or_insert(busy);
        true
    })?;
    Ok(())
}

fn read_psi(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), ApiError> {
    let names = with_columns(type_id, &["ts", "resource", "some_total"], &[]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let (Some(ts), Some(resource), Some(total)) = (
            timestamp(&row, "ts"),
            number(&row, "resource"),
            number(&row, "some_total"),
        ) else {
            return true;
        };
        #[expect(clippy::cast_possible_truncation, reason = "microseconds of an hour")]
        let total = total as i64;
        // 0 is cpu, 1 memory, 2 io.
        if resource < 0.5 {
            counters.stall_cpu.insert(ts, total);
        } else if resource > 1.5 {
            counters.stall_io.insert(ts, total);
        }
        true
    })?;
    Ok(())
}

fn read_memory(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), ApiError> {
    let names = with_columns(type_id, &["ts", "mem_total", "mem_available"], &[]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let (Some(ts), Some(total), Some(available)) = (
            timestamp(&row, "ts"),
            number(&row, "mem_total"),
            number(&row, "mem_available"),
        ) else {
            return true;
        };
        if total > 0.0 {
            counters
                .memory
                .insert(ts, (total - available) / total * 100.0);
        }
        true
    })?;
    Ok(())
}

fn read_disk(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), ApiError> {
    let names = with_columns(
        type_id,
        &["ts", "io_time_ms", "io_weighted_time_ms"],
        &["scope"],
    );
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let (Some(ts), Some(busy), Some(weighted)) = (
            timestamp(&row, "ts"),
            number(&row, "io_time_ms"),
            number(&row, "io_weighted_time_ms"),
        ) else {
            return true;
        };
        add(&mut counters.disk_busy, ts, busy);
        add(&mut counters.disk_queue, ts, weighted);
        true
    })?;
    Ok(())
}

fn read_network(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), ApiError> {
    const FIELDS: [&str; 9] = [
        "ts", "rx_bytes", "tx_bytes", "rx_drop", "tx_drop", "rx_errs", "tx_errs", "rx_fifo",
        "tx_fifo",
    ];
    let names = with_columns(type_id, &FIELDS, &[]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        for (store, columns) in [
            (&mut counters.net_rx, ["rx_bytes"].as_slice()),
            (&mut counters.net_tx, ["tx_bytes"].as_slice()),
            (
                &mut counters.net_drop,
                ["rx_drop", "tx_drop", "rx_fifo", "tx_fifo"].as_slice(),
            ),
            (&mut counters.net_errors, ["rx_errs", "tx_errs"].as_slice()),
        ] {
            let total: f64 = columns.iter().filter_map(|name| number(&row, name)).sum();
            add(store, ts, total);
        }
        true
    })?;
    Ok(())
}

fn read_vmstat(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), ApiError> {
    let names = with_columns(type_id, &["ts"], &["pswpin", "pswpout", "oom_kill"]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        let swapped: f64 = ["pswpin", "pswpout"]
            .iter()
            .filter_map(|name| number(&row, name))
            .sum();
        add(&mut counters.swap, ts, swapped);
        if let Some(killed) = number(&row, "oom_kill") {
            add(&mut counters.oom, ts, killed);
        }
        true
    })?;
    Ok(())
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "kernel counters stay below 2^63"
)]
fn add(store: &mut BTreeMap<i64, i64>, ts: i64, value: f64) {
    let value = value as i64;
    store
        .entry(ts)
        .and_modify(|total| *total += value)
        .or_insert(value);
}

fn read_activity(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), ApiError> {
    let names = with_columns(
        type_id,
        &[
            "ts",
            "state",
            "wait_event_type",
            "backend_type",
            "xact_start",
        ],
        &["leader_pid"],
    );
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        ids.extend(["backend_type", "state"].iter().filter_map(|column| {
            if let Some(Cell::StrId(id)) = row.get(column) {
                Some(*id)
            } else {
                None
            }
        }));
        rows.push(row);
        true
    })?;
    let dictionary = segment.dictionary_for(&ids)?;
    for row in rows {
        let Some(ts) = timestamp(&row, "ts") else {
            continue;
        };
        counters.running.entry(ts).or_insert(0.0);
        counters.waiting.entry(ts).or_insert(0.0);
        let kind = text(&row, "backend_type", &dictionary);
        // Count client backends, not their parallel workers.
        if kind != Some(b"client backend".as_slice()) || row.get("leader_pid").is_some_and(present)
        {
            continue;
        }
        if let Some(started) = timestamp(&row, "xact_start") {
            #[expect(clippy::cast_precision_loss, reason = "an hour is far below 2^53")]
            let age = (ts - started) as f64 / 1_000_000.0;
            counters
                .oldest_xact
                .entry(ts)
                .and_modify(|current| {
                    if age > *current {
                        *current = age;
                    }
                })
                .or_insert_with(|| age.max(0.0));
        }
        let state = text(&row, "state", &dictionary);
        if state != Some(b"active".as_slice()) {
            continue;
        }
        let stuck = row.get("wait_event_type").is_some_and(present);
        let lane = if stuck {
            &mut counters.waiting
        } else {
            &mut counters.running
        };
        *lane.entry(ts).or_insert(0.0) += 1.0;
    }
    Ok(())
}

fn current_points(
    counters: &Counters,
    ticks_per_second: i64,
    cpu_count: i64,
    from: i64,
    to: i64,
) -> Vec<LanePoint> {
    points(counters, ticks_per_second, cpu_count)
        .into_iter()
        .filter(|point| point.ts >= from && point.ts <= to)
        .collect()
}

fn points(counters: &Counters, ticks_per_second: i64, cpu_count: i64) -> Vec<LanePoint> {
    let mut out = Vec::new();
    if ticks_per_second > 0 && cpu_count > 0 {
        #[expect(clippy::cast_precision_loss, reason = "core counts are small")]
        let capacity = (ticks_per_second * cpu_count) as f64;
        let busy = rate(&counters.busy_ticks, |value, seconds| {
            value / seconds / capacity * 100.0
        });
        for (ts, value) in busy {
            out.push(LanePoint {
                key: "cpu_busy",
                ts,
                value,
            });
        }
    }
    for (key, stalls) in [
        ("cpu_stall", &counters.stall_cpu),
        ("io_stall", &counters.stall_io),
    ] {
        let stalled = rate(stalls, |value, seconds| {
            value / 1_000_000.0 / seconds * 100.0
        });
        for (ts, value) in stalled {
            out.push(LanePoint { key, ts, value });
        }
    }
    // io_time_ms per elapsed second is device busy percent.
    for (ts, value) in rate(&counters.disk_busy, |value, seconds| {
        (value / 1000.0 / seconds * 100.0).min(100.0)
    }) {
        out.push(LanePoint {
            key: "disk_busy",
            ts,
            value,
        });
    }
    for (key, stored, scale) in [
        ("disk_queue", &counters.disk_queue, 1000.0),
        ("net_rx", &counters.net_rx, 1.0),
        ("net_tx", &counters.net_tx, 1.0),
        ("net_drop", &counters.net_drop, 1.0),
        ("net_errors", &counters.net_errors, 1.0),
        ("mem_swap", &counters.swap, 1.0),
        ("mem_oom", &counters.oom, 1.0),
    ] {
        for (ts, value) in rate(stored, |value, seconds| value / scale / seconds) {
            out.push(LanePoint { key, ts, value });
        }
    }
    for (key, stored) in [
        ("memory", &counters.memory),
        ("pg_running", &counters.running),
        ("pg_waiting", &counters.waiting),
        ("pg_oldest_xact", &counters.oldest_xact),
    ] {
        for (ts, value) in stored {
            out.push(LanePoint {
                key,
                ts: *ts,
                value: Some(*value),
            });
        }
    }
    out.sort_by_key(|point| (point.ts, point.key));
    out
}

/// Returns per-second deltas; the first or unusable sample is null.
fn rate(stored: &BTreeMap<i64, i64>, scale: impl Fn(f64, f64) -> f64) -> Vec<(i64, Option<f64>)> {
    let mut out = Vec::with_capacity(stored.len());
    let mut earlier: Option<(i64, i64)> = None;
    for (ts, value) in stored {
        let point = if let Some((before_ts, before)) = earlier {
            #[expect(clippy::cast_precision_loss, reason = "an hour is far below 2^53")]
            let seconds = (ts - before_ts) as f64 / 1_000_000.0;
            #[expect(clippy::cast_precision_loss, reason = "counters stay below 2^53")]
            let delta = (value - before) as f64;
            (seconds > 0.0 && delta >= 0.0).then(|| scale(delta, seconds))
        } else {
            None
        };
        out.push((*ts, point));
        earlier = Some((*ts, *value));
    }
    out
}

fn with_columns(
    type_id: u32,
    required: &[&'static str],
    optional: &[&'static str],
) -> Vec<&'static str> {
    let Some(contract) = contract(type_id) else {
        return Vec::new();
    };
    let mut names: Vec<&'static str> = required
        .iter()
        .filter_map(|name| contract.column(name).map(|column| column.name))
        .collect();
    names.extend(
        optional
            .iter()
            .filter_map(|name| contract.column(name).map(|column| column.name)),
    );
    names
}

fn timestamp(row: &Row, column: &str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::Ts(stored)) => Some(*stored),
        _other => None,
    }
}

#[expect(clippy::cast_precision_loss, reason = "counters stay below 2^53")]
fn number(row: &Row, column: &str) -> Option<f64> {
    match row.get(column) {
        Some(Cell::I16(value)) => Some(f64::from(*value)),
        Some(Cell::I32(value)) => Some(f64::from(*value)),
        Some(Cell::I64(value) | Cell::Ts(value)) => Some(*value as f64),
        Some(Cell::U32(value)) => Some(f64::from(*value)),
        Some(Cell::U64(value)) => Some(*value as f64),
        Some(Cell::F64(value)) => Some(*value),
        _other => None,
    }
}

const fn present(cell: &Cell) -> bool {
    !matches!(cell, Cell::Null)
}

fn text<'a>(row: &Row, column: &str, dictionary: &'a Dictionary) -> Option<&'a [u8]> {
    match row.get(column) {
        Some(Cell::StrId(id)) => match dictionary.resolve(*id) {
            Some(Resolved::Str(bytes)) => Some(bytes),
            _other => None,
        },
        _other => None,
    }
}

pub(super) struct Facts {
    pub(super) ticks_per_second: i64,
    pub(super) cpu_count: i64,
}

pub(super) fn facts(segment: &Segment) -> Result<Facts, ApiError> {
    let mut ticks_per_second = 0_i64;
    let mut cores = std::collections::BTreeSet::new();
    for (type_id, _rows) in segment.sections() {
        match logical_section_name(type_id) {
            Some("instance_metadata") => {
                let names = with_columns(type_id, &["clock_ticks_per_sec"], &[]);
                segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
                    if let Some(ticks) = number(&row, "clock_ticks_per_sec") {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "a hundred, in practice"
                        )]
                        {
                            ticks_per_second = ticks as i64;
                        }
                    }
                    true
                })?;
            }
            Some("os_cpu") => {
                let names = with_columns(type_id, &["cpu_id"], &[]);
                segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
                    if let Some(id) = number(&row, "cpu_id")
                        && id >= 0.0
                    {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "core indexes are small"
                        )]
                        cores.insert(id as i64);
                    }
                    true
                })?;
            }
            _other => {}
        }
    }
    Ok(Facts {
        ticks_per_second,
        cpu_count: i64::try_from(cores.len()).unwrap_or(0),
    })
}
