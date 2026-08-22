use std::collections::BTreeMap;

use kronika_reader::{Cell, Row};
use kronika_registry::contract;

use super::{
    ActivitySample, Counters, activity_sample, counter_sum, cpu_busy_ticks, current_points, points,
    rate, record_activity_sample,
};
use crate::route::Window;

#[test]
fn counter_points_keep_unusable_subtractions_as_null_and_zero_as_data() {
    let stored = BTreeMap::from([(1, 10), (2, 10), (3, 5), (4, 12)]);
    assert_eq!(
        rate(&stored, |value, _seconds| value),
        vec![(1, None), (2, Some(0.0)), (3, None), (4, Some(7.0))]
    );
}

#[test]
fn a_segment_boundary_keeps_the_preceding_counter_reading() {
    let counters = Counters {
        busy_ticks: BTreeMap::from([(100, 10), (200, 20)]),
        ..Counters::default()
    };
    let current = current_points(
        &counters,
        100,
        1,
        200,
        200,
        Window {
            from: Some(200),
            to: Some(200),
        },
    );
    let busy = current
        .iter()
        .find(|point| point.key == "cpu_busy")
        .expect("current busy point");
    assert_eq!(busy.ts, 200);
    assert_eq!(busy.value, Some(100_000.0));
}

#[test]
fn only_the_latest_sample_is_carried_into_the_next_segment() {
    let mut counters = Counters {
        busy_ticks: BTreeMap::from([(1_000_000, 10), (2_000_000, 20)]),
        memory: BTreeMap::from([(1_000_000, 50.0), (2_000_000, 60.0)]),
        swap: BTreeMap::from([(1_000_000, None), (2_000_000, Some(4))]),
        ..Counters::default()
    };

    counters.retain_latest();

    assert_eq!(counters.busy_ticks, BTreeMap::from([(2_000_000, 20)]));
    assert_eq!(counters.memory, BTreeMap::from([(2_000_000, 60.0)]));
    assert_eq!(counters.swap, BTreeMap::from([(2_000_000, Some(4))]));
    counters.busy_ticks.insert(3_000_000, 30);
    let next = current_points(
        &counters,
        100,
        1,
        3_000_000,
        3_000_000,
        Window {
            from: Some(3_000_000),
            to: Some(3_000_000),
        },
    );
    assert_eq!(
        next.iter()
            .find(|point| point.key == "cpu_busy")
            .and_then(|point| point.value),
        Some(10.0)
    );
}

#[test]
fn public_lane_points_stay_inside_the_inclusive_window() {
    let counters = Counters {
        memory: BTreeMap::from([(99, 1.0), (100, 2.0), (200, 3.0), (201, 4.0)]),
        ..Counters::default()
    };
    let current = current_points(
        &counters,
        0,
        0,
        99,
        201,
        Window {
            from: Some(100),
            to: Some(200),
        },
    );

    assert_eq!(
        current
            .iter()
            .map(|point| (point.ts, point.value))
            .collect::<Vec<_>>(),
        [(100, Some(2.0)), (200, Some(3.0))]
    );
}

#[test]
fn cpu_busy_matches_the_aggregate_direct_comparator() {
    let row = row(
        1_102_001,
        &[
            ("user", Cell::I64(100)),
            ("nice", Cell::I64(10)),
            ("system", Cell::I64(20)),
            ("irq", Cell::I64(5)),
            ("softirq", Cell::I64(5)),
            ("steal", Cell::I64(7)),
        ],
    );
    assert_eq!(cpu_busy_ticks(&row), Some(147));
}

#[test]
fn swap_requires_both_counters() {
    let complete = row(
        1_106_001,
        &[("pswpin", Cell::I64(10)), ("pswpout", Cell::I64(20))],
    );
    let incomplete = row(
        1_106_001,
        &[("pswpin", Cell::I64(10)), ("pswpout", Cell::Null)],
    );
    assert_eq!(counter_sum(&complete, &["pswpin", "pswpout"]), Some(30));
    assert_eq!(counter_sum(&incomplete, &["pswpin", "pswpout"]), None);
}

