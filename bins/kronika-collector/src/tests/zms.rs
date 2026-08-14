use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kronika_format::{ENTRY_LEN, FRAME_HEADER_LEN};
use kronika_layout::{DataRoot, LayoutLimits, WriterOwner};
use kronika_reader::{Cell, Reader, Resolved, SegmentKind};
use kronika_registry::os_cgroup_context::OsCgroupContext;
use kronika_registry::{PgWalStorage, StrId, Ts, section_name};
use kronika_source_pg::query::{BATCH_LOGICAL_BYTES, BATCH_ROWS};
use kronika_source_pg::settings::SettingsRow;
use kronika_source_pg::statements::{StatementsRow, StatementsVersion};
use kronika_source_pg::store_plans::VadvRow;
use kronika_writer::{Journal, JournalConfig, SectionBuffers};

use crate::config::Config;
use crate::logging::peak_rss_kib;
use crate::os_sources::{OsSources, push_os_sources};
use crate::pg_sources::{PgBatch, push_pg_batch};
use crate::scheduler::Intervals;
use crate::segments::{
    SegmentState, append_window_and_maybe_close, close_open_segment, encode_window,
};

const STATEMENT_BATCH_COUNT: usize = 20;
const BASE_QUERY_COUNT: usize = 160;
const UNIQUE_QUERY_COUNT: usize = 40;
const QUERY_BYTES: usize = 2_048;
const STATEMENT_ROW_OVERHEAD_BOUND: usize = 512;
const PLAN_BATCH_COUNT: usize = 3;
const PLAN_ROWS_PER_BATCH: usize = 120;
const PLAN_BYTES: usize = 4_096;
const PLAN_ROW_OVERHEAD_BYTES: usize = 274;
const PG_STAT_STATEMENTS_V6_TYPE_ID: u32 = 1_002_006;
const PG_STORE_PLANS_VADV_TYPE_ID: u32 = 1_004_001;
const PG_WAL_STORAGE_TYPE_ID: u32 = 1_020_001;
const CGROUP_CONTEXT_TYPE_ID: u32 = 1_205_001;
const WAL_STORAGE_SNAPSHOTS_PER_HOUR: usize = 120;
const BASE_TS: i64 = 1_700_000_000_000_000;
const PRE_CHANGE_ZMS_BYTES: u64 = 3_141_820;
const PRE_CHANGE_DICT_STRINGS_BYTES: u64 = 1_978_136;
const MAX_REPLAY_ZMS_BYTES: u64 = 2_800_000;
const MAX_REPLAY_WAL_BYTES: u64 = 3_400_000;
const MAX_REPLAY_DICT_STRINGS_BYTES: u64 = 1_600_000;
const MAX_REPLAY_DICT_BLOBS_BYTES: u64 = 1_200_000;
const QUERY_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";

#[derive(Debug, Default)]
struct ReplayWriteReport {
    paths: Vec<PathBuf>,
    close_reasons: BTreeMap<&'static str, usize>,
    cumulative_appended_wal_bytes: u64,
    peak_wal_bytes: usize,
    appended_windows: usize,
    max_batch_rows: usize,
    max_batch_logical_bytes: usize,
}

#[derive(Debug, Default)]
struct SectionReport {
    rows: u64,
    bytes: u64,
}

struct ReplayAppend<'a> {
    journal: &'a mut Journal,
    writer: &'a WriterOwner,
    config: &'a Config,
    segment: &'a mut SegmentState,
    settings: &'a [SettingsRow],
    report: &'a mut ReplayWriteReport,
}

struct ReplayArtifactReport {
    segments: usize,
    zms_bytes: u64,
    section_body_bytes: u64,
    overhead_bytes: u64,
    windows: u64,
    statement_rows: usize,
    plan_rows: usize,
    rss_kib: u64,
    sections: BTreeMap<&'static str, SectionReport>,
}

