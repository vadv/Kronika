use std::path::Path;

use kronika_format::validate_part;
use kronika_layout::{DataRoot, LayoutLimits, WriterOwner};
use kronika_registry::Ts;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_source_log::pgbouncer::{Event, Level};
use kronika_writer::{Journal, JournalConfig, SectionBuffers};

use crate::config::Config;
use crate::log_sources::{LogRows, PgBouncerBatch};
use crate::pg_sources::PgRows;
use crate::scheduler::{DueSet, Intervals};
use crate::segments::{SegmentState, append_window_and_maybe_close, encode_window};
use crate::{append_pending_window, scheduler::Scheduler};

const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_001;
const PGBOUNCER_TYPE_ID: u32 = 2_100_001;

#[derive(Clone, Copy)]
enum Pressure {
    Format,
    Journal,
}

fn config(root: &Path, journal_max_bytes: u64) -> Config {
    Config {
        out_dir: root.to_path_buf(),
        tick_secs: 1,
        intervals: Intervals::default(),
        segment_max_bytes: u64::MAX,
        segment_max_age_secs: u64::MAX,
        journal_max_bytes,
        retention: None,
        pg_dsns: Vec::new(),
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
    }
}

fn owner(root: &Path) -> WriterOwner {
    DataRoot::open(root)
        .expect("open data root")
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer")
}

fn first_window(segment: &SegmentState) -> kronika_writer::FlushedPart {
    let mut buffers = SectionBuffers::new();
    buffers
        .push(OsLoadavg {
            ts: Ts(100),
            load1: 1.0,
            load5: 1.0,
            load15: 1.0,
            running: 1,
            total: 2,
            scope: 0,
        })
        .expect("buffer first row");
    encode_window(buffers, segment.interner()).expect("encode first window")
}

fn log_rows() -> LogRows {
    LogRows {
        postgres: Vec::new(),
        pgbouncer: vec![PgBouncerBatch {
            source_file: "/var/log/pgbouncer.log".to_owned(),
            events: vec![Event {
                ts: 200,
                level: Level::Error,
                database: Some("shop".to_owned()),
                username: Some("monitor".to_owned()),
                host: Some("127.0.0.1".to_owned()),
                text: "kernel file descriptor limit: 1024".to_owned(),
            }],
        }],
    }
}

fn assert_retained_batch_moves_to_fresh_segment(pressure: Pressure) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut segment = SegmentState::default();
    let first = first_window(&segment);
    let max = match pressure {
        Pressure::Format => JournalConfig::default().max_journal_len,
        Pressure::Journal => 16 * 1024,
    };
    let owner = owner(dir.path());
    let mut journal = Journal::open(
        &owner,
        JournalConfig {
            max_journal_len: max,
            ..JournalConfig::default()
        },
    )
    .expect("open journal");
    let config = config(dir.path(), max as u64);
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append old segment row");
    match pressure {
        Pressure::Format => segment.force_format_limit(),
        Pressure::Journal => {
            let segment_id = journal.segment_id().expect("old segment id");
            for _ in 0..7 {
                journal
                    .append(segment_id, &first.body)
                    .expect("fill old journal near its cap");
            }
        }
    }

    let mut scheduler = Scheduler::new(Intervals::default());
    let outcome = append_pending_window(
        &mut journal,
        &owner,
        &config,
        &DueSet::logs(),
        &log_rows(),
        &PgRows::default(),
        &[],
        200,
        &mut segment,
        &mut scheduler,
    )
    .expect("retain and append log batch");

    assert!(outcome.accepted);
    assert!(outcome.appended);
    assert_eq!(outcome.written.len(), 1, "the old segment closes once");
    let old = std::fs::read(&outcome.written[0]).expect("read old segment");
    let old_catalog = validate_part(&old).expect("old segment is valid");
    assert!(
        old_catalog
            .entries
            .iter()
            .all(|entry| entry.type_id != PGBOUNCER_TYPE_ID),
        "the pending row was not written to the old segment"
    );

    assert_eq!(journal.parts().len(), 1, "one retained batch is appended");
    let fresh = journal
        .read_part(journal.parts()[0])
        .expect("read fresh WAL part");
    let fresh_catalog = validate_part(&fresh).expect("fresh part and dictionary are valid");
    assert_eq!(
        fresh_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == PGBOUNCER_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1)
    );
    assert_eq!(
        fresh_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == INSTANCE_METADATA_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1),
        "a fresh segment gets exactly one metadata row"
    );
}

#[test]
fn format_limit_retains_and_reencodes_the_exact_log_batch() {
    assert_retained_batch_moves_to_fresh_segment(Pressure::Format);
}

#[test]
fn journal_full_retains_and_reencodes_the_exact_log_batch() {
    assert_retained_batch_moves_to_fresh_segment(Pressure::Journal);
}
