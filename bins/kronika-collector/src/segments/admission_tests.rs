use std::fs;
use std::path::Path;

use kronika_format::{
    DictLimits, FRAME_HEADER_LEN, JOURNAL_HEADER_LEN, PartMeta, RESET_MARKER_LEN, SectionInput,
    build_part, validate_part,
};
use kronika_layout::{DataRoot, FileKind, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::{
    CodecError, DICT_STRINGS_TYPE_ID, FINAL_DATA_PAGE_BYTES, MAX_SECTION_ROWS, Section, Ts,
    final_data_body_bound,
};
use kronika_writer::{
    FlushSummary, FlushedPart, Interner, Journal, JournalConfig, SectionBuffers,
    SectionFlushSummary,
};

use crate::config::Config;
use crate::scheduler::Intervals;

use super::admission::{AdmissionError, SegmentAdmission};
use super::open::{open_collector_journal, write_recovered_journal};
use super::{SegmentState, append_window_and_maybe_close, close_open_segment, encode_window};

fn data_summary(type_id: u32, rows: usize, list_i32_child_value_count: usize) -> FlushSummary {
    FlushSummary {
        sections: vec![SectionFlushSummary {
            type_id,
            rows: u32::try_from(rows).expect("test row count fits u32"),
            body_bytes: 1,
            list_i32_child_value_count,
        }],
        part_bytes: 1,
    }
}

fn empty_interner() -> Interner {
    Interner::new(DictLimits::new(8, 64).expect("test dictionary limits are valid"))
}

fn interner(blob_threshold: usize, value: &[u8]) -> Interner {
    let mut interner = Interner::new(DictLimits::new(blob_threshold, 64).expect("valid limits"));
    interner.intern(value).expect("test value interns");
    interner
}

fn loadavg(ts: i64) -> OsLoadavg {
    OsLoadavg {
        ts: Ts(ts),
        load1: 1.5,
        load5: 1.0,
        load15: 0.5,
        running: 2,
        total: 345,
        scope: 0,
    }
}

fn flushed_window(ts: i64) -> FlushedPart {
    let mut buffers = SectionBuffers::new();
    buffers.push(loadavg(ts)).expect("one row fits");
    buffers
        .flush_with_summary(&[])
        .expect("window encodes")
        .expect("one row yields one part")
}

fn test_config(out_dir: &Path) -> Config {
    Config {
        out_dir: out_dir.to_path_buf(),
        tick_secs: 5,
        intervals: Intervals::default(),
        segment_max_bytes: u64::MAX,
        segment_max_age_secs: u64::MAX,
        journal_max_bytes: u64::MAX,
        retention: None,
        pg_dsns: Vec::new(),
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
    }
}

fn open_journal(root_path: &Path, max_journal_len: usize) -> (WriterOwner, Journal) {
    let root = DataRoot::open(root_path).expect("open test data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire test writer");
    let journal = Journal::open(
        &owner,
        JournalConfig {
            max_journal_len,
            ..JournalConfig::default()
        },
    )
    .expect("open test journal");
    (owner, journal)
}

fn one_part_journal_cap(part_len: usize) -> usize {
    JOURNAL_HEADER_LEN + FRAME_HEADER_LEN + part_len + RESET_MARKER_LEN
}

fn segment_path(owner: &WriterOwner, ts: i64) -> std::path::PathBuf {
    let id = SegmentId::new(ts).expect("test timestamp is a valid segment id");
    let address = SegmentAddress::new(id).expect("test segment has a UTC address");
    owner.root().diagnostic_file_path(address, FileKind::Zms)
}

fn max_admitted_rows(type_id: u32) -> usize {
    let mut low = 0;
    let mut high = MAX_SECTION_ROWS + 1;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if final_data_body_bound(type_id, middle, 0).is_ok() {
            low = middle;
        } else {
            high = middle;
        }
    }
    assert!(low > 0, "the test type admits at least one row");
    assert!(final_data_body_bound(type_id, low, 0).is_ok());
    assert!(final_data_body_bound(type_id, low + 1, 0).is_err());
    low
}

