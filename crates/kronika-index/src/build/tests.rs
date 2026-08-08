use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::os_psi::OsPsi;
use kronika_registry::{Cell, Row, Section};

use super::{CONTAINER, HOST, POD, points};
use crate::file::Point;

/// One second, in the microseconds the counters use.
const SECOND: i64 = 1_000_000;

fn metadata(environment: Environment) -> Row {
    metadata_raw(u32::from(environment.as_u8()))
}

fn metadata_raw(environment: u32) -> Row {
    Row::new(
        &InstanceMetadata::CONTRACT,
        vec![
            Cell::Ts(0),
            Cell::StrId(1),
            Cell::StrId(2),
            Cell::U32(environment),
        ],
    )
}

fn psi(ts: i64, resource: u32, total: i64, scope: u32) -> Row {
    Row::new(
        &OsPsi::CONTRACT,
        vec![
            Cell::Ts(ts),
            Cell::U32(resource),
            Cell::F64(0.0),
            Cell::F64(0.0),
            Cell::F64(0.0),
            Cell::I64(total),
            Cell::Null,
            Cell::Null,
            Cell::Null,
            Cell::Null,
            Cell::U32(scope),
        ],
    )
}

fn snapshot(rows: &mut Vec<Row>, ts: i64, cpu: i64, memory: i64, io: i64, scope: u32) {
    rows.extend([
        psi(ts, 0, cpu, scope),
        psi(ts, 1, memory, scope),
        psi(ts, 2, io, scope),
    ]);
}

#[test]
fn no_pressure_snapshots_give_no_points() {
    assert_eq!(points(&[metadata(Environment::Machine)], &[]), Vec::new());
}

#[test]
fn machine_health_uses_adjacent_complete_host_snapshots() {
    let mut rows = Vec::new();
    snapshot(&mut rows, 0, 0, 0, 0, HOST);
    snapshot(&mut rows, SECOND, SECOND / 4, 0, 0, HOST);
    snapshot(&mut rows, 2 * SECOND, SECOND / 4, 0, 0, HOST);

    assert_eq!(
        points(&[metadata(Environment::Machine)], &rows),
        vec![
            Point {
                ts: 0,
                health: None,
            },
            Point {
                ts: SECOND,
                health: Some(75),
            },
            Point {
                ts: 2 * SECOND,
                health: Some(100),
            },
        ]
    );
}

#[test]
fn container_host_pressure_keeps_timestamps_but_has_no_health() {
    let mut rows = Vec::new();
    snapshot(&mut rows, 0, 0, 0, 0, HOST);
    snapshot(&mut rows, SECOND, SECOND / 2, 0, 0, HOST);

    assert_eq!(
        points(&[metadata(Environment::Container)], &rows),
        vec![
            Point {
                ts: 0,
                health: None,
            },
            Point {
                ts: SECOND,
                health: None,
            },
        ]
    );
}

#[test]
fn container_own_cgroup_pressure_can_produce_health() {
    for scope in [POD, CONTAINER] {
        let mut rows = Vec::new();
        snapshot(&mut rows, 0, 0, 0, 0, scope);
        snapshot(&mut rows, SECOND, 0, SECOND / 2, 0, scope);

        assert_eq!(
            points(&[metadata(Environment::Container)], &rows)[1].health,
            Some(50)
        );
    }
}

#[test]
fn missing_or_unknown_environment_keeps_timestamps_but_has_no_health() {
    let mut rows = Vec::new();
    snapshot(&mut rows, 0, 0, 0, 0, HOST);
    snapshot(&mut rows, SECOND, SECOND / 2, 0, 0, HOST);

    for metadata_rows in [&[][..], &[metadata_raw(9)][..]] {
        assert_eq!(
            points(metadata_rows, &rows),
            vec![
                Point {
                    ts: 0,
                    health: None,
                },
                Point {
                    ts: SECOND,
                    health: None,
                },
            ]
        );
    }
}

#[test]
fn a_missing_resource_resets_the_baseline_until_two_complete_snapshots() {
    let mut rows = Vec::new();
    snapshot(&mut rows, 0, 0, 0, 0, HOST);
    rows.extend([psi(SECOND, 0, SECOND / 4, HOST), psi(SECOND, 1, 0, HOST)]);
    snapshot(&mut rows, 2 * SECOND, SECOND / 2, 0, 0, HOST);
    snapshot(&mut rows, 3 * SECOND, 3 * SECOND / 4, 0, 0, HOST);

    let built = points(&[metadata(Environment::Machine)], &rows);
    assert_eq!(built.len(), 4, "every observed timestamp gets a point");
    assert_eq!(built[0].health, None, "the first snapshot is a baseline");
    assert_eq!(built[1].health, None, "io is absent");
    assert_eq!(built[2].health, None, "io reappears and starts a baseline");
    assert_eq!(built[3].health, Some(75));
}

#[test]
fn points_come_out_oldest_first_whatever_row_order_was_read() {
    let mut rows = Vec::new();
    snapshot(&mut rows, 2 * SECOND, 0, 0, 0, HOST);
    snapshot(&mut rows, SECOND, 0, 0, 0, HOST);

    let built = points(&[metadata(Environment::Machine)], &rows);
    assert_eq!(
        built.iter().map(|point| point.ts).collect::<Vec<_>>(),
        vec![SECOND, 2 * SECOND]
    );
}