fn config(root: &Path, journal_max_bytes: u64) -> Config {
    Config {
        out_dir: root.to_path_buf(),
        tick_secs: 1,
        intervals: Intervals::default(),
        segment_max_bytes: 64 * 1024 * 1024,
        segment_max_age_secs: u64::MAX,
        journal_max_bytes,
        retention: None,
        pg_dsns: Vec::new(),
        postgres_effective_cpus: None,
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
    }
}

fn owner(root: &Path) -> WriterOwner {
    DataRoot::open(root)
        .expect("open replay data root")
        .acquire_writer(LayoutLimits::default())
        .expect("acquire replay writer")
}

fn settings_row() -> SettingsRow {
    SettingsRow {
        ts: BASE_TS,
        datid: 16_384,
        datname: "app".to_owned(),
        usesysid: 16_385,
        usename: "monitor".to_owned(),
        name: "shared_buffers".to_owned(),
        setting: "16384".to_owned(),
        unit: Some("8kB".to_owned()),
        source: "default".to_owned(),
        sourcefile: None,
        sourceline: None,
        pending_restart: false,
        context: "postmaster".to_owned(),
        vartype: "integer".to_owned(),
        boot_val: Some("16384".to_owned()),
        reset_val: Some("16384".to_owned()),
    }
}

fn query_text(index: usize) -> String {
    let mut state = u64::try_from(index)
        .unwrap_or(u64::MAX)
        .wrapping_add(1)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut bytes = Vec::with_capacity(QUERY_BYTES);
    bytes.extend_from_slice(b"select '");
    while bytes.len() + 1 < QUERY_BYTES {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let random = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        bytes.push(QUERY_ALPHABET[(random & 63) as usize]);
    }
    bytes.push(b'\'');
    String::from_utf8(bytes).expect("query alphabet is ASCII")
}

fn statement_row(batch_index: usize, query_index: usize) -> StatementsRow {
    let sample = i64::try_from(batch_index).unwrap_or(i64::MAX);
    let sample_f64 = f64::from(u32::try_from(batch_index).unwrap_or(u32::MAX));
    let query_id = i64::try_from(query_index)
        .unwrap_or(i64::MAX)
        .saturating_add(1);
    let ts = BASE_TS.saturating_add(sample);
    StatementsRow {
        ts,
        queryid: Some(query_id),
        userid: 16_385,
        dbid: 16_384,
        toplevel: Some(true),
        datname: Some("app".to_owned()),
        usename: Some("monitor".to_owned()),
        query: Some(query_text(query_index)),
        calls: sample.saturating_add(1),
        rows: query_id,
        plans: Some(sample),
        total_exec_time: sample_f64,
        total_plan_time: Some(0.0),
        min_exec_time: 0.0,
        max_exec_time: sample_f64,
        mean_exec_time: 0.0,
        stddev_exec_time: 0.0,
        min_plan_time: Some(0.0),
        max_plan_time: Some(0.0),
        mean_plan_time: Some(0.0),
        stddev_plan_time: Some(0.0),
        shared_blks_hit: sample,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        shared_blk_read_time: 0.0,
        shared_blk_write_time: 0.0,
        local_blk_read_time: Some(0.0),
        local_blk_write_time: Some(0.0),
        temp_blk_read_time: Some(0.0),
        temp_blk_write_time: Some(0.0),
        wal_records: Some(sample),
        wal_fpi: Some(0),
        wal_bytes: Some(sample),
        wal_buffers_full: Some(0),
        jit_functions: Some(0),
        jit_generation_time: Some(0.0),
        jit_inlining_count: Some(0),
        jit_inlining_time: Some(0.0),
        jit_optimization_count: Some(0),
        jit_optimization_time: Some(0.0),
        jit_emission_count: Some(0),
        jit_emission_time: Some(0.0),
        jit_deform_count: Some(0),
        jit_deform_time: Some(0.0),
        parallel_workers_to_launch: Some(0),
        parallel_workers_launched: Some(0),
        stats_since: Some(BASE_TS),
        minmax_stats_since: Some(BASE_TS),
    }
}

