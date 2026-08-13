use std::path::Path;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_reader::{Reader, SegmentKind, SegmentRef};
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_topology::OsTopology;
use kronika_registry::pg_log::{
    PgLogAutovacuum, PgLogCheckpoints, PgLogErrors, PgLogLifecycle, PgLogLockWaits,
    PgLogSlowQueries, PgLogTempFiles,
};
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};

use crate::{BuildError, FindingKind, LoadError, SeriesBlock};

use super::{finding_keys, path_of, read, resource, series_keys};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;

fn address() -> SegmentAddress {
    address_at(SEGMENT_ID)
}

fn address_at(raw: i64) -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(raw).expect("segment id")).expect("address")
}

fn row(ts: i64, model_name: StrId, mhz: f64) -> OsTopology {
    OsTopology {
        ts: Ts(ts),
        cpu_id: 7,
        model_name,
        mhz_max: Some(mhz),
        core_id: 3,
        socket_id: 1,
        numa_node: 0,
        scope: 0,
    }
}

fn append_fixture(journal: &mut Journal) {
    let mut interner = Interner::new(DictLimits::default());
    let model_name = StrId(
        interner
            .intern(b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL")
            .expect("intern label")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(row(100, model_name, 2_000.0))
        .expect("first row");
    buffers
        .push(row(200, model_name, 2_500.0))
        .expect("second row");
    let part = buffers
        .flush(&dictionary)
        .expect("encode part")
        .expect("nonempty part");
    journal.append(address().id, &part).expect("append fixture");
}

fn process_row(ts: i64, read_bytes: Option<i64>, label: StrId) -> OsProcess {
    OsProcess {
        ts: Ts(ts),
        pid: 41,
        starttime: Ts(SEGMENT_ID - 1_000_000),
        ppid: 1,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'S',
        num_threads: 1,
        tty: 0,
        comm: label,
        cmdline: Some(label),
        utime: 0,
        stime: 0,
        nice: 0,
        prio: 20,
        rtprio: 0,
        policy: 0,
        curcpu: 0,
        rundelay_ns: 0,
        blkdelay_ticks: 0,
        nvcsw: 0,
        nivcsw: 0,
        minflt: 0,
        majflt: 0,
        vmem_kb: 0,
        rmem_kb: 0,
        vswap_kb: 0,
        syscr: None,
        syscw: None,
        rchar: None,
        wchar: None,
        read_bytes,
        write_bytes: None,
        cancelled_write_bytes: None,
        exit_signal: 17,
        scope: 0,
    }
}

fn statement_row(ts: i64, calls: i64, total_exec_time: f64, label: StrId) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(ts),
        queryid: Some(71),
        userid: 72,
        dbid: 73,
        datname: None,
        usename: None,
        query: Some(label),
        calls,
        rows: 0,
        plans: 0,
        total_exec_time,
        total_plan_time: 0.0,
        min_exec_time: 0.0,
        max_exec_time: 0.0,
        mean_exec_time: 0.0,
        stddev_exec_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
        stddev_plan_time: 0.0,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        wal_records: 0,
        wal_fpi: 0,
        wal_bytes: 0,
    }
}