#[test]
fn admission_deduplicates_exact_dictionary_values() {
    let type_id = OsLoadavg::CONTRACT.type_id.get();
    let first = interner(8, b"same");
    let second = interner(8, b"same");
    let mut admission = SegmentAdmission::default();

    let delta = admission
        .assess(&data_summary(type_id, 1, 0), &first)
        .expect("first window fits");
    admission.commit(delta);
    let bytes_after_first = admission.string_stored_bytes;
    let delta = admission
        .assess(&data_summary(type_id, 1, 0), &second)
        .expect("the repeated value and second data row fit");
    assert!(
        delta.dictionary.is_empty(),
        "the duplicate adds no dictionary row"
    );
    admission.commit(delta);

    assert_eq!(admission.dictionary.len(), 1);
    assert_eq!(admission.string_rows, 1, "the repeated id counts once");
    assert_eq!(admission.string_stored_bytes, bytes_after_first);
}

#[test]
fn admission_rejects_cross_dictionary_placement() {
    let strings = interner(8, b"same");
    let blobs = interner(1, b"same");
    let mut admission = SegmentAdmission::default();
    let empty = FlushSummary {
        sections: Vec::new(),
        part_bytes: 0,
    };

    let delta = admission.assess(&empty, &strings).expect("string fits");
    admission.commit(delta);
    assert!(matches!(
        admission.assess(&empty, &blobs),
        Err(AdmissionError::DictionaryPlacementConflict { .. })
    ));
}

#[test]
fn dictionary_plain_budgets_are_independent_per_placement() {
    let mut admission = SegmentAdmission {
        string_rows: 1,
        string_stored_bytes: FINAL_DATA_PAGE_BYTES - 5,
        ..SegmentAdmission::default()
    };
    let summary = FlushSummary {
        sections: Vec::new(),
        part_bytes: 0,
    };
    let blob = interner(1, b"blob");
    let delta = admission
        .assess(&summary, &blob)
        .expect("a full strings value page does not consume the blobs value page");
    admission.commit(delta);

    let string = interner(8, b"new");
    assert!(matches!(
        admission.assess(&summary, &string),
        Err(AdmissionError::Codec(CodecError::PlainPageTooLarge {
            name: "bytes",
            ..
        }))
    ));
}

#[test]
fn admission_projects_section_descriptors_against_the_format_cap() {
    let type_id = OsLoadavg::CONTRACT.type_id.get();
    let interner = empty_interner();
    let admission = SegmentAdmission {
        descriptors: MAX_SECTION_ROWS,
        ..SegmentAdmission::default()
    };
    assert!(matches!(
        admission.assess(&data_summary(type_id, 1, 0), &interner),
        Err(AdmissionError::Capacity {
            resource: "section descriptors",
            projected,
            max: MAX_SECTION_ROWS,
        }) if projected == MAX_SECTION_ROWS + 1
    ));
}

#[test]
fn format_capacity_crossing_writes_accumulated_segment_before_append() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let config = test_config(dir.path());
    let mut segment = SegmentState::default();
    let first = flushed_window(100);
    let incoming = flushed_window(200);

    assert!(
        append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            100,
            false,
            &first,
        )
        .expect("append first")
        .is_empty()
    );
    let type_id = first.summary.sections[0].type_id;
    segment
        .admission
        .data_by_type
        .get_mut(&type_id)
        .expect("first row was admitted")
        .rows = max_admitted_rows(type_id);

    let finished = append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &incoming,
    )
    .expect("capacity crossing writes the accumulated segment");

    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, "format-limit");
    let old = fs::read(&finished[0].0).expect("read finished old segment");
    let old_catalog = validate_part(&old).expect("old segment is canonical");
    assert_eq!(old_catalog.entries.len(), 1);
    assert_eq!(old_catalog.entries[0].rows, 1);
    assert_eq!(old_catalog.min_ts, 100);
    assert!(
        journal.parts().is_empty(),
        "the next cycle opens a new segment"
    );
    assert_eq!(segment.first_ts(), None);
    assert_eq!(segment.admission, SegmentAdmission::default());
}