fn statement_batch(batch_index: usize) -> (PgBatch, usize, usize) {
    let unique_start = BASE_QUERY_COUNT + batch_index * UNIQUE_QUERY_COUNT;
    let rows = (0..BASE_QUERY_COUNT)
        .chain(unique_start..unique_start + UNIQUE_QUERY_COUNT)
        .map(|query_index| statement_row(batch_index, query_index))
        .collect::<Vec<_>>();
    let query_bytes = rows
        .iter()
        .filter_map(|row| row.query.as_ref())
        .map(String::len)
        .sum::<usize>();
    let row_count = rows.len();
    let logical_bytes =
        query_bytes.saturating_add(row_count.saturating_mul(STATEMENT_ROW_OVERHEAD_BOUND));
    assert_eq!(row_count, BASE_QUERY_COUNT + UNIQUE_QUERY_COUNT);
    assert!(
        row_count <= BATCH_ROWS,
        "one retained source batch fits the row bound"
    );
    assert_eq!(query_bytes, row_count * QUERY_BYTES);
    assert!(
        logical_bytes <= BATCH_LOGICAL_BYTES,
        "one retained statement batch stays below the source bound"
    );
    (
        PgBatch::Statements(StatementsVersion::V6, rows),
        logical_bytes,
        row_count,
    )
}

fn plan_text(index: usize) -> String {
    let mut plan = query_text(index.saturating_mul(2));
    plan.push_str(&query_text(index.saturating_mul(2).saturating_add(1)));
    assert_eq!(plan.len(), PLAN_BYTES);
    plan
}

fn plan_row(batch_index: usize, plan_index: usize) -> VadvRow {
    let sample = i64::try_from(batch_index).unwrap_or(i64::MAX);
    let identity = i64::try_from(plan_index)
        .unwrap_or(i64::MAX)
        .saturating_add(1);
    VadvRow {
        ts: BASE_TS.saturating_add(sample),
        userid: 16_385,
        dbid: 16_384,
        queryid: identity,
        planid: identity,
        queryid_stat_statements: identity,
        datname: Some("app".to_owned()),
        usename: Some("monitor".to_owned()),
        plan: Some(plan_text(plan_index)),
        calls: sample.saturating_add(1),
        slow_log_calls: 0,
        total_time: 0.0,
        min_time: 0.0,
        max_time: 0.0,
        mean_time: 0.0,
        stddev_time: 0.0,
        rows: identity,
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
        first_call: BASE_TS,
        last_call: BASE_TS,
        total_plan_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
    }
}

fn plan_batch(batch_index: usize) -> (PgBatch, usize, usize) {
    let first = batch_index.saturating_mul(PLAN_ROWS_PER_BATCH);
    let rows = (first..first + PLAN_ROWS_PER_BATCH)
        .map(|plan_index| plan_row(batch_index, plan_index))
        .collect::<Vec<_>>();
    let logical_bytes = rows
        .len()
        .saturating_mul(PLAN_BYTES.saturating_add(PLAN_ROW_OVERHEAD_BYTES));
    assert!(rows.len() <= BATCH_ROWS);
    assert!(logical_bytes >= BATCH_LOGICAL_BYTES);
    assert!(
        logical_bytes <= BATCH_LOGICAL_BYTES.saturating_add(PLAN_BYTES + PLAN_ROW_OVERHEAD_BYTES),
        "the source checks its byte target after appending a complete row"
    );
    (
        PgBatch::StorePlansVadv(rows),
        logical_bytes,
        PLAN_ROWS_PER_BATCH,
    )
}