fn append_finding_fixture(
    journal: &mut Journal,
    segment_id: i64,
    process_rows: &[(i64, Option<i64>)],
    statement_rows: &[(i64, i64, f64)],
) {
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(
        interner
            .intern(b"FINDING-SOURCE-TEXT-MUST-STAY-IN-ZMS")
            .expect("intern source text")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    for &(ts, read_bytes) in process_rows {
        buffers
            .push(process_row(ts, read_bytes, label))
            .expect("process row");
    }
    for &(ts, calls, total_exec_time) in statement_rows {
        buffers
            .push(statement_row(ts, calls, total_exec_time, label))
            .expect("statement row");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode finding fixture")
        .expect("nonempty finding fixture");
    journal
        .append(
            SegmentId::new(segment_id).expect("fixture segment id"),
            &part,
        )
        .expect("append finding fixture");
}

fn cpu_row(ts: i64, cpu_id: i32, user: i64, idle: i64, scope: u8) -> OsCpu {
    OsCpu {
        ts: Ts(ts),
        cpu_id,
        user,
        nice: 0,
        system: 0,
        idle,
        iowait: 0,
        irq: 0,
        softirq: 0,
        steal: 0,
        guest: 0,
        guest_nice: 0,
        scope,
    }
}

fn append_log_event_rows(
    buffers: &mut SectionBuffers,
    timestamp: i64,
    label: StrId,
    error_category: u8,
) {
    append_log_event_rows_2_001_to_2_003(buffers, timestamp, label, error_category);
    append_log_event_rows_2_004_to_2_007(buffers, timestamp, label);
}

fn append_log_event_rows_2_001_to_2_003(
    buffers: &mut SectionBuffers,
    timestamp: i64,
    label: StrId,
    error_category: u8,
) {
    buffers
        .push(PgLogErrors {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            severity: 0,
            category: error_category,
            sqlstate: Some(label),
            pattern: label,
            count: 1,
            sample: label,
            detail: Some(label),
            hint: Some(label),
            context: Some(label),
            statement: Some(label),
            database: Some(label),
            username: Some(label),
        })
        .expect("error row");
    buffers
        .push(PgLogCheckpoints {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            phase: 0,
            reason: Some(label),
            seconds_apart: None,
            buffers_written: None,
            write_ms: None,
            sync_ms: None,
            total_ms: None,
            distance_kb: None,
            estimate_kb: None,
            wal_added: None,
            wal_removed: None,
            wal_recycled: None,
            sync_files: None,
            longest_sync_ms: None,
            average_sync_ms: None,
        })
        .expect("checkpoint row");
    buffers
        .push(PgLogAutovacuum {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            kind: 0,
            relation: Some(label),
            index_scans: None,
            pages_removed: None,
            pages_remaining: None,
            tuples_removed: None,
            tuples_remaining: None,
            tuples_dead_not_removable: None,
            elapsed_ms: None,
            buffer_hits: None,
            buffer_misses: None,
            buffer_dirtied: None,
            avg_read_rate_mbs: None,
            avg_write_rate_mbs: None,
            cpu_user_ms: None,
            cpu_system_ms: None,
            wal_records: None,
            wal_fpi: None,
            wal_bytes: None,
        })
        .expect("autovacuum row");
}

fn append_log_event_rows_2_004_to_2_007(
    buffers: &mut SectionBuffers,
    timestamp: i64,
    label: StrId,
) {
    buffers
        .push(PgLogSlowQueries {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            pattern: label,
            sample: label,
            count: 3,
            max_duration_ms: 5_000.0,
            total_duration_ms: 99_999.0,
        })
        .expect("slow query row");
    buffers
        .push(PgLogLockWaits {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            kind: 0,
            pid: Some(41),
            lock_mode: Some(label),
            lock_target: Some(label),
            duration_ms: Some(1.0),
            detail: Some(label),
            context: Some(label),
            statement: Some(label),
        })
        .expect("lock wait row");
    buffers
        .push(PgLogLifecycle {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            kind: 0,
            pid: Some(41),
            signal: Some(9),
            shutdown_mode: Some(label),
            message: label,
            query_detail: Some(label),
        })
        .expect("lifecycle row");
    buffers
        .push(PgLogTempFiles {
            ts: Ts(timestamp),
            system_identifier: None,
            source_file: label,
            path: Some(label),
            size_bytes: 1,
            statement: Some(label),
        })
        .expect("temporary-file row");
}

fn append_direct_fixture(journal: &mut Journal, segment_id: i64, error_category: u8) {
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(
        interner
            .intern(b"EVENT-SOURCE-MESSAGE-QUERY-STATEMENT-MUST-STAY-IN-ZMS")
            .expect("intern direct source text")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    let later = segment_id + 1_000_000;
    for row in [
        cpu_row(segment_id, -1, 0, 0, 0),
        cpu_row(later, -1, 80, 20, 0),
        cpu_row(later, 0, 1, 1, 0),
        cpu_row(later, 1, 1, 1, 0),
        cpu_row(later, 2, 1, 1, 1),
    ] {
        buffers.push(row).expect("CPU row");
    }
    buffers
        .push(OsLoadavg {
            ts: Ts(later),
            load1: 4.0,
            load5: 0.0,
            load15: 0.0,
            running: 1,
            total: 1,
            scope: 0,
        })
        .expect("load row");
    buffers
        .push(OsMountinfo {
            ts: Ts(later),
            major: 8,
            minor: 1,
            mount_point: label,
            fstype: label,
            source: label,
            is_k8s_infra: false,
            total_bytes: Some(100),
            free_bytes: Some(10),
            scope: 0,
        })
        .expect("mount row");
    append_log_event_rows(&mut buffers, later, label, error_category);
    let part = buffers
        .flush(&dictionary)
        .expect("encode direct fixture")
        .expect("nonempty direct fixture");
    journal
        .append(
            SegmentId::new(segment_id).expect("direct segment id"),
            &part,
        )
        .expect("append direct fixture");
}

fn only_segment(reader: &Reader, kind: SegmentKind) -> SegmentRef {
    let listing = reader.catalog_segments(..).expect("list fixture");
    let segments: Vec<_> = listing
        .segments
        .into_iter()
        .filter(|segment| segment.kind() == kind)
        .collect();
    assert_eq!(segments.len(), 1, "one segment of requested kind");
    segments.into_iter().next().expect("one segment")
}

#[test]
fn an_index_lives_beside_its_finished_segment() {
    assert_eq!(
        path_of(Path::new("/data/2026/08/08/17.zms")),
        Some(Path::new("/data/2026/08/08/17.idx").to_path_buf())
    );
}

#[test]
fn active_data_never_gets_an_index_path() {
    assert_eq!(path_of(Path::new("/data/active.wal")), None);
}

#[test]
fn real_active_and_finished_resources_are_bounded_and_atomically_cached() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_fixture(&mut journal);

    let reader = Reader::open(directory.path()).expect("reader");
    let active_ref = only_segment(&reader, SegmentKind::Active);
    let active =
        resource(directory.path(), &reader, &active_ref, "health").expect("active resource");
    assert!(!active.persisted);
    assert_eq!(active.index.checksum, None);
    assert_eq!(active.index.blocks.len(), 3);

    write_segment(&journal, &writer, address()).expect("finish segment");
    let reader = Reader::open(directory.path()).expect("finished reader");
    let finished_ref = only_segment(&reader, SegmentKind::Finished);
    let index_path = path_of(reader.open_segment(&finished_ref).expect("segment").path())
        .expect("finished index path");

    let contended_owner = data_root
        .acquire_index(LayoutLimits::default())
        .expect("hold index owner");
    let (release, held) = std::sync::mpsc::sync_channel(0);
    let local = std::thread::scope(|scope| {
        scope.spawn(move || {
            held.recv_timeout(std::time::Duration::from_secs(2))
                .expect("resource returns before the bounded holder timeout");
            drop(contended_owner);
        });
        let local = resource(directory.path(), &reader, &finished_ref, "health")
            .expect("answer without waiting out the holder");
        release.send(()).expect("release index owner");
        local
    });
    assert!(!local.persisted);
    assert!(!index_path.is_file());
    let published = resource(directory.path(), &reader, &finished_ref, "health")
        .expect("publish after the holder releases");
    assert!(published.persisted);
    assert!(index_path.is_file());
    let bytes = std::fs::read(&index_path).expect("read index bytes");
    assert!(
        !bytes
            .windows(b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL".len())
            .any(|window| window == b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL"),
        "non-identity display labels do not belong in IDX"
    );
    assert_eq!(read(&index_path).expect("read index").blocks.len(), 3);
}

#[test]
fn a_published_index_does_not_require_its_finished_source_body() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_fixture(&mut journal);
    write_segment(&journal, &writer, address()).expect("finish segment");

    let reader = Reader::open(directory.path()).expect("finished reader");
    let finished_ref = only_segment(&reader, SegmentKind::Finished);
    let source_path = reader
        .open_segment(&finished_ref)
        .expect("open finished segment")
        .path()
        .to_path_buf();
    let published = resource(directory.path(), &reader, &finished_ref, "health")
        .expect("publish finished index");
    assert!(published.persisted);
    journal.reset().expect("leave no active segment");

    let mut bytes = std::fs::read(&source_path).expect("read source segment");
    bytes[kronika_format::MAGIC.len()] ^= 0xff;
    std::fs::write(&source_path, bytes).expect("damage source section body");

    let reader = Reader::open(directory.path()).expect("reader after damage");
    let validated = reader.segments(..).expect("validate source bodies");
    assert!(validated.segments.is_empty());
    assert_eq!(validated.warnings.len(), 1);
    assert_eq!(
        validated.warnings[0].reason.code(),
        "invalid_zms_section_checksum"
    );

    let catalog = reader.catalog_segments(..).expect("catalog-only discovery");
    assert!(catalog.warnings.is_empty());
    assert_eq!(catalog.segments.len(), 1);
    let recovered = resource(directory.path(), &reader, &catalog.segments[0], "health")
        .expect("load published index without source body");
    assert_eq!(recovered, published);
}

#[test]
fn truncated_and_unknown_finished_indexes_are_rebuilt_in_place() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_direct_fixture(&mut journal, SEGMENT_ID, 5);
    write_segment(&journal, &writer, address()).expect("finish segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let finished = only_segment(&reader, SegmentKind::Finished);
    let published = resource(directory.path(), &reader, &finished, "pg_log_errors")
        .expect("publish current index");
    let path = path_of(
        reader
            .open_segment(&finished)
            .expect("finished segment")
            .path(),
    )
    .expect("index path");
    let canonical = std::fs::read(&path).expect("canonical index");

    std::fs::write(&path, &canonical[..10]).expect("truncate derived index");
    let rebuilt = resource(directory.path(), &reader, &finished, "pg_log_errors")
        .expect("rebuild truncated index");
    assert_eq!(rebuilt, published);
    assert_eq!(std::fs::read(&path).expect("rebuilt index"), canonical);

    let mut unknown = canonical.clone();
    unknown[0] ^= 1;
    std::fs::write(&path, unknown).expect("replace index magic");
    let rebuilt = resource(directory.path(), &reader, &finished, "pg_log_errors")
        .expect("rebuild unknown index");
    assert_eq!(rebuilt, published);
    assert_eq!(
        std::fs::read(path).expect("rebuilt current index"),
        canonical
    );
}

#[test]
fn direct_boundaries_and_log_events_use_exact_production_fields() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_direct_fixture(&mut journal, SEGMENT_ID, 5);
    write_segment(&journal, &writer, address()).expect("finish direct segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let segment = only_segment(&reader, SegmentKind::Finished);
    for (logical_name, type_id, field_ordinal, row_ordinal) in [
        ("os_cpu", 1_102_001, 5, 1),
        ("os_loadavg", 1_105_001, 1, 0),
        ("os_mountinfo", 1_112_001, 8, 0),
    ] {
        let selected = resource(directory.path(), &reader, &segment, logical_name)
            .expect("direct finding resource");
        let [SeriesBlock::Findings(block)] = selected.index.blocks.as_slice() else {
            panic!("one direct finding block for {logical_name}");
        };
        assert_eq!(block.type_id, type_id);
        assert_eq!(block.total_hits, 1);
        assert!(!block.truncated);
        assert_eq!(block.findings.len(), 1);
        assert_eq!(block.findings[0].kind, FindingKind::KnownBad);
        assert_eq!(block.findings[0].category, None);
        assert_eq!(block.findings[0].field_ordinal, field_ordinal);
        assert_eq!(block.findings[0].row_ordinal, row_ordinal);
        assert_eq!(block.findings[0].timestamp, SEGMENT_ID + 1_000_000);
    }

    for (logical_name, type_id) in [
        ("pg_log_errors", 2_001_001),
        ("pg_log_checkpoints", 2_002_001),
        ("pg_log_autovacuum", 2_003_001),
        ("pg_log_slow_queries", 2_004_001),
        ("pg_log_lock_waits", 2_005_001),
        ("pg_log_lifecycle", 2_006_001),
    ] {
        let selected = resource(directory.path(), &reader, &segment, logical_name)
            .expect("log event resource");
        let [SeriesBlock::Findings(block)] = selected.index.blocks.as_slice() else {
            panic!("one event locator block for {logical_name}");
        };
        assert_eq!(block.type_id, type_id);
        let expected_hits = if type_id == 2_004_001 { 2 } else { 1 };
        assert_eq!(block.total_hits, expected_hits);
        assert!(!block.truncated);
        assert_eq!(block.findings.len(), expected_hits as usize);
        assert_eq!(block.findings[0].kind, FindingKind::Event);
        assert_eq!(
            block.findings[0].category,
            (type_id == 2_001_001).then_some(5)
        );
        assert_eq!(block.findings[0].field_ordinal, 0);
        assert_eq!(block.findings[0].row_ordinal, 0);
        assert_eq!(block.findings[0].timestamp, SEGMENT_ID + 1_000_000);
        if type_id == 2_004_001 {
            assert_eq!(block.findings[1].kind, FindingKind::KnownBad);
            assert_eq!(block.findings[1].category, None);
            assert_eq!(block.findings[1].field_ordinal, 6);
            assert_eq!(block.findings[1].row_ordinal, 0);
            assert_eq!(block.findings[1].timestamp, SEGMENT_ID + 1_000_000);
        }
    }

    let raw = reader.open_segment(&segment).expect("finished segment");
    assert_eq!(raw.rows_of(2_007_001), Some(1));
    assert!(
        finding_keys(&segment)
            .iter()
            .all(|key| key.type_id != 2_007_001)
    );
    assert!(series_keys(&segment, "pg_log_temp_files").is_empty());
    let selected = resource(directory.path(), &reader, &segment, "pg_log_temp_files")
        .expect("raw temporary-file section has no index resource");
    assert!(selected.index.blocks.is_empty());

    let path = path_of(
        reader
            .open_segment(&segment)
            .expect("finished segment")
            .path(),
    )
    .expect("index path");
    assert!(
        read(&path).expect("current index").blocks.iter().all(
            |block| !matches!(block, SeriesBlock::Findings(block) if block.type_id == 2_007_001)
        )
    );
    let bytes = std::fs::read(path).expect("published index");
    let copied = b"EVENT-SOURCE-MESSAGE-QUERY-STATEMENT-MUST-STAY-IN-ZMS";
    assert!(
        !bytes.windows(copied.len()).any(|window| window == copied),
        "event locators contain no source row text"
    );
}

#[test]
fn production_builder_rejects_an_invalid_log_error_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_direct_fixture(&mut journal, SEGMENT_ID, 11);
    write_segment(&journal, &writer, address()).expect("finish direct segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let segment = only_segment(&reader, SegmentKind::Finished);
    let error = match resource(directory.path(), &reader, &segment, "pg_log_errors") {
        Ok(_resource) => panic!("invalid category must fail the build"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        LoadError::Build(BuildError::InvalidLogErrorCategory)
    ));
}

#[test]
fn slow_process_and_statement_spikes_cross_finished_segment_boundaries() {
    const STEP: i64 = 5 * 60 * 1_000_000;

    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");

    let process_prior: Vec<_> = (0_i64..6)
        .map(|at| (SEGMENT_ID + at * STEP, Some(at * 300)))
        .collect();
    let statement_prior: Vec<_> = (0_i32..6)
        .map(|at| {
            (
                SEGMENT_ID + i64::from(at) * STEP,
                i64::from(at),
                f64::from(at) * 100.0,
            )
        })
        .collect();
    append_finding_fixture(&mut journal, SEGMENT_ID, &process_prior, &statement_prior);
    write_segment(&journal, &writer, address_at(SEGMENT_ID)).expect("finish prior segment");
    journal.reset().expect("reset after prior segment");

    let spike_ts = SEGMENT_ID + 6 * STEP;
    let process_current = [
        (spike_ts, Some(301_500)),
        (spike_ts + STEP, None),
        (spike_ts + 2 * STEP, Some(301_800)),
        (spike_ts + 3 * STEP, Some(100)),
    ];
    let statement_current = [
        (spike_ts, 6, 10_500.0),
        (spike_ts + STEP, 0, 0.0),
        (spike_ts + 2 * STEP, 1, 100.0),
    ];
    append_finding_fixture(
        &mut journal,
        SEGMENT_ID + 1,
        &process_current,
        &statement_current,
    );
    write_segment(&journal, &writer, address_at(SEGMENT_ID + 1)).expect("finish current segment");
    journal.reset().expect("leave no active segment");

    let reader = Reader::open(directory.path()).expect("reader");
    let listing = reader.catalog_segments(..).expect("catalog segments");
    let current = listing
        .segments
        .into_iter()
        .find(|segment| segment.id() == SEGMENT_ID + 1)
        .expect("current finished segment");
    let process =
        resource(directory.path(), &reader, &current, "os_process").expect("process findings");
    let statements = resource(directory.path(), &reader, &current, "pg_stat_statements")
        .expect("statement findings");
    assert!(process.persisted);
    assert!(statements.persisted);
    assert_eq!(process.index.checksum, statements.index.checksum);

    let [SeriesBlock::Findings(process)] = process.index.blocks.as_slice() else {
        panic!("one process finding block");
    };
    assert_eq!(process.type_id, 1_100_001);
    assert_eq!(process.total_hits, 1);
    assert_eq!(process.findings.len(), 1);
    assert_eq!(process.findings[0].kind, FindingKind::Spike);
    assert_eq!(process.findings[0].field_ordinal, 33);
    assert_eq!(process.findings[0].row_ordinal, 0);
    assert_eq!(process.findings[0].timestamp, spike_ts);

    let [SeriesBlock::Findings(statements)] = statements.index.blocks.as_slice() else {
        panic!("one statement finding block");
    };
    assert_eq!(statements.type_id, 1_002_002);
    assert_eq!(statements.total_hits, 1);
    assert_eq!(statements.findings.len(), 1);
    assert_eq!(statements.findings[0].kind, FindingKind::Spike);
    assert_eq!(statements.findings[0].field_ordinal, 10);
    assert_eq!(statements.findings[0].row_ordinal, 0);
    assert_eq!(statements.findings[0].timestamp, spike_ts);

    let index_path = path_of(
        reader
            .open_segment(&current)
            .expect("open current segment")
            .path(),
    )
    .expect("finished index path");
    let bytes = std::fs::read(index_path).expect("read published index");
    assert!(
        !bytes
            .windows(b"FINDING-SOURCE-TEXT-MUST-STAY-IN-ZMS".len())
            .any(|window| window == b"FINDING-SOURCE-TEXT-MUST-STAY-IN-ZMS"),
        "finding blocks contain locators, not source text"
    );
}
