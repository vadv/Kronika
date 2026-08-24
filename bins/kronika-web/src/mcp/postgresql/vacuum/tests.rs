use std::path::Path;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::pg_stat_progress_vacuum::PgStatProgressVacuumV2;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;

use super::super::resolve_anchor;
use super::cadence::recorded_cadence;
use super::reader::{collect_hour, decode_hour};
use super::{
    EpisodeKey, Policies, Sample, adjacency_limit, admit_samples, build_episodes, episode_value,
    execute, sort_episodes,
};
use crate::mcp::State;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const FROM: i64 = SEGMENT_ID;
const TO: i64 = SEGMENT_ID + 30_000_000;

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
            .expect("acquire fixture writer");
        let journal = Journal::open(&writer, JournalConfig::default()).expect("open fixture WAL");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
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

    fn append_history(&mut self) {
        self.append_history_with_cadence(10);
    }

    fn append_history_with_cadence(&mut self, cadence_seconds: u64) {
        let mut interner = Interner::new(DictLimits::default());
        let hostname = label(&mut interner, "db-01");
        let kernel = label(&mut interner, "6.12");
        let boot = label(&mut interner, "fixture-boot");
        let datname = label(&mut interner, "inventory");
        let schema = label(&mut interner, "public");
        let relation = label(&mut interner, "items");
        let scanning = label(&mut interner, "scanning heap");
        let truncating = label(&mut interner, "truncating heap");

        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(FROM + 1),
                hostname,
                kernel_version: kernel,
                environment: Environment::Machine.as_u8(),
                clock_ticks_per_sec: 100,
                page_size_bytes: 4096,
                boot_id: boot,
                btime: Ts(FROM - 1_000_000),
                postgresql_enabled: true,
                postgresql_interval_seconds: cadence_seconds,
                postgresql_effective_cpus: Some(4),
            })
            .expect("metadata row fits");
        for (timestamp, scanned) in [
            (FROM + 10_000_000, 100),
            (FROM + 20_000_000, 100),
            (TO, 100),
        ] {
            buffers
                .push(vacuum_row(
                    timestamp, 100, 16_384, 20_000, datname, schema, relation, scanning, scanned,
                ))
                .expect("scanning row fits");
        }
        buffers
            .push(vacuum_row(
                TO, 200, 16_384, 30_000, datname, schema, relation, truncating, 500,
            ))
            .expect("truncating row fits");
        let dictionary = dict::encode(interner.window()).expect("encode Vacuum dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode Vacuum fixture")
            .expect("nonempty Vacuum fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append Vacuum fixture");
    }

    fn state(&self) -> State {
        State {
            data_root: self.root().to_owned(),
            sources: 0b10,
            synthetic_demo: false,
            heavy_scans: Arc::new(Semaphore::new(2)),
        }
    }
}

#[test]
fn active_vacuum_passes_reuse_one_captured_prefix() {
    let mut fixture = Fixture::new();
    fixture.append_history_with_cadence(10);
    let state = fixture.state();
    let anchor = resolve_anchor(&state, TO, &[super::SECTION, "instance_metadata"], &|| {
        false
    })
    .expect("capture one Vacuum source prefix");
    let captured = anchor
        .active_wal_position
        .expect("active Vacuum source prefix");

    fixture.append_history_with_cadence(20);
    let collected = collect_hour(&state, FROM, TO, &anchor, &|| false)
        .expect("read Vacuum rows from captured prefix");
    let decoded = decode_hour(collected.records).expect("decode captured Vacuum rows");
    let cadence = recorded_cadence(&state, &anchor, TO, &|| false)
        .expect("read cadence from captured prefix");

    assert_eq!(decoded.rows.len(), 4);
    assert_eq!(cadence.seconds, Some(10));
    assert_eq!(anchor.active_wal_position, Some(captured));
}