impl ReplayAppend<'_> {
    fn append(&mut self, batch: &PgBatch, ts: i64) {
        for attempt in 0..2 {
            let includes_settings = self.segment.needs_pg_settings() && !self.settings.is_empty();
            let opening_settings = if includes_settings {
                self.settings
            } else {
                &[]
            };
            let mut buffers = SectionBuffers::new();
            push_pg_batch(
                &mut buffers,
                self.segment.interner_mut(),
                batch,
                opening_settings,
            )
            .expect("buffer retained PostgreSQL batch");
            let flushed = encode_window(buffers, self.segment.interner())
                .expect("encode retained PostgreSQL batch");
            let appended_bytes = u64::try_from(flushed.summary.part_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(FRAME_HEADER_LEN).unwrap_or(u64::MAX));
            let finished = append_window_and_maybe_close(
                self.journal,
                self.writer,
                self.config,
                self.segment,
                ts,
                false,
                &flushed,
            )
            .expect("append retained PostgreSQL batch");
            let retry = finished
                .iter()
                .any(|(_, reason)| matches!(*reason, "format-limit" | "journal-full"));
            for (path, reason) in finished {
                self.report.paths.push(path);
                *self.report.close_reasons.entry(reason).or_default() += 1;
            }
            if retry {
                assert_eq!(attempt, 0, "a fresh segment accepts the retained batch");
                continue;
            }
            if includes_settings && !self.segment.is_empty() {
                self.segment.mark_pg_settings_present();
            }
            self.report.cumulative_appended_wal_bytes = self
                .report
                .cumulative_appended_wal_bytes
                .saturating_add(appended_bytes);
            self.report.peak_wal_bytes = self.report.peak_wal_bytes.max(self.journal.bytes());
            self.report.appended_windows += 1;
            return;
        }
        panic!("retained PostgreSQL batch exhausted its append attempts");
    }
}

fn assert_query_rows(
    segment: &kronika_reader::Segment,
    counts: &mut BTreeMap<i64, usize>,
) -> usize {
    let dictionary = segment.dictionary().expect("read segment dictionary");
    let rows = segment
        .rows(PG_STAT_STATEMENTS_V6_TYPE_ID)
        .expect("read statement rows");
    for row in &rows {
        let Some(Cell::I64(query_id)) = row.get("queryid") else {
            panic!("statement queryid must be present");
        };
        let Some(Cell::StrId(query)) = row.get("query") else {
            panic!("statement text must be present");
        };
        let query_index =
            usize::try_from(query_id.saturating_sub(1)).expect("positive queryid fits usize");
        let expected = query_text(query_index);
        match dictionary.resolve(*query) {
            Some(Resolved::Str(actual)) => assert_eq!(actual, expected.as_bytes()),
            Some(Resolved::Blob(_)) => panic!("short statement text belongs in dict.strings"),
            None => panic!("statement text id resolves"),
        }
        *counts.entry(*query_id).or_default() += 1;
    }
    rows.len()
}

fn assert_query_counts(counts: &BTreeMap<i64, usize>) {
    assert_eq!(
        counts.len(),
        BASE_QUERY_COUNT + STATEMENT_BATCH_COUNT * UNIQUE_QUERY_COUNT
    );
    for query_index in 0..BASE_QUERY_COUNT {
        let query_id = i64::try_from(query_index).unwrap_or(i64::MAX) + 1;
        assert_eq!(counts.get(&query_id), Some(&STATEMENT_BATCH_COUNT));
    }
    for query_index in
        BASE_QUERY_COUNT..BASE_QUERY_COUNT + STATEMENT_BATCH_COUNT * UNIQUE_QUERY_COUNT
    {
        let query_id = i64::try_from(query_index).unwrap_or(i64::MAX) + 1;
        assert_eq!(counts.get(&query_id), Some(&1));
    }
}