#[test]
fn nullable_swap_and_oom_do_not_bridge_null_samples() {
    let samples = BTreeMap::from([
        (1_000_000, Some(10)),
        (2_000_000, None),
        (3_000_000, Some(30)),
        (4_000_000, Some(40)),
    ]);
    let counters = Counters {
        swap: samples.clone(),
        oom: samples,
        ..Counters::default()
    };

    for key in ["mem_swap", "mem_oom"] {
        let values = points(&counters, 0, 0)
            .into_iter()
            .filter(|point| point.key == key)
            .map(|point| point.value)
            .collect::<Vec<_>>();
        assert_eq!(values, [None, None, None, Some(10.0)], "{key}");
    }
}

#[test]
fn activity_rows_are_reduced_to_the_lane_fields() {
    let row = row(
        1_001_004,
        &[
            ("ts", Cell::Ts(5_000_000)),
            ("backend_type", Cell::StrId(11)),
            ("state", Cell::StrId(12)),
            ("wait_event_type", Cell::StrId(13)),
            ("leader_pid", Cell::I32(42)),
            ("xact_start", Cell::Ts(3_000_000)),
        ],
    );

    assert_eq!(
        activity_sample(&row),
        ActivitySample {
            ts: Some(5_000_000),
            backend_type: Some(11),
            state: Some(12),
            wait_event_type: Some(13),
            leader: true,
            xact_start: Some(3_000_000),
        }
    );
}

#[test]
fn lock_waits_have_a_lane_distinct_from_other_backend_waits() {
    let counters = Counters {
        waiting: BTreeMap::from([(5_000_000, 1.0)]),
        lock_waiting: BTreeMap::from([(5_000_000, 0.0)]),
        ..Counters::default()
    };

    let lanes = points(&counters, 0, 0);
    assert_eq!(
        lanes
            .iter()
            .find(|point| point.key == "pg_waiting")
            .and_then(|point| point.value),
        Some(1.0)
    );
    assert_eq!(
        lanes
            .iter()
            .find(|point| point.key == "pg_lock_waiting")
            .and_then(|point| point.value),
        Some(0.0)
    );
}

#[test]
fn background_lock_waits_keep_the_lock_graph_visible() {
    let mut counters = Counters::default();
    let sample = ActivitySample {
        ts: Some(5_000_000),
        backend_type: Some(11),
        state: Some(12),
        wait_event_type: Some(13),
        leader: false,
        xact_start: None,
    };

    record_activity_sample(
        &mut counters,
        &sample,
        Some(b"autovacuum worker"),
        Some(b"active"),
        Some(b"Lock"),
    );

    assert_eq!(counters.lock_waiting, BTreeMap::from([(5_000_000, 1.0)]));
    assert_eq!(counters.waiting, BTreeMap::from([(5_000_000, 0.0)]));
}

fn row(type_id: u32, values: &[(&str, Cell)]) -> Row {
    let contract = contract(type_id).expect("fixture contract");
    let cells = contract
        .columns
        .iter()
        .map(|column| {
            values
                .iter()
                .find_map(|(name, value)| (*name == column.name).then(|| value.clone()))
                .unwrap_or(Cell::Null)
        })
        .collect();
    Row::new(contract, cells)
}

#[test]
fn a_shared_boundary_row_is_not_emitted_again_by_the_next_segment() {
    // Adjacent segments share the snapshot row at ts 200. The first segment
    // emits it with a computed rate; after retain_latest the next segment
    // holds only that row, whose rate is null — re-emitting it would conflict
    // with the value already sent.
    let mut counters = Counters {
        busy_ticks: BTreeMap::from([(100, 10), (200, 20)]),
        ..Counters::default()
    };
    let window = Window {
        from: Some(0),
        to: Some(1_000),
    };
    let first = current_points(&counters, 100, 1, 100, 200, window);
    assert!(
        first
            .iter()
            .any(|point| point.key == "cpu_busy" && point.ts == 200 && point.value.is_some())
    );
    counters.retain_latest();
    counters.busy_ticks.insert(200, 20);
    counters.busy_ticks.insert(300, 30);
    let second = current_points(&counters, 100, 1, 200_i64.saturating_add(1), 300, window);
    assert!(second.iter().all(|point| point.ts != 200));
    assert!(
        second
            .iter()
            .any(|point| point.key == "cpu_busy" && point.ts == 300 && point.value.is_some())
    );
}
