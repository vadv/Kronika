use std::cell::Cell as TestCell;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::os_process::OsProcess;
use kronika_registry::pg_stat_progress_vacuum::PgStatProgressVacuumV2;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::json;

use super::*;
use crate::api::{ValueLimits, ValueStopReason};

const BASE: i64 = 1_709_164_800_000_000;

#[test]
fn registry_projection_covers_pg10_through_pg18() {
    let ids = vacuum_contracts()
        .iter()
        .map(|item| item.type_id.get())
        .collect::<Vec<_>>();
    assert_eq!(ids, [1_012_004, 1_012_005, 1_012_006]);

    let old = available_fields(&BTreeSet::from([1_012_004]));
    assert!(old.contains(&"max_dead_tuples"));
    assert!(!old.contains(&"indexes_total"));
    assert!(!old.contains(&"delay_time"));

    let current = available_fields(&BTreeSet::from([1_012_005, 1_012_006]));
    assert!(current.contains(&"indexes_total"));
    assert!(current.contains(&"delay_time"));
    assert_eq!(
        projected_fields(&[]).expect("shared default projection"),
        DEFAULT_VACUUM_FIELDS
    );

    let error =
        projected_fields(&["imaginary".to_owned()]).expect_err("unknown projection is refused");
    assert_eq!(error.code(), "invalid_vacuum_fields");
    assert_eq!(error.parameter(), Some("fields"));
}

#[test]
fn manual_and_autovacuum_workers_keep_recorded_identity_and_kind() {
    let policies = Policies::load().expect("Vacuum semantics");
    let rows = vec![
        sample(
            1_012_004,
            1,
            0,
            BASE,
            41,
            false,
            "scanning heap",
            10,
            0,
            0,
            None,
            None,
            None,
        ),
        sample(
            1_012_004,
            1,
            1,
            BASE,
            42,
            true,
            "scanning heap",
            10,
            0,
            0,
            None,
            None,
            None,
        ),
    ];
    let episodes = build_episodes(rows, &policies).expect("manual and automatic episodes");
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[0].key.pid, 41);
    assert_eq!(episodes[1].key.pid, 42);
    assert_eq!(
        relation_value(episode_last(&episodes[0]).expect("manual sample"))
            .expect("manual relation")["is_autovacuum"],
        json!(false)
    );
    assert_eq!(
        relation_value(episode_last(&episodes[1]).expect("automatic sample"))
            .expect("automatic relation")["is_autovacuum"],
        json!(true)
    );
}

#[test]
fn complete_phase_history_keeps_index_cycles_risk_and_stillness() {
    let policies = Policies::load().expect("Vacuum semantics");
    let phases = [
        ("scanning heap", 0, Some(0)),
        ("vacuuming indexes", 0, Some(2)),
        ("vacuuming indexes", 0, Some(2)),
        ("vacuuming indexes", 0, Some(2)),
        ("vacuuming indexes", 1, Some(0)),
        ("truncating heap", 1, Some(0)),
    ];
    let rows = phases
        .into_iter()
        .enumerate()
        .map(|(index, (phase, cycle, processed))| {
            let index = i64::try_from(index).expect("small phase index");
            with_cadence(
                sample(
                    1_012_005,
                    1,
                    u64::try_from(index).expect("nonnegative phase index"),
                    BASE + index * 10_000_000,
                    42,
                    true,
                    phase,
                    10 + index,
                    0,
                    cycle,
                    Some(4),
                    processed,
                    None,
                ),
                10,
            )
        })
        .collect();
    let episodes = build_episodes(rows, &policies).expect("one repeated-cycle episode");
    assert_eq!(episodes.len(), 1);
    let episode = &episodes[0];
    assert_eq!(
        episode
            .phases
            .iter()
            .map(|phase| (phase.name.as_str(), phase.cycle))
            .collect::<Vec<_>>(),
        [
            ("scanning heap", Some(0)),
            ("vacuuming indexes", Some(0)),
            ("vacuuming indexes", Some(1)),
            ("truncating heap", Some(1)),
        ]
    );
    let index_phase = &episode.phases[1];
    let still = index_phase
        .no_movement
        .as_ref()
        .expect("three unchanged index readings");
    assert_eq!(still.field, "indexes_processed");
    assert_eq!(still.samples, 3);
    assert_eq!(policies.risk("vacuuming indexes"), VacuumRisk::Heavy);
    assert_eq!(policies.risk("truncating heap"), VacuumRisk::Dangerous);
    assert_eq!(index_cycles(episode).expect("index cycles").len(), 2);
}