fn assert_plan_rows(segment: &kronika_reader::Segment, seen: &mut usize) -> usize {
    let dictionary = segment.dictionary().expect("read segment dictionary");
    let rows = segment
        .rows(PG_STORE_PLANS_VADV_TYPE_ID)
        .expect("read plan rows");
    for row in &rows {
        let Some(Cell::I64(plan_id)) = row.get("planid") else {
            panic!("plan id must be present");
        };
        let Some(Cell::StrId(plan)) = row.get("plan") else {
            panic!("plan text must be present");
        };
        let plan_index =
            usize::try_from(plan_id.saturating_sub(1)).expect("positive plan id fits usize");
        let expected = plan_text(plan_index);
        match dictionary.resolve(*plan) {
            Some(Resolved::Blob(actual)) => {
                assert_eq!(actual.stored_bytes, expected.as_bytes());
                assert_eq!(
                    actual.full_len,
                    u64::try_from(PLAN_BYTES).unwrap_or(u64::MAX)
                );
                assert!(!actual.truncated);
                assert!(actual.full_sha256.is_none());
            }
            Some(Resolved::Str(_)) => panic!("plan text belongs in dict.blobs"),
            None => panic!("plan text id resolves"),
        }
        *seen += 1;
    }
    rows.len()
}

fn read_replay_artifacts(root: &Path, expected_paths: usize) -> ReplayArtifactReport {
    let reader = Reader::open(root).expect("open replay reader");
    let listing = reader.segments(..).expect("list replay segments");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), expected_paths);

    let mut report = ReplayArtifactReport {
        segments: listing.segments.len(),
        zms_bytes: 0,
        section_body_bytes: 0,
        overhead_bytes: 0,
        windows: 0,
        statement_rows: 0,
        plan_rows: 0,
        rss_kib: 0,
        sections: BTreeMap::new(),
    };
    let mut query_counts = BTreeMap::new();
    let mut plans_seen = 0;
    for reference in &listing.segments {
        assert_eq!(reference.kind(), SegmentKind::Finished);
        let segment = reader
            .open_segment(reference)
            .expect("open finished replay segment");
        report.zms_bytes = report.zms_bytes.saturating_add(segment.captured_bytes());
        report.windows = report
            .windows
            .saturating_add(u64::from(segment.window_count()));
        for (type_id, section) in segment.sections() {
            report.section_body_bytes = report.section_body_bytes.saturating_add(section.bytes);
            let aggregate = report
                .sections
                .entry(section_name(type_id).unwrap_or("unknown"))
                .or_default();
            aggregate.rows = aggregate.rows.saturating_add(section.rows);
            aggregate.bytes = aggregate.bytes.saturating_add(section.bytes);
        }
        report.statement_rows = report
            .statement_rows
            .saturating_add(assert_query_rows(&segment, &mut query_counts));
        report.plan_rows = report
            .plan_rows
            .saturating_add(assert_plan_rows(&segment, &mut plans_seen));
    }
    assert_query_counts(&query_counts);
    assert_eq!(plans_seen, PLAN_BATCH_COUNT * PLAN_ROWS_PER_BATCH);
    report.overhead_bytes = report
        .zms_bytes
        .checked_sub(report.section_body_bytes)
        .expect("section bodies fit inside ZMS files");
    report.rss_kib = peak_rss_kib();
    report
}

fn assert_replay_costs(
    write: &ReplayWriteReport,
    artifact: &ReplayArtifactReport,
    max_journal_len: usize,
) {
    assert_eq!(artifact.segments, 1);
    assert_eq!(
        artifact.windows,
        u64::try_from(STATEMENT_BATCH_COUNT + PLAN_BATCH_COUNT).unwrap_or(u64::MAX)
    );
    assert_eq!(
        artifact.statement_rows,
        STATEMENT_BATCH_COUNT * (BASE_QUERY_COUNT + UNIQUE_QUERY_COUNT)
    );
    assert_eq!(artifact.plan_rows, PLAN_BATCH_COUNT * PLAN_ROWS_PER_BATCH);
    assert_eq!(write.close_reasons.get("test-end"), Some(&1));
    assert!(write.peak_wal_bytes <= max_journal_len);
    assert!(artifact.zms_bytes <= MAX_REPLAY_ZMS_BYTES);
    assert!(artifact.zms_bytes.saturating_mul(10) <= PRE_CHANGE_ZMS_BYTES.saturating_mul(9));
    assert!(write.cumulative_appended_wal_bytes <= MAX_REPLAY_WAL_BYTES);
    let strings = artifact
        .sections
        .get("dict.strings")
        .expect("replay carries its strings dictionary");
    assert!(strings.bytes <= MAX_REPLAY_DICT_STRINGS_BYTES);
    assert!(strings.bytes.saturating_mul(5) <= PRE_CHANGE_DICT_STRINGS_BYTES.saturating_mul(4));
    let blobs = artifact
        .sections
        .get("dict.blobs")
        .expect("replay carries its blobs dictionary");
    assert!(blobs.bytes <= MAX_REPLAY_DICT_BLOBS_BYTES);
}