#[test]
fn reducer_uses_cadence_monotonicity_and_accepted_sorting() {
    let policies = Policies::load().expect("accepted Vacuum policies");
    let limit = adjacency_limit(10, policies.adjacency_factor).expect("adjacency limit");
    let rows = vec![
        sample(10, 1, 10_000_000, "scanning heap", 10, 0),
        sample(10, 1, 20_000_000, "scanning heap", 10, 1),
        sample(10, 1, 30_000_000, "scanning heap", 10, 2),
        // More than 2.5 recorded intervals starts another episode.
        sample(10, 1, 60_000_000, "scanning heap", 11, 3),
        sample(20, 2, 40_000_000, "truncating heap", 20, 4),
        sample(20, 2, 50_000_000, "truncating heap", 20, 5),
        sample(20, 2, 60_000_000, "truncating heap", 20, 6),
    ];

    let (mut episodes, at_timestamp) = build_episodes(rows, Some(limit)).expect("bounded episodes");
    assert_eq!(episodes.len(), 3);
    sort_episodes(&mut episodes, at_timestamp, &policies).expect("accepted ordering");
    assert_eq!(episodes[0].key.pid, 20);
    assert_eq!(episodes[1].key.pid, 10);
    let value = episode_value(
        &episodes[0],
        at_timestamp,
        &["pid".to_owned(), "phase".to_owned()],
        &policies,
    )
    .expect("episode summary");
    assert_eq!(value["observation"]["kind"], json!("at_sample"));
    assert_eq!(value["phase"]["risk"], json!("dangerous"));
    assert_eq!(value["phase"]["no_movement"]["samples"], json!(3));

    let regressed = vec![
        sample(30, 3, 10_000_000, "scanning heap", 20, 7),
        sample(30, 3, 20_000_000, "scanning heap", 19, 8),
    ];
    let (episodes, _at) = build_episodes(regressed, Some(limit)).expect("regression split");
    assert_eq!(episodes.len(), 2);
}

