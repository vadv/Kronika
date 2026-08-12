use std::collections::BTreeMap;

use kronika_reader::{Cell, Row};
use kronika_registry::contract;

use super::{Counters, counter_sum, cpu_busy_ticks, current_points, points, rate};

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
    let current = current_points(&counters, 100, 1, 200, 200);
    let busy = current
        .iter()
        .find(|point| point.key == "cpu_busy")
        .expect("current busy point");
    assert_eq!(busy.ts, 200);
    assert_eq!(busy.value, Some(100_000.0));
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