#[test]
fn pg10_index_phase_never_claims_counter_stillness() {
    let policies = Policies::load().expect("Vacuum semantics");
    let rows = (0..3)
        .map(|index| {
            let index_i64 = i64::try_from(index).expect("small sample index");
            with_cadence(
                sample(
                    1_012_004,
                    1,
                    index,
                    BASE + index_i64 * 10_000_000,
                    42,
                    true,
                    "vacuuming indexes",
                    20,
                    0,
                    0,
                    None,
                    None,
                    None,
                ),
                10,
            )
        })
        .collect();
    let episodes = build_episodes(rows, &policies).expect("old-layout episode");
    assert!(episodes[0].phases[0].no_movement.is_none());
}

#[test]
fn each_adjacency_uses_the_later_samples_recorded_cadence() {
    let policies = Policies::load().expect("Vacuum semantics");
    let rows = vec![
        with_cadence(sample_at(1, 0, BASE), 10),
        with_cadence(sample_at(1, 1, BASE + 25_000_000), 10),
        with_cadence(sample_at(1, 2, BASE + 51_000_000), 10),
        with_cadence(sample_at(2, 3, BASE + 81_000_000), 20),
    ];
    let episodes = build_episodes(rows, &policies).expect("per-segment cadence");
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[0].rows.len(), 2);
    assert_eq!(episodes[1].rows.len(), 2);

    let without_cadence = vec![sample_at(1, 0, BASE), sample_at(2, 0, BASE + 600_000_000)];
    assert_eq!(
        build_episodes(without_cadence, &policies)
            .expect("missing cadence remains missing")
            .len(),
        1
    );
}

#[test]
fn elapsed_time_and_counter_regression_start_new_episodes() {
    let policies = Policies::load().expect("Vacuum semantics");
    let rows = vec![
        with_cadence(sample_at(1, 0, BASE), 10),
        with_cadence(sample_at(1, 1, BASE + 30_000_000), 10),
        with_cadence(
            sample(
                1_012_005,
                1,
                2,
                BASE + 40_000_000,
                42,
                true,
                "scanning heap",
                1,
                0,
                0,
                Some(4),
                Some(0),
                None,
            ),
            10,
        ),
    ];
    let episodes = build_episodes(rows, &policies).expect("mechanical episode boundaries");
    assert_eq!(episodes.len(), 3);
}

#[test]
fn layout_progress_and_pg18_delay_remain_exact() {
    let policies = Policies::load().expect("Vacuum semantics");
    let old = finish_episode(
        sample_at(1, 0, BASE).key,
        vec![sample(
            1_012_004,
            1,
            0,
            BASE,
            42,
            true,
            "scanning heap",
            50,
            0,
            0,
            None,
            None,
            None,
        )],
        &policies,
    )
    .expect("PG10 episode");
    assert!(progress_value(&old).expect("old progress")["index"].is_null());

    let pg18 = vec![
        sample(
            1_012_006,
            1,
            0,
            BASE,
            42,
            true,
            "vacuuming indexes",
            50,
            0,
            0,
            Some(4),
            Some(1),
            Some(4.5),
        ),
        sample(
            1_012_006,
            1,
            1,
            BASE + 10_000_000,
            42,
            true,
            "vacuuming indexes",
            60,
            0,
            0,
            Some(4),
            Some(2),
            Some(7.0),
        ),
    ];
    let episode = build_episodes(pg18, &policies)
        .expect("PG18 episode")
        .remove(0);
    let progress = progress_value(&episode).expect("PG18 progress");
    assert_eq!(progress["index"]["applicable"], json!(true));
    assert_eq!(progress["heap_scan"][1]["percent"], json!(60.0));
    assert_eq!(delay_delta(&episode).expect("delay delta"), Some(2.5));
}

#[test]
fn process_enrichment_preserves_endpoint_deltas_and_current_row() {
    let before = process_row(1, 0, BASE, 42, 10, 5, 1, 20, Some(100), Some(40), 2);
    let after = process_row(
        1,
        1,
        BASE + 10_000_000,
        42,
        30,
        15,
        5,
        60,
        Some(500),
        Some(90),
        5,
    );
    let enrichment = ProcessEnrichment {
        current: Some(after.clone()),
        before: Some(before),
        after: Some(after),
        clock_ticks_per_sec: Some(100),
    };
    let value = process_value(Some(&enrichment)).expect("process enrichment");
    assert_eq!(value["current_row"]["values"]["pid"], json!(42));
    assert_eq!(value["load"]["cpu_ms"], json!(300.0));
    assert_eq!(value["load"]["cpu_share_percent"], json!(3.0));
    assert_eq!(value["load"]["block_wait_ms"], json!(40.0));
    assert_eq!(value["load"]["run_delay_ns"], json!("40"));
    assert_eq!(value["load"]["read_bytes"], json!("400"));
    assert_eq!(value["load"]["write_bytes"], json!("50"));
    assert_eq!(value["load"]["major_faults"], json!("3"));
}