fn print_replay_costs(write: ReplayWriteReport, artifact: ReplayArtifactReport) {
    println!(
        "zms_replay segments={} zms_bytes={} peak_wal_bytes={} cumulative_appended_wal_bytes={} section_body_bytes={} overhead_bytes={} windows={} statement_rows={} plan_rows={} max_batch_rows={} max_batch_logical_bytes={} peak_rss_kib={}",
        artifact.segments,
        artifact.zms_bytes,
        write.peak_wal_bytes,
        write.cumulative_appended_wal_bytes,
        artifact.section_body_bytes,
        artifact.overhead_bytes,
        artifact.windows,
        artifact.statement_rows,
        artifact.plan_rows,
        write.max_batch_rows,
        write.max_batch_logical_bytes,
        artifact.rss_kib,
    );
    for (reason, count) in write.close_reasons {
        println!("zms_replay_close reason={reason} count={count}");
    }
    for (name, section) in artifact.sections {
        println!(
            "zms_replay_section name={name} rows={} bytes={}",
            section.rows, section.bytes
        );
    }
}

#[test]
fn bounded_postgres_batches_report_finished_segment_costs() {
    let directory = tempfile::tempdir().expect("create replay directory");
    let writer = owner(directory.path());
    let journal_config = JournalConfig::default();
    let max_journal_len = journal_config.max_journal_len;
    let mut journal = Journal::open(&writer, journal_config).expect("open replay journal");
    let config = config(
        directory.path(),
        u64::try_from(max_journal_len).expect("journal bound fits u64"),
    );
    let settings = [settings_row()];
    let mut segment = SegmentState::default();
    let mut write_report = ReplayWriteReport::default();

    {
        let mut replay = ReplayAppend {
            journal: &mut journal,
            writer: &writer,
            config: &config,
            segment: &mut segment,
            settings: &settings,
            report: &mut write_report,
        };
        for batch_index in 0..STATEMENT_BATCH_COUNT {
            let (batch, logical_bytes, batch_rows) = statement_batch(batch_index);
            replay.report.max_batch_rows = replay.report.max_batch_rows.max(batch_rows);
            replay.report.max_batch_logical_bytes =
                replay.report.max_batch_logical_bytes.max(logical_bytes);
            replay.append(
                &batch,
                BASE_TS + i64::try_from(batch_index).unwrap_or(i64::MAX),
            );
        }
        for batch_index in 0..PLAN_BATCH_COUNT {
            let (batch, logical_bytes, batch_rows) = plan_batch(batch_index);
            replay.report.max_batch_rows = replay.report.max_batch_rows.max(batch_rows);
            replay.report.max_batch_logical_bytes =
                replay.report.max_batch_logical_bytes.max(logical_bytes);
            replay.append(
                &batch,
                BASE_TS + i64::try_from(STATEMENT_BATCH_COUNT + batch_index).unwrap_or(i64::MAX),
            );
        }
    }

    assert_eq!(
        write_report.appended_windows,
        STATEMENT_BATCH_COUNT + PLAN_BATCH_COUNT
    );
    assert!(
        write_report.paths.is_empty(),
        "bounded PostgreSQL batches stay in the open logical segment"
    );
    assert!(
        write_report.close_reasons.is_empty(),
        "no retained batch requests an early write"
    );
    assert!(write_report.peak_wal_bytes <= max_journal_len);
    if !segment.is_empty() {
        let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
            .expect("write final replay segment");
        write_report.paths.push(path);
        *write_report.close_reasons.entry("test-end").or_default() += 1;
    }
    let artifact = read_replay_artifacts(directory.path(), write_report.paths.len());
    assert_eq!(
        write_report
            .paths
            .iter()
            .map(|path| std::fs::metadata(path).expect("stat replay segment").len())
            .sum::<u64>(),
        artifact.zms_bytes
    );
    assert_replay_costs(&write_report, &artifact, max_journal_len);
    print_replay_costs(write_report, artifact);
}

