use std::path::Path;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentId};
use kronika_reader::{BlobEntry, Resolved};
use kronika_registry::{DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};

use super::{dictionary_json, percent, section, sizes, write_json_row};

#[test]
fn a_share_of_nothing_is_zero_rather_than_a_division_by_zero() {
    assert_eq!(percent(0, 0), 0);
    assert_eq!(percent(5, 0), 0);
}

#[test]
fn a_share_rounds_to_the_nearest_whole_percent() {
    assert_eq!(percent(1, 3), 33);
    assert_eq!(percent(2, 3), 67);
    assert_eq!(percent(1, 1), 100);
}

#[test]
fn a_huge_part_does_not_overflow_the_multiplication() {
    assert_eq!(percent(u64::MAX, u64::MAX), 100);
    assert_eq!(percent(u64::MAX / 2, u64::MAX), 50);
}

#[test]
fn a_part_larger_than_the_whole_is_capped_at_a_hundred() {
    assert_eq!(percent(3, 2), 100);
}

#[test]
fn section_and_overhead_shares_use_the_captured_segment_size() {
    let captured_bytes = 100;
    let section_bytes = 30;
    let overhead_bytes = captured_bytes - section_bytes;
    assert_eq!(percent(section_bytes, captured_bytes), 30);
    assert_eq!(percent(overhead_bytes, captured_bytes), 70);
}

#[test]
fn a_json_row_carries_its_segment_and_type() {
    let mut output = Vec::new();
    write_json_row(
        &mut output,
        Path::new("/data/one.zms"),
        1_107_001,
        &serde_json::json!({"ts": 42}),
    )
    .expect("write row");
    let line: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(line["kind"], "row");
    assert_eq!(line["path"], "/data/one.zms");
    assert_eq!(line["type_id"], 1_107_001);
    assert_eq!(line["row"], serde_json::json!({"ts": 42}));
}

#[test]
fn blob_json_keeps_raw_bytes_and_identity_metadata() {
    let hash = [0xa5; 32];
    let row = dictionary_json(
        7,
        Resolved::Blob(BlobEntry {
            str_id: kronika_format::StrId::from_raw(7).expect("nonzero id"),
            stored_bytes: &[0xff, 0x00, b'a'],
            full_len: 10,
            truncated: true,
            full_sha256: Some(hash),
        }),
    );
    assert_eq!(row["str_id"], 7);
    assert_eq!(row["stored_bytes"], serde_json::json!([255, 0, 97]));
    assert_eq!(row["full_len"], 10);
    assert_eq!(row["truncated"], true);
    assert_eq!(row["full_sha256"], serde_json::json!(hash));
}

#[test]
fn dictionary_section_selectors_dump_their_rows() {
    let (_directory, segment) = dictionary_segment();

    let mut strings = Vec::new();
    section(&mut strings, true, &segment, DICT_STRINGS_TYPE_ID, 0).expect("dump strings section");
    let strings: serde_json::Value = serde_json::from_slice(&strings).expect("strings JSON line");
    assert_eq!(strings["kind"], "row");
    assert_eq!(strings["type_id"], DICT_STRINGS_TYPE_ID);
    assert_eq!(
        strings["row"]["bytes"],
        serde_json::json!([115, 104, 111, 114, 116])
    );

    let mut blobs = Vec::new();
    section(&mut blobs, true, &segment, DICT_BLOBS_TYPE_ID, 0).expect("dump blobs section");
    let blobs: serde_json::Value = serde_json::from_slice(&blobs).expect("blobs JSON line");
    assert_eq!(blobs["kind"], "row");
    assert_eq!(blobs["type_id"], DICT_BLOBS_TYPE_ID);
    assert_eq!(
        blobs["row"]["stored_bytes"],
        serde_json::json!([255, 0, 97])
    );
    assert_eq!(blobs["row"]["full_len"], 3);
    assert_eq!(blobs["row"]["truncated"], false);
    assert_eq!(blobs["row"]["full_sha256"], serde_json::Value::Null);
}

#[test]
fn a_size_report_accounts_for_the_captured_file() {
    let (_directory, segment) = dictionary_segment();
    let mut output = Vec::new();
    sizes(&mut output, true, &segment).expect("render sizes");
    let lines: Vec<serde_json::Value> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("JSON line"))
        .collect();
    let summary = &lines[0];
    let captured = summary["captured_bytes"].as_u64().expect("captured bytes");
    let sections = summary["section_bytes"].as_u64().expect("section bytes");
    let overhead = summary["overhead_bytes"].as_u64().expect("overhead bytes");
    assert_eq!(captured, segment.captured_bytes());
    assert_eq!(sections + overhead, captured);
    assert_eq!(lines.last().expect("overhead row")["kind"], "overhead");
}

#[test]
fn a_broken_pipe_is_returned_as_an_output_error() {
    let mut output = BrokenPipe;
    let error = write_json_row(
        &mut output,
        Path::new("/data/one.zms"),
        1_107_001,
        &serde_json::json!({"ts": 42}),
    )
    .expect_err("closed pipe");
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

fn dictionary_segment() -> (tempfile::TempDir, kronika_reader::Segment) {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("open data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut interner = Interner::new(DictLimits::default());
    interner.intern(b"short").expect("intern string");
    interner
        .intern_blob(&[0xff, 0x00, b'a'])
        .expect("intern blob");
    let dictionary = dict::encode(interner.window()).expect("encode dictionary");
    let part = SectionBuffers::new()
        .flush(&dictionary)
        .expect("encode part")
        .expect("dictionary part");
    let segment_id = SegmentId::new(1_709_164_800_000_000).expect("segment id");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    journal.append(segment_id, &part).expect("append part");
    drop(journal);

    let reader = kronika_reader::Reader::open(directory.path()).expect("open reader");
    let listing = reader.segments(..).expect("list segment");
    assert_eq!(listing.segments.len(), 1);
    let segment = reader
        .open_segment(&listing.segments[0])
        .expect("open segment");
    (directory, segment)
}

struct BrokenPipe;

impl std::io::Write for BrokenPipe {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