#[test]
fn admission_limits_and_duplicate_locators_are_atomic_preconditions() {
    let mut admitted = (0..MAX_VACUUM_SAMPLES)
        .map(|ordinal| {
            sample_at(
                1,
                u64::try_from(ordinal).expect("test ordinal fits u64"),
                BASE + i64::try_from(ordinal).expect("test ordinal fits i64"),
            )
        })
        .collect::<Vec<_>>();
    admit_samples(&admitted).expect("exact native sample limit");

    admitted.push(sample_at(
        1,
        u64::try_from(MAX_VACUUM_SAMPLES).expect("sample limit fits u64"),
        BASE + 600,
    ));
    assert_eq!(
        admit_samples(&admitted)
            .expect_err("native sample limit plus one")
            .code(),
        "sample_bound_exceeded"
    );

    let mut duplicate = vec![sample_at(1, 0, BASE), sample_at(1, 0, BASE + 1)];
    duplicate[1].row.ordinal = duplicate[0].row.ordinal;
    assert_eq!(
        admit_samples(&duplicate)
            .expect_err("repeated locator")
            .code(),
        "malformed_vacuum_history"
    );

    let identities = (0..=MAX_VACUUM_IDENTITIES)
        .map(|index| {
            sample(
                1_012_005,
                1,
                index as u64,
                BASE,
                i32::try_from(index + 1).expect("test pid"),
                true,
                "scanning heap",
                1,
                0,
                0,
                Some(1),
                Some(0),
                None,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admit_samples(&identities)
            .expect_err("identity limit plus one")
            .code(),
        "entity_bound_exceeded"
    );
}

#[test]
fn observation_sort_uses_only_samples_at_or_before_request_time() {
    let policies = Policies::load().expect("Vacuum semantics");
    let mut episodes = build_episodes(
        vec![
            sample(
                1_012_005,
                1,
                0,
                BASE,
                41,
                true,
                "truncating heap",
                1,
                0,
                0,
                Some(1),
                Some(0),
                None,
            ),
            sample(
                1_012_005,
                1,
                1,
                BASE + 10,
                42,
                true,
                "scanning heap",
                1,
                0,
                0,
                Some(1),
                Some(0),
                None,
            ),
            sample(
                1_012_005,
                1,
                2,
                BASE + 20,
                43,
                true,
                "vacuuming heap",
                1,
                0,
                0,
                Some(1),
                Some(0),
                None,
            ),
        ],
        &policies,
    )
    .expect("sortable episodes");
    sort_episodes(&mut episodes, Some(BASE + 10), &policies).expect("request-time sort");
    assert_eq!(episodes[0].key.pid, 42);
}

#[test]
fn direct_requests_enforce_exact_interval_and_whole_set_bounds() {
    let invalid = VacuumRequest {
        from: BASE,
        to: BASE + HOUR_US,
        at: BASE,
        fields: Vec::new(),
        page_size: MAX_VACUUM_EPISODES,
    };
    assert_eq!(
        validate_request(&invalid)
            .expect_err("cross-hour interval")
            .parameter(),
        Some("to_us")
    );
    let invalid = VacuumRequest {
        from: BASE,
        to: BASE + 10,
        at: BASE + 11,
        fields: Vec::new(),
        page_size: MAX_VACUUM_EPISODES,
    };
    assert_eq!(
        validate_request(&invalid)
            .expect_err("observation outside interval")
            .parameter(),
        Some("to_us")
    );
    let invalid = VacuumRequest {
        from: BASE,
        to: BASE + 10,
        at: BASE + 10,
        fields: Vec::new(),
        page_size: 0,
    };
    assert_eq!(
        validate_request(&invalid)
            .expect_err("zero admission")
            .parameter(),
        Some("page_size")
    );
}

#[test]
fn prepared_product_stays_on_its_captured_active_prefix() {
    let mut fixture = Fixture::new();
    fixture.append(10, &[(BASE + 10, 42, 10), (BASE + 20, 42, 20)]);
    let prepared = prepare(
        fixture.root(),
        request(BASE + 10, BASE + 20, BASE + 20, MAX_VACUUM_EPISODES),
    )
    .expect("prepare active Vacuum product");
    fixture.append(20, &[(BASE + 20, 99, 5)]);

    let collected = crate::api::Prepared::Vacuum(prepared)
        .collect_values(
            ValueLimits {
                records: 2,
                ndjson_bytes: usize::MAX,
            },
            &|| false,
        )
        .expect("read captured Vacuum product");
    assert_eq!(collected.stop_reason, ValueStopReason::Complete);
    assert_eq!(collected.records.len(), 1);
    let product = &collected.records[0];
    assert_eq!(product["episodes"].as_array().map(Vec::len), Some(1));
    assert_eq!(product["episodes"][0]["identity"]["pid"], json!(42));
    assert!(product["anchor"]["active_wal_position"].is_string());
    assert_eq!(product["anchor"]["cadence_seconds"], json!(10));
}

#[test]
fn exact_interval_keeps_both_edges_and_excludes_neighbors() {
    let mut fixture = Fixture::new();
    fixture.append(
        10,
        &[
            (BASE + 9, 42, 9),
            (BASE + 10, 42, 10),
            (BASE + 20, 42, 20),
            (BASE + 21, 42, 21),
        ],
    );
    let prepared = prepare(
        fixture.root(),
        request(BASE + 10, BASE + 20, BASE + 20, MAX_VACUUM_EPISODES),
    )
    .expect("prepare exact Vacuum interval");
    let collected = crate::api::Prepared::Vacuum(prepared)
        .collect_values(
            ValueLimits {
                records: 2,
                ndjson_bytes: usize::MAX,
            },
            &|| false,
        )
        .expect("read exact Vacuum interval");
    let samples = collected.records[0]["episodes"][0]["samples"]
        .as_array()
        .expect("episode samples");
    assert_eq!(
        samples
            .iter()
            .map(|sample| sample["timestamp"].as_str().expect("sample timestamp"))
            .collect::<Vec<_>>(),
        [(BASE + 10).to_string(), (BASE + 20).to_string()]
    );
}

#[test]
fn http_route_returns_recorded_relation_and_process_enrichment() {
    let mut fixture = Fixture::new();
    fixture.append(
        10,
        &[(BASE + 10_000_000, 42, 10), (BASE + 20_000_000, 42, 20)],
    );
    let route = crate::route::parse(
        "/api/postgresql/vacuum",
        Some(&format!(
            "from={}&to={}&at={}&page_size=500",
            BASE + 10_000_000,
            BASE + 20_000_000,
            BASE + 20_000_000
        )),
    )
    .expect("HTTP Vacuum route");
    let collected = crate::api::prepare(fixture.root(), 0, route, None)
        .expect("prepare HTTP Vacuum product")
        .collect_values(
            ValueLimits {
                records: 2,
                ndjson_bytes: usize::MAX,
            },
            &|| false,
        )
        .expect("read HTTP Vacuum product");

    let episode = &collected.records[0]["episodes"][0];
    assert_eq!(episode["relation"]["database"], json!("inventory"));
    assert_eq!(episode["relation"]["schema"], json!("public"));
    assert_eq!(episode["relation"]["name"], json!("items"));
    assert_eq!(
        episode["process"]["current_row"]["values"]["pid"],
        json!(42)
    );
    assert_eq!(episode["process"]["load"]["cpu_ms"], json!(300.0));
    assert_eq!(episode["process"]["load"]["cpu_share_percent"], json!(3.0));
    assert_eq!(episode["process"]["load"]["read_bytes"], json!("1000"));
    assert_eq!(episode["process"]["load"]["write_bytes"], json!("100"));
}

#[test]
fn cancellation_and_whole_set_rejection_emit_no_product_record() {
    let mut fixture = Fixture::new();
    fixture.append(10, &[(BASE + 10, 42, 10), (BASE + 20, 99, 10)]);

    let prepared = prepare(
        fixture.root(),
        request(BASE + 10, BASE + 20, BASE + 20, MAX_VACUUM_EPISODES),
    )
    .expect("prepare cancellable Vacuum product");
    let checks = TestCell::new(0_usize);
    let cancelled = || {
        let current = checks.get().saturating_add(1);
        checks.set(current);
        current >= 4
    };
    let collected = crate::api::Prepared::Vacuum(prepared)
        .collect_values(
            ValueLimits {
                records: 2,
                ndjson_bytes: usize::MAX,
            },
            &cancelled,
        )
        .expect("cancel Vacuum product");
    assert!(checks.get() >= 4);
    assert!(collected.records.is_empty());
    assert_eq!(collected.stop_reason, ValueStopReason::Cancelled);

    let prepared = prepare(fixture.root(), request(BASE + 10, BASE + 20, BASE + 20, 1))
        .expect("prepare bounded Vacuum product");
    let mut emitted = Vec::new();
    let error = prepared
        .stream(
            &mut |record| {
                emitted.push(record);
                true
            },
            &|| false,
        )
        .expect_err("two episodes exceed one admitted episode");
    assert_eq!(error.code(), "whole_set_bound_exceeded");
    assert!(emitted.is_empty());
}

fn request(from: i64, to: i64, at: i64, page_size: usize) -> VacuumRequest {
    VacuumRequest {
        from,
        to,
        at,
        fields: Vec::new(),
        page_size,
    }
}

#[allow(clippy::too_many_arguments, reason = "compact exact-layout test rows")]
fn sample(
    type_id: u32,
    segment_id: i64,
    ordinal: u64,
    timestamp: i64,
    pid: i32,
    is_autovacuum: bool,
    phase: &str,
    scanned: i64,
    vacuumed: i64,
    cycle: i64,
    indexes_total: Option<i64>,
    indexes_processed: Option<i64>,
    delay_time: Option<f64>,
) -> Sample {
    let mut values = Map::from_iter([
        ("ts".to_owned(), json!(timestamp.to_string())),
        ("pid".to_owned(), json!(pid)),
        ("datid".to_owned(), json!(16_385)),
        ("datname".to_owned(), json!("app")),
        ("relid".to_owned(), json!(16_384)),
        ("schemaname".to_owned(), json!("public")),
        ("relname".to_owned(), json!("orders")),
        ("is_autovacuum".to_owned(), json!(is_autovacuum)),
        ("phase".to_owned(), json!(phase)),
        ("heap_blks_total".to_owned(), json!("100")),
        ("heap_blks_scanned".to_owned(), json!(scanned.to_string())),
        ("heap_blks_vacuumed".to_owned(), json!(vacuumed.to_string())),
        ("index_vacuum_count".to_owned(), json!(cycle.to_string())),
    ]);
    match type_id {
        1_012_004 => {
            values.insert("max_dead_tuples".to_owned(), json!("1000"));
            values.insert("num_dead_tuples".to_owned(), json!("50"));
        }
        1_012_005 | 1_012_006 => {
            values.insert("max_dead_tuple_bytes".to_owned(), json!("4096"));
            values.insert("dead_tuple_bytes".to_owned(), json!("1024"));
            values.insert("num_dead_item_ids".to_owned(), json!("50"));
            values.insert(
                "indexes_total".to_owned(),
                indexes_total.map_or(Value::Null, |value| json!(value.to_string())),
            );
            values.insert(
                "indexes_processed".to_owned(),
                indexes_processed.map_or(Value::Null, |value| json!(value.to_string())),
            );
            if type_id == 1_012_006 {
                values.insert(
                    "delay_time".to_owned(),
                    delay_time.map_or(Value::Null, |value| json!(value)),
                );
            }
        }
        _ => panic!("unsupported test layout"),
    }
    Sample {
        key: EpisodeKey {
            type_id,
            pid,
            datid: 16_385,
            relid: 16_384,
        },
        row: NamedRow {
            segment_id,
            logical_name: VACUUM_SECTION,
            type_id,
            ordinal,
            timestamp,
            values,
        },
        cadence_seconds: None,
    }
}

fn sample_at(segment_id: i64, ordinal: u64, timestamp: i64) -> Sample {
    sample(
        1_012_005,
        segment_id,
        ordinal,
        timestamp,
        42,
        true,
        "scanning heap",
        i64::try_from(ordinal).expect("test counter") + 10,
        0,
        0,
        Some(4),
        Some(0),
        None,
    )
}

fn with_cadence(mut sample: Sample, seconds: u64) -> Sample {
    sample.cadence_seconds = Some(seconds);
    sample
}

#[allow(
    clippy::too_many_arguments,
    reason = "compact process counter test rows"
)]
fn process_row(
    segment_id: i64,
    ordinal: u64,
    timestamp: i64,
    pid: i32,
    utime: i64,
    stime: i64,
    block: i64,
    run: i64,
    read: Option<i64>,
    write: Option<i64>,
    faults: i64,
) -> NamedRow {
    NamedRow {
        segment_id,
        logical_name: PROCESS_SECTION,
        type_id: 1_100_001,
        ordinal,
        timestamp,
        values: Map::from_iter([
            ("ts".to_owned(), json!(timestamp.to_string())),
            ("pid".to_owned(), json!(pid)),
            ("utime".to_owned(), json!(utime.to_string())),
            ("stime".to_owned(), json!(stime.to_string())),
            ("blkdelay_ticks".to_owned(), json!(block.to_string())),
            ("rundelay_ns".to_owned(), json!(run.to_string())),
            (
                "read_bytes".to_owned(),
                read.map_or(Value::Null, |value| json!(value.to_string())),
            ),
            (
                "write_bytes".to_owned(),
                write.map_or(Value::Null, |value| json!(value.to_string())),
            ),
            ("majflt".to_owned(), json!(faults.to_string())),
        ]),
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary Vacuum data root");
        let root = DataRoot::open(directory.path()).expect("open Vacuum data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire Vacuum fixture writer");
        let journal =
            Journal::open(&writer, JournalConfig::default()).expect("open Vacuum fixture WAL");
        let address = SegmentAddress::new(SegmentId::new(BASE).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            _writer: writer,
            journal,
            address,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn append(&mut self, cadence_seconds: u64, readings: &[(i64, i32, i64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let hostname = label(&mut interner, "db-01");
        let kernel = label(&mut interner, "6.12");
        let boot = label(&mut interner, "fixture-boot");
        let datname = label(&mut interner, "inventory");
        let schema = label(&mut interner, "public");
        let relation = label(&mut interner, "items");
        let scanning = label(&mut interner, "scanning heap");
        let command = label(&mut interner, "postgres: vacuum worker");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(BASE + 1),
                hostname,
                kernel_version: kernel,
                environment: Environment::Machine.as_u8(),
                clock_ticks_per_sec: 100,
                page_size_bytes: 4096,
                boot_id: boot,
                btime: Ts(BASE - 1_000_000),
                postgresql_enabled: true,
                postgresql_interval_seconds: cadence_seconds,
                postgresql_effective_cpus: Some(4),
            })
            .expect("metadata row fits");
        for &(timestamp, pid, scanned) in readings {
            buffers
                .push(PgStatProgressVacuumV2 {
                    ts: Ts(timestamp),
                    pid,
                    datid: 16_384,
                    datname,
                    relid: 20_000,
                    schemaname: Some(schema),
                    relname: Some(relation),
                    is_autovacuum: true,
                    phase: scanning,
                    heap_blks_total: 1_000,
                    heap_blks_scanned: scanned,
                    heap_blks_vacuumed: 50,
                    index_vacuum_count: 1,
                    max_dead_tuple_bytes: 67_108_864,
                    dead_tuple_bytes: 1_024,
                    num_dead_item_ids: 20,
                    indexes_total: 2,
                    indexes_processed: 1,
                })
                .expect("Vacuum row fits");
            buffers
                .push(process_fixture(timestamp, pid, scanned, command))
                .expect("Process row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode Vacuum dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode Vacuum fixture")
            .expect("nonempty Vacuum fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append Vacuum fixture");
    }
}

fn process_fixture(timestamp: i64, pid: i32, step: i64, command: StrId) -> OsProcess {
    OsProcess {
        ts: Ts(timestamp),
        pid,
        starttime: Ts(BASE - 1_000_000 + i64::from(pid)),
        ppid: 1,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'R',
        num_threads: 1,
        tty: 0,
        comm: command,
        cmdline: Some(command),
        utime: step.saturating_mul(2),
        stime: step,
        nice: 0,
        prio: 20,
        rtprio: 0,
        policy: 0,
        curcpu: 0,
        rundelay_ns: step.saturating_mul(10),
        blkdelay_ticks: step,
        nvcsw: 0,
        nivcsw: 0,
        minflt: 0,
        majflt: step,
        vmem_kb: 1_024,
        rmem_kb: 512,
        vswap_kb: 0,
        syscr: Some(0),
        syscw: Some(0),
        rchar: Some(0),
        wchar: Some(0),
        read_bytes: Some(step.saturating_mul(100)),
        write_bytes: Some(step.saturating_mul(10)),
        cancelled_write_bytes: Some(0),
        exit_signal: 17,
        scope: 0,
    }
}

fn label(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern Vacuum fixture label")
            .get(),
    )
}