#[test]
fn intrinsically_oversized_window_preserves_active_journal_and_admission() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let config = test_config(dir.path());
    let mut segment = SegmentState::default();
    let first = flushed_window(100);
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append first");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");
    let first_before = segment.first_ts();
    let admission_before = segment.admission.clone();
    let dictionary_before = segment.interner.stats();
    let mut oversized = flushed_window(200);
    oversized.summary.sections[0].rows =
        u32::try_from(MAX_SECTION_ROWS + 1).expect("row count fits u32");

    let err = append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &oversized,
    )
    .expect_err("one oversized window is rejected");
    assert!(format!("{err:#}").contains("one collection window exceeds finished segment limits"));
    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(segment.first_ts(), first_before);
    assert_eq!(segment.admission, admission_before);
    assert_eq!(segment.interner.stats(), dictionary_before);
    assert_eq!(journal.parts().len(), 1);
    assert!(!segment_path(&owner, 100).exists());
}

#[test]
fn journal_full_writes_accumulated_segment_and_defers_window() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = test_config(dir.path());
    let mut segment = SegmentState::default();
    let first = flushed_window(100);
    let incoming = flushed_window(200);
    let (owner, mut journal) = open_journal(dir.path(), one_part_journal_cap(first.body.len()));
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("the first frame is exempt from the journal cap");

    let finished = append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &incoming,
    )
    .expect("full journal writes the accumulated segment");

    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, "journal-full");
    assert!(journal.parts().is_empty());
    assert_eq!(segment.first_ts(), None);
    assert_eq!(segment.admission, SegmentAdmission::default());
}

#[test]
fn invalid_part_at_journal_cap_is_transactional() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let config = test_config(dir.path());
    let mut segment = SegmentState::default();
    let first = flushed_window(100);
    let (owner, mut journal) = open_journal(dir.path(), one_part_journal_cap(first.body.len()));
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append first");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");
    let first_before = segment.first_ts();
    let admission_before = segment.admission.clone();
    let dictionary_before = segment.interner.stats();
    let invalid = FlushedPart {
        body: b"not a ZMS part".to_vec(),
        summary: flushed_window(200).summary,
    };

    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &invalid,
    )
    .expect_err("invalid incoming part is rejected before a full-journal write");

    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(segment.first_ts(), first_before);
    assert_eq!(segment.admission, admission_before);
    assert_eq!(segment.interner.stats(), dictionary_before);
    assert_eq!(journal.parts().len(), 1);
    assert!(!segment_path(&owner, 100).exists());
}

#[test]
fn persistent_interner_writes_only_new_dictionary_entries_to_each_part() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = test_config(dir.path());
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let mut segment = SegmentState::default();

    segment
        .interner_mut()
        .intern(b"shared-value")
        .expect("intern first value");
    let mut first_buffers = SectionBuffers::new();
    first_buffers.push(loadavg(100)).expect("buffer first row");
    let first = encode_window(first_buffers, segment.interner()).expect("encode first window");
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append first window");

    segment
        .interner_mut()
        .intern(b"shared-value")
        .expect("re-intern shared value");
    let mut second_buffers = SectionBuffers::new();
    second_buffers
        .push(loadavg(200))
        .expect("buffer second row");
    let second = encode_window(second_buffers, segment.interner()).expect("encode second window");
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &second,
    )
    .expect("append second window");

    let first_part = journal
        .read_part(journal.parts()[0])
        .expect("read first WAL part");
    let first_catalog = validate_part(&first_part).expect("validate first WAL part");
    assert!(
        first_catalog
            .entries
            .iter()
            .any(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
    );
    let second_part = journal
        .read_part(journal.parts()[1])
        .expect("read second WAL part");
    let second_catalog = validate_part(&second_part).expect("validate second WAL part");
    assert!(
        second_catalog
            .entries
            .iter()
            .all(|entry| entry.type_id != DICT_STRINGS_TYPE_ID)
    );

    let dest = close_open_segment(&mut journal, &owner, &mut segment, "test")
        .expect("write reconstructed segment");
    let finished = fs::read(dest).expect("read reconstructed segment");
    let finished_catalog = validate_part(&finished).expect("validate reconstructed segment");
    assert_eq!(
        finished_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1)
    );
    assert_eq!(
        finished_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == OsLoadavg::CONTRACT.type_id.get())
            .map(|entry| entry.rows),
        Some(2)
    );
}

