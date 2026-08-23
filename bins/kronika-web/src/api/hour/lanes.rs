use std::collections::{BTreeMap, BTreeSet, HashSet};

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment};
use kronika_registry::{contract, logical_section_name};

use crate::api::ApiError;
use crate::route::Window;

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
    swap: BTreeMap<i64, Option<i64>>,
    oom: BTreeMap<i64, Option<i64>>,
    running: BTreeMap<i64, f64>,
    waiting: BTreeMap<i64, f64>,
    lock_waiting: BTreeMap<i64, f64>,
    oldest_xact: BTreeMap<i64, f64>,
}

impl Counters {
    fn retain_latest(&mut self) {
        retain_latest(&mut self.busy_ticks);
        retain_latest(&mut self.stall_cpu);
        retain_latest(&mut self.stall_io);
        retain_latest(&mut self.memory);
        retain_latest(&mut self.disk_busy);
        retain_latest(&mut self.disk_queue);
        retain_latest(&mut self.net_rx);
        retain_latest(&mut self.net_tx);
        retain_latest(&mut self.net_drop);
        retain_latest(&mut self.net_errors);
        retain_latest(&mut self.swap);
        retain_latest(&mut self.oom);
        retain_latest(&mut self.running);
        retain_latest(&mut self.waiting);
        retain_latest(&mut self.lock_waiting);
        retain_latest(&mut self.oldest_xact);
    }
}

fn retain_latest<T>(samples: &mut BTreeMap<i64, T>) {
    if samples.len() <= 1 {
        return;
    }
    let latest = samples.pop_last();
    samples.clear();
    if let Some((ts, value)) = latest {
        samples.insert(ts, value);
    }
}

/// Carries counter state without re-emitting the shared boundary row.
pub(super) struct State {
    counters: Counters,
    emitted_before: i64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            counters: Counters::default(),
            emitted_before: i64::MIN,
        }
    }
}

pub(super) fn collect(
    segment: &Segment,
    window: Window,
    state: &mut State,
) -> Result<Vec<LanePoint>, ApiError> {
    let mut facts = Facts::default();
    for (type_id, _rows) in segment.sections() {
        let Some(name) = logical_section_name(type_id) else {
            continue;
        };
        match name {
            "instance_metadata" => read_metadata(segment, type_id, &mut facts)?,
            "os_cpu" => read_cpu(segment, type_id, &mut state.counters, &mut facts)?,
            "os_psi" => read_psi(segment, type_id, &mut state.counters)?,
            "os_meminfo" => read_memory(segment, type_id, &mut state.counters)?,
            "os_diskstats" => read_disk(segment, type_id, &mut state.counters)?,
            "os_netdev" => read_network(segment, type_id, &mut state.counters)?,
            "os_vmstat" => read_vmstat(segment, type_id, &mut state.counters)?,
            "pg_stat_activity" => read_activity(segment, type_id, &mut state.counters)?,
            _other => {}
        }
    }
    let current = current_points(
        &state.counters,
        facts.ticks_per_second,
        i64::try_from(facts.cores.len()).unwrap_or(0),
        segment.min_ts().max(state.emitted_before.saturating_add(1)),
        segment.max_ts(),
        window,
    );
    state.counters.retain_latest();
    state.emitted_before = state.emitted_before.max(segment.max_ts());
    Ok(current)
}

#[derive(Default)]
struct Facts {
    ticks_per_second: i64,
    cores: BTreeSet<i64>,
}

fn read_metadata(segment: &Segment, type_id: u32, facts: &mut Facts) -> Result<(), ApiError> {
    let names = with_columns(type_id, &["clock_ticks_per_sec"], &[]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        if let Some(ticks) = number(&row, "clock_ticks_per_sec") {
            #[expect(clippy::cast_possible_truncation, reason = "a hundred, in practice")]
            {
                facts.ticks_per_second = ticks as i64;
            }
        }
        true
    })?;
    Ok(())
}

fn read_cpu(
    segment: &Segment,
    type_id: u32,
    counters: &mut Counters,
    facts: &mut Facts,
) -> Result<(), ApiError> {
    const FIELDS: [&str; 8] = [
        "ts", "cpu_id", "user", "nice", "system", "irq", "softirq", "steal",
    ];
    let names = with_columns(type_id, &FIELDS, &["scope"]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        if let Some(id) = number(&row, "cpu_id")
            && id >= 0.0
        {
            #[expect(clippy::cast_possible_truncation, reason = "core indexes are small")]
            facts.cores.insert(id as i64);
        }
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        if !matches!(row.get("cpu_id"), Some(Cell::I32(-1)))
            || !matches!(row.get("scope"), None | Some(Cell::U32(0)))
        {
            return true;
        }
        if let Some(busy) = cpu_busy_ticks(&row) {
            counters.busy_ticks.insert(ts, busy);
        }
        true
    })?;
    Ok(())
}

fn cpu_busy_ticks(row: &Row) -> Option<i64> {
    ["user", "nice", "system", "irq", "softirq", "steal"]
        .iter()
        .try_fold(0_i64, |total, name| {
            let Cell::I64(value) = row.get(name)? else {
                return None;
            };
            total.checked_add(*value)
        })
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
        counters
            .swap
            .insert(ts, counter_sum(&row, &["pswpin", "pswpout"]));
        counters.oom.insert(ts, counter_sum(&row, &["oom_kill"]));
        true
    })?;
    Ok(())
}

