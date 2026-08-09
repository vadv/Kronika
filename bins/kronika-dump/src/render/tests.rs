use std::path::Path;

use kronika_format::DictLimits;
use kronika_index::{BuildError, IdentityValue, IndexError, Number, Observation, Sample};
use kronika_layout::{DataRoot, LayoutLimits, SegmentId};
use kronika_reader::{BlobEntry, Resolved};
use kronika_registry::instance_metadata::{Environment, InstanceMetadataV1};
use kronika_registry::os_psi::OsPsi;
use kronika_registry::{DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};

use crate::DumpError;

use super::{
    dictionary_json, index, index_identity, index_number, index_observation, percent, section,
    sizes, write_json_row,
};

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
fn index_numbers_that_javascript_cannot_represent_are_decimal_strings() {
    assert_eq!(
        index_number(Number::I64(i64::MIN)),
        serde_json::json!(i64::MIN.to_string())
    );
    assert_eq!(
        index_number(Number::U64(u64::MAX)),
        serde_json::json!(u64::MAX.to_string())
    );
    assert_eq!(index_number(Number::F64(1.5)), serde_json::json!(1.5));
}

#[test]
fn index_observations_use_js_safe_counts_timestamps_and_durations() {
    let rendered = index_observation(&Observation {
        count: u64::MAX,
        first: Some(Sample {
            ts: i64::MIN,
            value: Number::I64(i64::MIN),
        }),
        last: Some(Sample {
            ts: i64::MAX,
            value: Number::U64(u64::MAX),
        }),
        nonnegative_delta: Some(Number::U64(u64::MAX)),
        observed_us: u64::MAX,
    });
    assert_eq!(rendered["count"], u64::MAX.to_string());
    assert_eq!(rendered["first"]["ts"], i64::MIN.to_string());
    assert_eq!(rendered["first"]["value"], i64::MIN.to_string());
    assert_eq!(rendered["last"]["ts"], i64::MAX.to_string());
    assert_eq!(rendered["last"]["value"], u64::MAX.to_string());
    assert_eq!(rendered["nonnegative_delta"], u64::MAX.to_string());
    assert_eq!(rendered["observed_us"], u64::MAX.to_string());
}

#[test]
fn index_blob_identity_keeps_exact_bytes_and_metadata() {
    let rendered = index_identity(&IdentityValue::Blob {
        stored_bytes: vec![0xff, 0, b'a'],
        full_len: u64::MAX,
        truncated: true,
        full_sha256: Some([0xa5; 32]),
    });
    assert_eq!(rendered["representation"], "blob");
    assert_eq!(rendered["stored_bytes"]["representation"], "bytes");
    assert_eq!(
        rendered["stored_bytes"]["bytes"],
        serde_json::json!([255, 0, 97])
    );
    assert_eq!(rendered["full_len"], u64::MAX.to_string());
    assert_eq!(rendered["truncated"], true);
    assert_eq!(
        rendered["full_sha256"],
        "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5"
    );
}

#[test]
fn index_dump_uses_the_generic_health_section() {
    let (_directory, segment) = dictionary_segment();
    let mut output = Vec::new();
    index(&mut output, true, &segment).expect("dump index");
    let line: serde_json::Value = serde_json::from_slice(&output).expect("index JSON line");
    assert_eq!(line["kind"], "object");
    assert_eq!(line["type_id"], kronika_index::DERIVED_HEALTH_TYPE_ID);
    assert_eq!(line["section"], "health");
    assert_eq!(line["identity"], serde_json::json!([]));
    assert_eq!(line["observations"][0]["count"], "0");

    let mut output = Vec::new();
    index(&mut output, false, &segment).expect("dump index table");
    let output = String::from_utf8(output).expect("UTF-8 index table");
    assert!(output.contains("sections=1  objects=1  series=1"));
    assert!(output.contains("health"));
}

#[test]
fn index_json_keeps_one_derived_health_point_per_psi_snapshot() {
    let (_directory, segment) = health_segment();
    let mut output = Vec::new();
    index(&mut output, true, &segment).expect("dump index");
    let points: Vec<serde_json::Value> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("index JSON line"))
        .filter(|line: &serde_json::Value| line["kind"] == "point")
        .collect();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0]["ts"], "1000000");
    assert_eq!(points[0]["health"], serde_json::Value::Null);
    assert_eq!(points[1]["ts"], "2000000");
    assert_eq!(points[1]["health"], 75);
}

#[test]
fn index_failures_keep_their_typed_dump_error_variants() {
    let build: DumpError = BuildError::UnresolvedIdentity {
        type_id: 42,
        column: "identity",
        id: 9,
    }
    .into();
    assert!(matches!(build, DumpError::Build(_)));

    let index: DumpError = IndexError::BadLayout.into();
    assert!(matches!(index, DumpError::Index(IndexError::BadLayout)));
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
    let mut output = FailingWriter {
        kind: std::io::ErrorKind::BrokenPipe,
        message: "closed pipe",
    };
    let error = write_json_row(
        &mut output,
        Path::new("/data/one.zms"),
        1_107_001,
        &serde_json::json!({"ts": 42}),
    )
    .expect_err("closed pipe");
    assert!(matches!(
        error,
        DumpError::Output(problem) if problem.kind() == std::io::ErrorKind::BrokenPipe
    ));
}

#[test]
fn a_non_pipe_failure_is_reported_as_an_output_write() {
    let mut output = FailingWriter {
        kind: std::io::ErrorKind::Other,
        message: "synthetic failure",
    };
    let error = write_json_row(
        &mut output,
        Path::new("/data/one.zms"),
        1_107_001,
        &serde_json::json!({"ts": 42}),
    )
    .expect_err("failed write");
    assert_eq!(error.to_string(), "write output: synthetic failure");
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

fn health_segment() -> (tempfile::TempDir, kronika_reader::Segment) {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("open data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut sections = SectionBuffers::new();
    sections
        .push(InstanceMetadataV1 {
            ts: Ts(1_000_000),
            hostname: StrId(1),
            kernel_version: StrId(2),
            environment: Environment::Machine.as_u8(),
            clock_ticks_per_sec: 100,
            page_size_bytes: 4096,
            boot_id: StrId(3),
            btime: Ts(0),
        })
        .expect("buffer instance metadata");
    for (ts, totals) in [(1_000_000, [0, 0, 0]), (2_000_000, [250_000, 0, 0])] {
        for (resource, some_total) in totals.into_iter().enumerate() {
            sections
                .push(OsPsi {
                    ts: Ts(ts),
                    resource: u8::try_from(resource).expect("resource id"),
                    some_avg10: 0.0,
                    some_avg60: 0.0,
                    some_avg300: 0.0,
                    some_total,
                    full_avg10: None,
                    full_avg60: None,
                    full_avg300: None,
                    full_total: None,
                    scope: 0,
                })
                .expect("buffer PSI row");
        }
    }
    let part = sections
        .flush(&[])
        .expect("encode part")
        .expect("metric part");
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

struct FailingWriter {
    kind: std::io::ErrorKind,
    message: &'static str,
}

impl std::io::Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(self.kind, self.message))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
