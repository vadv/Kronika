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

mod objects {
    use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
    use kronika_registry::os_meminfo::OsMeminfo;
    use kronika_registry::{Cell, Row, Section};

    use crate::build::objects;
    use crate::objects::Value;

    /// One `os_cgroup_cpu` row: identity `cgroup_path`, five counters, two
    /// gauges, and the scope label.
    fn cgroup(ts: i64, path: u64, usage: i64, quota: i64) -> Row {
        Row::new(
            &OsCgroupCpu::CONTRACT,
            vec![
                Cell::Ts(ts),
                Cell::StrId(path),
                Cell::I64(usage),
                Cell::I64(usage / 2),
                Cell::I64(usage / 2),
                Cell::I64(0),
                Cell::I64(0),
                Cell::I64(quota),
                Cell::I64(100_000),
                Cell::U32(0),
            ],
        )
    }

    fn names(id: u64) -> Option<String> {
        match id {
            1 => Some("/system.slice/postgresql.service".to_owned()),
            2 => Some("/user.slice".to_owned()),
            _unknown => None,
        }
    }

    #[test]
    fn a_counter_becomes_its_delta_and_a_gauge_its_last_reading() {
        let built = objects(
            &[
                cgroup(1_000, 1, 100, 200_000),
                cgroup(2_000, 1, 450, 400_000),
            ],
            names,
        )
        .expect("the section declares an identity");
        assert_eq!(built.type_id, 1_201_001);
        assert_eq!(built.objects.len(), 1);
        assert_eq!(built.objects[0].values[0], Value::Int(350));
        assert_eq!(built.objects[0].values[5], Value::Int(400_000));
    }

    #[test]
    fn one_object_per_identity_no_matter_how_many_snapshots() {
        let built = objects(
            &[
                cgroup(1_000, 1, 100, 0),
                cgroup(1_000, 2, 10, 0),
                cgroup(2_000, 1, 200, 0),
                cgroup(2_000, 2, 20, 0),
                cgroup(3_000, 1, 300, 0),
            ],
            names,
        )
        .expect("identity");
        assert_eq!(built.objects.len(), 2);
        assert_eq!(built.objects[0].values[0], Value::Int(200));
        assert_eq!(built.objects[1].values[0], Value::Int(10));
    }

    #[test]
    fn a_counter_that_went_backwards_has_no_delta() {
        let built = objects(&[cgroup(1_000, 1, 900, 0), cgroup(2_000, 1, 100, 0)], names)
            .expect("identity");
        assert_eq!(built.objects[0].values[0], Value::Null);
    }

    #[test]
    fn one_snapshot_gives_a_zero_delta_and_the_reading_it_had() {
        let built = objects(&[cgroup(1_000, 1, 900, 5)], names).expect("identity");
        assert_eq!(built.objects[0].values[0], Value::Int(0));
        assert_eq!(built.objects[0].values[5], Value::Int(5));
    }

    #[test]
    fn a_label_comes_out_as_the_segment_interned_it() {
        let built = objects(&[cgroup(1_000, 1, 0, 0)], names).expect("identity");
        assert_eq!(built.label_count, 2);
        assert_eq!(
            built.objects[0].labels[0],
            "/system.slice/postgresql.service"
        );
        assert_eq!(built.objects[0].labels[1], "0");
    }

    #[test]
    fn an_id_the_dictionary_does_not_hold_says_so_rather_than_going_blank() {
        let built = objects(&[cgroup(1_000, 7, 0, 0)], names).expect("identity");
        assert_eq!(built.objects[0].labels[0], "<str 7>");
    }

    #[test]
    fn objects_are_ordered_by_their_identity() {
        let built =
            objects(&[cgroup(1_000, 2, 0, 0), cgroup(1_000, 1, 0, 0)], names).expect("identity");
        assert_eq!(
            built.objects[0].labels[0],
            "/system.slice/postgresql.service"
        );
        assert_eq!(built.objects[1].labels[0], "/user.slice");
    }

    #[test]
    fn a_section_with_one_row_per_snapshot_has_no_objects_to_reduce() {
        let row = Row::new(&OsMeminfo::CONTRACT, vec![Cell::Ts(1_000)]);
        assert!(objects(&[row], names).is_none());
    }

    #[test]
    fn nothing_to_read_is_nothing_to_build() {
        assert!(objects(&[], names).is_none());
    }
}
