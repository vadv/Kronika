use std::collections::BTreeMap;

use super::points;
use crate::file::Point;
use crate::health::Stall;

/// One second, in the microseconds the counters use.
const SECOND: i64 = 1_000_000;

fn snapshots(readings: &[(i64, i64, i64, i64)]) -> BTreeMap<i64, Stall> {
    readings
        .iter()
        .map(|&(ts, cpu, memory, io)| (ts, Stall { cpu, memory, io }))
        .collect()
}

#[test]
fn no_snapshots_give_no_points() {
    assert_eq!(points(&BTreeMap::new()), Vec::new());
}

#[test]
fn the_first_snapshot_has_nothing_to_subtract_from() {
    let only = snapshots(&[(SECOND, 0, 0, 0)]);
    assert_eq!(
        points(&only),
        vec![Point {
            ts: SECOND,
            health: None
        }]
    );
}

#[test]
fn every_later_point_covers_the_interval_since_the_one_before() {
    let three = snapshots(&[
        (0, 0, 0, 0),
        (SECOND, SECOND / 4, 0, 0),
        (2 * SECOND, SECOND / 4, 0, 0),
    ]);
    assert_eq!(
        points(&three),
        vec![
            Point {
                ts: 0,
                health: None
            },
            Point {
                ts: SECOND,
                health: Some(75)
            },
            Point {
                ts: 2 * SECOND,
                health: Some(100)
            },
        ]
    );
}

#[test]
fn a_point_is_emitted_even_where_health_cannot_be_computed() {
    // The counters restarted between the second and third snapshot.
    let restarted = snapshots(&[(0, 0, 0, 0), (SECOND, SECOND, 0, 0), (2 * SECOND, 0, 0, 0)]);
    let built = points(&restarted);
    assert_eq!(built.len(), 3, "a snapshot always gets a point");
    assert_eq!(built[1].health, Some(0));
    assert_eq!(built[2].health, None, "the counter went backwards");
}

#[test]
fn points_come_out_oldest_first_whatever_order_they_went_in() {
    let mut out_of_order = BTreeMap::new();
    out_of_order.insert(
        2 * SECOND,
        Stall {
            cpu: 0,
            memory: 0,
            io: 0,
        },
    );
    out_of_order.insert(
        SECOND,
        Stall {
            cpu: 0,
            memory: 0,
            io: 0,
        },
    );
    let built = points(&out_of_order);
    assert_eq!(
        built.iter().map(|point| point.ts).collect::<Vec<_>>(),
        vec![SECOND, 2 * SECOND]
    );
}