#[test]
fn wal_storage_hour_reports_raw_and_finished_costs() {
    let directory = tempfile::tempdir().expect("create WAL storage cost directory");
    let writer = owner(directory.path());
    let journal_config = JournalConfig::default();
    let mut journal = Journal::open(&writer, journal_config).expect("open WAL storage journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let mut write_report = ReplayWriteReport::default();

    {
        let mut replay = ReplayAppend {
            journal: &mut journal,
            writer: &writer,
            config: &config,
            segment: &mut segment,
            settings: &[],
            report: &mut write_report,
        };
        for sample in 0..WAL_STORAGE_SNAPSHOTS_PER_HOUR {
            let sample = i64::try_from(sample).expect("sample count fits i64");
            let ts = BASE_TS.saturating_add(sample.saturating_mul(30_000_000));
            replay.append(
                &PgBatch::WalStorage(PgWalStorage {
                    ts: Ts(ts),
                    wal_files_bytes: 16_777_216_i64.saturating_mul(sample.saturating_add(1)),
                }),
                ts,
            );
        }
    }

    let raw_wal_bytes = journal.bytes();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write WAL storage cost segment");
    let reader = Reader::open(directory.path()).expect("open WAL storage cost reader");
    let listing = reader.segments(..).expect("list WAL storage cost segment");
    let reference = listing.segments.first().expect("one WAL storage segment");
    let stored = reader
        .open_segment(reference)
        .expect("open finished WAL storage segment");
    let rows = stored
        .rows(PG_WAL_STORAGE_TYPE_ID)
        .expect("read WAL storage rows");
    let section = stored
        .sections()
        .find(|(type_id, _section)| *type_id == PG_WAL_STORAGE_TYPE_ID)
        .map(|(_type_id, section)| section)
        .expect("WAL storage section is catalogued");
    let zms_bytes = std::fs::metadata(path)
        .expect("stat WAL storage segment")
        .len();
    let marginal_zms_bytes = section
        .bytes
        .saturating_add(u64::try_from(ENTRY_LEN).expect("catalog entry length fits u64"));

    assert_eq!(listing.segments.len(), 1);
    assert_eq!(
        write_report.appended_windows,
        WAL_STORAGE_SNAPSHOTS_PER_HOUR
    );
    assert_eq!(
        stored.window_count(),
        u32::try_from(WAL_STORAGE_SNAPSHOTS_PER_HOUR).expect("sample count fits u32")
    );
    assert_eq!(rows.len(), WAL_STORAGE_SNAPSHOTS_PER_HOUR);
    assert_eq!(
        rows.first().and_then(|row| row.get("wal_files_bytes")),
        Some(&Cell::I64(16_777_216))
    );
    assert_eq!(
        rows.last().and_then(|row| row.get("wal_files_bytes")),
        Some(&Cell::I64(2_013_265_920))
    );
    assert!(raw_wal_bytes < 256 * 1024);
    assert!(section.bytes < 4 * 1024);
    assert!(zms_bytes < 8 * 1024);
    println!(
        "pg_wal_storage_cost rows={} raw_wal_bytes={} section_bytes={} marginal_zms_bytes={} zms_bytes={}",
        rows.len(),
        raw_wal_bytes,
        section.bytes,
        marginal_zms_bytes,
        zms_bytes
    );
}

#[test]
fn cgroup_context_hour_reports_raw_and_finished_costs() {
    let directory = tempfile::tempdir().expect("create cgroup context cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open cgroup context journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();

    for sample in 0..WAL_STORAGE_SNAPSHOTS_PER_HOUR {
        let sample = i64::try_from(sample).expect("sample count fits i64");
        let ts = BASE_TS.saturating_add(sample.saturating_mul(30_000_000));
        let path = segment
            .interner_mut()
            .intern(b"/kubepods/pod-a/container-a")
            .map(|id| StrId(id.get()))
            .expect("intern cgroup path");
        let mut buffers = SectionBuffers::new();
        let sources = OsSources::cgroup_context_only(OsCgroupContext {
            ts: Ts(ts),
            cgroup_version: 2,
            cpu_path: Some(path),
            memory_path: Some(path),
            io_path: Some(path),
            cpuset_cpus: Some(2),
            scope: 3,
        });
        push_os_sources(&mut buffers, &sources).expect("buffer cgroup context");
        let flushed = encode_window(buffers, segment.interner()).expect("encode cgroup context");
        let completed = append_window_and_maybe_close(
            &mut journal,
            &writer,
            &config,
            &mut segment,
            ts,
            false,
            &flushed,
        )
        .expect("append cgroup context");
        assert!(completed.is_empty());
    }

    let raw_wal_bytes = journal.bytes();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write cgroup context cost segment");
    let reader = Reader::open(directory.path()).expect("open cgroup context cost reader");
    let listing = reader
        .segments(..)
        .expect("list cgroup context cost segment");
    let reference = listing
        .segments
        .first()
        .expect("one cgroup context segment");
    let stored = reader
        .open_segment(reference)
        .expect("open finished cgroup context segment");
    let rows = stored
        .rows(CGROUP_CONTEXT_TYPE_ID)
        .expect("read cgroup context rows");
    let section = stored
        .sections()
        .find(|(type_id, _section)| *type_id == CGROUP_CONTEXT_TYPE_ID)
        .map(|(_type_id, section)| section)
        .expect("cgroup context section is catalogued");
    let zms_bytes = std::fs::metadata(path)
        .expect("stat cgroup context segment")
        .len();
    let marginal_zms_bytes = section
        .bytes
        .saturating_add(u64::try_from(ENTRY_LEN).expect("catalog entry length fits u64"));

    assert_eq!(listing.segments.len(), 1);
    assert_eq!(stored.window_count(), 120);
    assert_eq!(rows.len(), WAL_STORAGE_SNAPSHOTS_PER_HOUR);
    assert_eq!(
        rows.first().and_then(|row| row.get("cpuset_cpus")),
        Some(&Cell::I64(2))
    );
    let dictionary = stored.dictionary().expect("read cgroup context dictionary");
    for field in ["cpu_path", "memory_path", "io_path"] {
        let Some(Cell::StrId(path)) = rows.first().and_then(|row| row.get(field)) else {
            panic!("cgroup context {field} must be persisted");
        };
        match dictionary.resolve(*path) {
            Some(Resolved::Str(actual)) => {
                assert_eq!(actual, b"/kubepods/pod-a/container-a");
            }
            Some(Resolved::Blob(_)) => panic!("cgroup context path belongs in dict.strings"),
            None => panic!("cgroup context {field} id resolves"),
        }
    }
    assert!(raw_wal_bytes < 512 * 1024);
    assert!(section.bytes < 8 * 1024);
    assert!(zms_bytes < 16 * 1024);
    println!(
        "os_cgroup_context_cost rows={} raw_wal_bytes={} section_bytes={} marginal_zms_bytes={} zms_bytes={}",
        rows.len(),
        raw_wal_bytes,
        section.bytes,
        marginal_zms_bytes,
        zms_bytes
    );
}