#[test]
fn recovery_preserves_an_unreadable_journal_at_its_canonical_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let bytes = b"not a valid journal";
    fs::write(&path, bytes).expect("write unreadable journal");
    let root = DataRoot::open(dir.path()).expect("open test data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire test writer");
    let journal_max_bytes =
        u64::try_from(JournalConfig::default().max_journal_len).expect("journal cap fits u64");

    let err = open_collector_journal(&owner, journal_max_bytes)
        .expect_err("an unreadable journal stops recovery");

    assert!(format!("{err:#}").contains("existing file is preserved"));
    assert_eq!(fs::read(&path).expect("read preserved journal"), bytes);
    assert!(!dir.path().join("active.wal.damaged").exists());
}

#[test]
fn recovery_publication_failure_keeps_the_readable_journal_canonical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let part = flushed_window(100);
    journal
        .append(
            SegmentId::new(100).expect("valid recovery identity"),
            &part.body,
        )
        .expect("append readable part");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");
    let destination = segment_path(&owner, 100);
    fs::create_dir_all(destination.parent().expect("segment has a parent"))
        .expect("create segment day");
    fs::write(&destination, b"conflicting segment").expect("write conflicting segment");

    write_recovered_journal(&mut journal, &owner)
        .expect_err("a conflicting destination stops recovery");

    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(
        fs::read(destination).expect("read existing segment"),
        b"conflicting segment"
    );
    assert_eq!(journal.parts().len(), 1);
}

#[test]
fn recovery_preserves_a_populated_part_without_a_timestamp() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let body = OsLoadavg::encode(&[loadavg(100)]).expect("encode section");
    let part = build_part(
        &[SectionInput {
            type_id: OsLoadavg::CONTRACT.type_id.get(),
            rows: 1,
            body: &body,
        }],
        PartMeta {
            min_ts: i64::MAX,
            max_ts: i64::MIN,
        },
    );
    journal
        .append(SegmentId::new(100).expect("valid recovery identity"), &part)
        .expect("append structurally valid part");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");

    let err = write_recovered_journal(&mut journal, &owner)
        .expect_err("populated sentinel-timestamp part is not publishable");

    assert!(format!("{err:#}").contains("active.wal is preserved"));
    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(journal.parts().len(), 1);
    assert!(
        fs::read_dir(dir.path())
            .expect("read output directory")
            .all(|entry| {
                entry.expect("directory entry").path().extension()
                    != Some(std::ffi::OsStr::new("zms"))
            })
    );
}

#[test]
fn recovery_publishes_a_readable_journal_without_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let part = build_part(
        &[],
        PartMeta {
            min_ts: i64::MAX,
            max_ts: i64::MIN,
        },
    );
    journal
        .append(SegmentId::new(100).expect("valid recovery identity"), &part)
        .expect("append empty but structurally valid part");

    let dest = write_recovered_journal(&mut journal, &owner)
        .expect("publish the readable journal")
        .expect("a nonempty journal gets a publication attempt");

    let recovered = fs::read(dest).expect("read recovered segment");
    let catalog = validate_part(&recovered).expect("recovered segment is valid");
    assert!(catalog.entries.is_empty());
    assert_eq!((catalog.min_ts, catalog.max_ts), (0, 0));
    assert!(journal.parts().is_empty());
    assert_eq!(
        fs::metadata(path).expect("stat reset journal").len(),
        JOURNAL_HEADER_LEN as u64
    );
}

#[test]
fn recovery_publishes_a_valid_dictionary_only_journal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let mut interner = empty_interner();
    interner.intern(b"dict").expect("intern dictionary value");
    let dictionary = kronika_writer::dict::encode(interner.window()).expect("encode dictionary");
    let part = SectionBuffers::new()
        .flush_with_summary(&dictionary)
        .expect("encode dictionary-only part")
        .expect("dictionary yields a part");
    journal
        .append(
            SegmentId::new(100).expect("valid recovery identity"),
            &part.body,
        )
        .expect("append dictionary-only part");

    let dest = write_recovered_journal(&mut journal, &owner)
        .expect("publish dictionary-only journal")
        .expect("a nonempty journal gets a publication attempt");

    let recovered = fs::read(dest).expect("read recovered segment");
    let catalog = validate_part(&recovered).expect("recovered segment is valid");
    assert_eq!((catalog.min_ts, catalog.max_ts), (0, 0));
    assert_eq!(
        catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1)
    );
}