#[test]
fn sample_admission_refuses_more_than_the_complete_bound() {
    let rows = (0_u64..=500)
        .map(|ordinal| {
            sample(
                i32::try_from(ordinal).unwrap_or(0) + 1,
                u32::try_from(ordinal).unwrap_or(0) + 1,
                i64::try_from(ordinal).unwrap_or(0),
                "scanning heap",
                1,
                ordinal,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admit_samples(&rows).expect_err("oversized native set").code,
        "sample_bound_exceeded"
    );

    let entities = (0_u64..=256)
        .map(|ordinal| {
            sample(
                i32::try_from(ordinal).unwrap_or(0) + 1,
                u32::try_from(ordinal).unwrap_or(0) + 1,
                i64::try_from(ordinal).unwrap_or(0),
                "scanning heap",
                1,
                ordinal,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admit_samples(&entities)
            .expect_err("oversized entity set")
            .code,
        "entity_bound_exceeded"
    );
}

#[test]
fn trailing_cycle_and_layout_availability_bound_no_movement() {
    let policies = Policies::load().expect("accepted Vacuum policies");
    let mut rows = vec![
        sample(40, 4, 10_000_000, "vacuuming indexes", 10, 1),
        sample(40, 4, 20_000_000, "vacuuming indexes", 10, 2),
        sample(40, 4, 30_000_000, "vacuuming indexes", 10, 3),
    ];
    for row in &mut rows {
        row.type_id = 1_012_004;
        row.key.type_id = 1_012_004;
        row.row.insert("type_id".to_owned(), json!("1012004"));
        if let Some(values) = row.row.get_mut("values").and_then(Value::as_object_mut) {
            values.remove("indexes_processed");
        }
    }
    let (episodes, at_timestamp) = build_episodes(rows, None).expect("PG16 index episode");
    let value = episode_value(&episodes[0], at_timestamp, &["phase".to_owned()], &policies)
        .expect("PG16 episode summary");
    assert!(value["phase"]["no_movement"].is_null());

    let mut rows = vec![
        sample(50, 5, 10_000_000, "vacuuming indexes", 10, 4),
        sample(50, 5, 20_000_000, "vacuuming indexes", 10, 5),
        sample(50, 5, 30_000_000, "vacuuming indexes", 10, 6),
    ];
    if let Some(value) = rows[2]
        .row
        .get_mut("values")
        .and_then(Value::as_object_mut)
        .and_then(|values| values.get_mut("index_vacuum_count"))
    {
        *value = json!("1");
    }
    let (episodes, at_timestamp) = build_episodes(rows, None).expect("cycle transition episode");
    let value = episode_value(&episodes[0], at_timestamp, &["phase".to_owned()], &policies)
        .expect("cycle transition summary");
    assert_eq!(value["phase"]["sample_count"], json!(1));
    assert!(value["phase"]["no_movement"].is_null());
}

#[test]
fn handler_returns_recorded_episode_artifacts_and_semantic_provenance() {
    let mut fixture = Fixture::new();
    fixture.append_history();
    let arguments = json!({
        "from_us": FROM.to_string(),
        "to_us": TO.to_string(),
        "fields": ["pid", "phase", "indexes_processed"],
        "page_size": 10,
    });
    let payload = execute(
        &fixture.state(),
        arguments.as_object().expect("argument object"),
        &|| false,
    )
    .expect("Vacuum handler result");

    let episodes = payload.data["episodes"]
        .as_array()
        .expect("Vacuum episodes");
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[0]["identity"]["pid"], json!(200));
    assert_eq!(episodes[0]["phase"]["risk"], json!("dangerous"));
    assert_eq!(episodes[0]["observation"]["kind"], json!("at_sample"));
    assert_eq!(episodes[1]["identity"]["pid"], json!(100));
    assert_eq!(episodes[1]["phase"]["no_movement"]["samples"], json!(3));
    assert_eq!(
        episodes[1]["latest_row"]["segment_id"],
        json!(SEGMENT_ID.to_string())
    );
    assert_eq!(
        episodes[1]["latest_row"]["timestamp"],
        json!(TO.to_string())
    );
    assert_eq!(
        episodes[1]["latest_row"]["values"]["indexes_processed"],
        json!(1_i64.to_string())
    );
    assert_eq!(
        episodes[1]["sample_locators"].as_array().map(Vec::len),
        Some(3)
    );
    assert!(
        payload.data["semantics"]
            .as_array()
            .is_some_and(|semantics| semantics
                .iter()
                .any(|semantic| { semantic.get("id") == Some(&json!("vacuum.phase_risk")) }))
    );
    assert!(
        payload.data["semantics"]
            .as_array()
            .is_some_and(|semantics| semantics.iter().any(|semantic| {
                semantic.get("source") == Some(&json!("instance_metadata"))
                    && semantic.get("value") == Some(&json!("10"))
            }))
    );
    assert_eq!(payload.page["truncated"], json!(false));
    assert_eq!(payload.page["stop_reason"], json!("complete"));
}

fn sample(pid: i32, relid: u32, timestamp: i64, phase: &str, scanned: i64, ordinal: u64) -> Sample {
    let values = json!({
        "pid": pid,
        "datid": 16_384,
        "relid": relid,
        "phase": phase,
        "heap_blks_total": "100",
        "heap_blks_scanned": scanned.to_string(),
        "heap_blks_vacuumed": "0",
        "index_vacuum_count": "0",
        "indexes_processed": "0",
    });
    let row = json!({
        "record": "row",
        "logical_name": "pg_stat_progress_vacuum",
        "segment_id": "1",
        "type_id": "1012005",
        "ordinal": ordinal.to_string(),
        "timestamp": timestamp.to_string(),
        "values": values,
    });
    Sample {
        key: EpisodeKey {
            type_id: 1_012_005,
            pid,
            datid: 16_384,
            relid,
        },
        timestamp,
        segment_id: 1,
        type_id: 1_012_005,
        ordinal,
        row: row.as_object().cloned().unwrap_or_else(Map::new),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture spells out one exact recorded Vacuum identity and phase"
)]
fn vacuum_row(
    timestamp: i64,
    pid: i32,
    datid: u32,
    relid: u32,
    datname: StrId,
    schemaname: StrId,
    relname: StrId,
    phase: StrId,
    heap_blks_scanned: i64,
) -> PgStatProgressVacuumV2 {
    PgStatProgressVacuumV2 {
        ts: Ts(timestamp),
        pid,
        datid,
        datname,
        relid,
        schemaname: Some(schemaname),
        relname: Some(relname),
        is_autovacuum: true,
        phase,
        heap_blks_total: 1_000,
        heap_blks_scanned,
        heap_blks_vacuumed: 50,
        index_vacuum_count: 1,
        max_dead_tuple_bytes: 67_108_864,
        dead_tuple_bytes: 1_024,
        num_dead_item_ids: 20,
        indexes_total: 2,
        indexes_processed: 1,
    }
}

fn label(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern fixture label")
            .get(),
    )
}