fn counter_sum(row: &Row, columns: &[&str]) -> Option<i64> {
    columns.iter().try_fold(0_i64, |total, column| {
        let Cell::I64(value) = row.get(column)? else {
            return None;
        };
        total.checked_add(*value)
    })
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

#[derive(Debug, PartialEq, Eq)]
struct ActivitySample {
    ts: Option<i64>,
    backend_type: Option<u64>,
    state: Option<u64>,
    wait_event_type: Option<u64>,
    leader: bool,
    xact_start: Option<i64>,
}

fn activity_sample(row: &Row) -> ActivitySample {
    ActivitySample {
        ts: timestamp(row, "ts"),
        backend_type: string_id(row, "backend_type"),
        state: string_id(row, "state"),
        wait_event_type: string_id(row, "wait_event_type"),
        leader: row.get("leader_pid").is_some_and(present),
        xact_start: timestamp(row, "xact_start"),
    }
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
    let mut samples = Vec::new();
    let mut ids = HashSet::new();
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let sample = activity_sample(&row);
        ids.extend(
            [sample.backend_type, sample.state, sample.wait_event_type]
                .into_iter()
                .flatten(),
        );
        samples.push(sample);
        true
    })?;
    let dictionary = segment.dictionary_for(&ids)?;
    for sample in samples {
        record_activity_sample(
            counters,
            &sample,
            text(sample.backend_type, &dictionary),
            text(sample.state, &dictionary),
            text(sample.wait_event_type, &dictionary),
        );
    }
    Ok(())
}

fn record_activity_sample(
    counters: &mut Counters,
    sample: &ActivitySample,
    kind: Option<&[u8]>,
    state: Option<&[u8]>,
    wait_event_type: Option<&[u8]>,
) {
    let Some(ts) = sample.ts else {
        return;
    };
    counters.running.entry(ts).or_insert(0.0);
    counters.waiting.entry(ts).or_insert(0.0);
    counters.lock_waiting.entry(ts).or_insert(0.0);
    // Lock-wait expiry counts all backends, unlike the client-only public lane.
    if wait_event_type == Some(b"Lock".as_slice()) {
        *counters.lock_waiting.entry(ts).or_insert(0.0) += 1.0;
    }
    // Count client backends, not their parallel workers, in public activity.
    if kind != Some(b"client backend".as_slice()) || sample.leader {
        return;
    }
    if let Some(started) = sample.xact_start {
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
    if state != Some(b"active".as_slice()) {
        return;
    }
    let lane = if wait_event_type.is_some() {
        &mut counters.waiting
    } else {
        &mut counters.running
    };
    *lane.entry(ts).or_insert(0.0) += 1.0;
}

fn current_points(
    counters: &Counters,
    ticks_per_second: i64,
    cpu_count: i64,
    from: i64,
    to: i64,
    window: Window,
) -> Vec<LanePoint> {
    points(counters, ticks_per_second, cpu_count)
        .into_iter()
        .filter(|point| point.ts >= from && point.ts <= to && window.contains(point.ts))
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
    ] {
        for (ts, value) in rate(stored, |value, seconds| value / scale / seconds) {
            out.push(LanePoint { key, ts, value });
        }
    }
    for (key, stored) in [("mem_swap", &counters.swap), ("mem_oom", &counters.oom)] {
        for (ts, value) in nullable_rate(stored, |value, seconds| value / seconds) {
            out.push(LanePoint { key, ts, value });
        }
    }
    for (key, stored) in [
        ("memory", &counters.memory),
        ("pg_running", &counters.running),
        ("pg_waiting", &counters.waiting),
        ("pg_lock_waiting", &counters.lock_waiting),
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
    rate_samples(
        stored.iter().map(|(ts, value)| (*ts, Some(*value))),
        stored.len(),
        scale,
    )
}

/// Returns per-second deltas and starts again after an explicit null sample.
fn nullable_rate(
    stored: &BTreeMap<i64, Option<i64>>,
    scale: impl Fn(f64, f64) -> f64,
) -> Vec<(i64, Option<f64>)> {
    rate_samples(
        stored.iter().map(|(ts, value)| (*ts, *value)),
        stored.len(),
        scale,
    )
}

fn rate_samples(
    samples: impl Iterator<Item = (i64, Option<i64>)>,
    len: usize,
    scale: impl Fn(f64, f64) -> f64,
) -> Vec<(i64, Option<f64>)> {
    let mut out = Vec::with_capacity(len);
    let mut earlier: Option<(i64, i64)> = None;
    for (ts, value) in samples {
        let Some(value) = value else {
            out.push((ts, None));
            earlier = None;
            continue;
        };
        let point = if let Some((before_ts, before)) = earlier {
            #[expect(clippy::cast_precision_loss, reason = "an hour is far below 2^53")]
            let seconds = (ts - before_ts) as f64 / 1_000_000.0;
            #[expect(clippy::cast_precision_loss, reason = "counters stay below 2^53")]
            let delta = (value - before) as f64;
            (seconds > 0.0 && delta >= 0.0).then(|| scale(delta, seconds))
        } else {
            None
        };
        out.push((ts, point));
        earlier = Some((ts, value));
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

fn string_id(row: &Row, column: &str) -> Option<u64> {
    match row.get(column) {
        Some(Cell::StrId(id)) => Some(*id),
        _other => None,
    }
}

fn text(id: Option<u64>, dictionary: &Dictionary) -> Option<&[u8]> {
    match dictionary.resolve(id?) {
        Some(Resolved::Str(bytes)) => Some(bytes),
        _other => None,
    }
}
