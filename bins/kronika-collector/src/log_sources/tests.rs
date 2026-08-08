use kronika_layout::{DataRoot, LayoutLimits, SegmentId};
use kronika_registry::Ts;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_source_log::pgbouncer::PgBouncerLog;
use kronika_source_log::{Offsets, Position};
use kronika_writer::{Journal, JournalConfig, SectionBuffers};

use crate::scheduler::{DueSet, SourceKind};

use super::{
    LogSources, MAX_READ_BYTES, MAX_SOURCE_READ_BYTES, key, next_batch_bytes, parse_connections,
};

fn pgbouncer_line(message: &str) -> String {
    format!("2026-08-07 12:34:56.789 MSK [12345] ERROR {message}\n")
}

fn sources(root: &std::path::Path, path: std::path::PathBuf) -> LogSources {
    LogSources {
        offsets: Offsets::load(root).expect("load offsets"),
        pg_dsns: Vec::new(),
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
        postgres: Vec::new(),
        pgbouncer: vec![PgBouncerLog::new(path, Position::default())],
        next_scan: None,
    }
}

fn one_wal_part() -> Vec<u8> {
    let mut buffers = SectionBuffers::new();
    buffers
        .push(OsLoadavg {
            ts: Ts(1),
            load1: 1.0,
            load5: 1.0,
            load15: 1.0,
            running: 1,
            total: 1,
            scope: 0,
        })
        .expect("buffer one row");
    buffers
        .flush(&[])
        .expect("encode one row")
        .expect("one row yields a part")
}

#[test]
fn configured_connections_retain_no_raw_dsn_or_secret() {
    let raw = "postgresql://monitor:RAW_SECRET@db.example:6432/PRIVATE_DATABASE";
    let configured = vec![raw.to_owned()];

    let parsed = parse_connections("postgresql", &configured);

    assert_eq!(parsed.len(), 1);
    let debug = format!("{:?}", parsed[0]);
    assert!(debug.contains("monitor@db.example:6432"));
    for secret in [raw, "RAW_SECRET", "PRIVATE_DATABASE"] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn each_file_gets_an_independent_256_mib_budget_in_4_mib_batches() {
    fn consume_budget() -> (usize, usize) {
        let mut read = 0_usize;
        let mut batches = 0_usize;
        while next_batch_bytes(read) != 0 {
            let batch = next_batch_bytes(read);
            assert!(batch <= MAX_READ_BYTES);
            read += batch;
            batches += 1;
        }
        (read, batches)
    }

    assert_eq!(consume_budget(), (MAX_SOURCE_READ_BYTES, 64));
    assert_eq!(consume_budget(), (MAX_SOURCE_READ_BYTES, 64));
    assert_eq!(
        next_batch_bytes(MAX_SOURCE_READ_BYTES - 1),
        0,
        "a full batch is not started when it could cross the ceiling"
    );
}

#[test]
fn wal_append_precedes_offset_ack_and_a_retry_replays_the_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    let first = pgbouncer_line("kernel file descriptor limit: 1024");
    std::fs::write(
        &path,
        format!("{first}{}", pgbouncer_line("unrecognized sentinel")),
    )
    .expect("write log");
    let due = DueSet::for_test(vec![SourceKind::Logs]);
    let mut sources = sources(dir.path(), path.clone());

    let root = DataRoot::open(dir.path()).expect("open data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let body = one_wal_part();
    let completed = sources
        .collect(&due, 0, |rows| {
            assert_eq!(rows.pgbouncer[0].events.len(), 1);
            journal
                .append(SegmentId::new(1).expect("segment id"), &body)
                .expect("append and sync WAL");
            Ok(false)
        })
        .expect("recoverable downstream failure");

    assert!(!completed);
    assert_eq!(journal.parts().len(), 1, "the WAL append is durable");
    assert_eq!(sources.pgbouncer[0].position().offset, 0);
    assert_eq!(
        Offsets::load(dir.path())
            .expect("reload offsets")
            .get(&key(&path))
            .offset,
        0
    );

    let mut replayed = Vec::new();
    assert!(
        sources
            .collect(&due, 0, |rows| {
                replayed.push(rows.pgbouncer[0].events[0].text.clone());
                Ok(true)
            })
            .expect("retry succeeds")
    );
    assert_eq!(replayed, ["kernel file descriptor limit: 1024"]);
    let committed = sources.pgbouncer[0].position();
    assert_eq!(committed.offset, first.len() as u64);
    assert_eq!(
        Offsets::load(dir.path())
            .expect("reload committed offsets")
            .get(&key(&path)),
        committed
    );
}
