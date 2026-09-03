use std::error::Error as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kronika_index::{Index, SeriesBlock};
use kronika_layout::SegmentId;
use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};

use super::{GenerateError, TEMP_PREFIX, generate, isolated_index, segment_id};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const ZMS: &[u8] = include_bytes!("../tests/fixtures/standalone.zms");
const IDX: &[u8] = include_bytes!("../tests/fixtures/standalone.idx");

fn fixture(directory: &Path) -> PathBuf {
    let input = directory.join(format!("{SEGMENT_ID}.zms"));
    std::fs::write(&input, ZMS).expect("write standalone fixture");
    input
}

fn temporary_names(directory: &Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .expect("read temporary directory")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(TEMP_PREFIX))
        .collect()
}

fn script_blocks(html: &str) -> Option<usize> {
    let mut tail = html;
    let mut count = 0;
    while let Some((_head, body)) = tail.split_once("<script>") {
        let (_script, after) = body.split_once("</script>")?;
        count += 1;
        tail = after;
    }
    Some(count)
}

#[test]
fn canonical_filename_binds_the_explicit_segment_identity() {
    for (name, expected) in [
        ("0.zms", 0),
        ("-1.zms", -1),
        ("1709164800000000.zms", SEGMENT_ID),
    ] {
        assert_eq!(
            segment_id(Path::new(name)).expect("canonical input").get(),
            expected
        );
    }

    for name in [
        "",
        ".zms",
        "+1.zms",
        "-0.zms",
        "00.zms",
        "01.zms",
        "1",
        "1.ZMS",
        "1.zms.extra",
        "9223372036854775808.zms",
    ] {
        assert!(
            matches!(
                segment_id(Path::new(name)),
                Err(GenerateError::InvalidInputName(_))
            ),
            "{name:?}"
        );
    }
    assert!(matches!(
        segment_id(Path::new("9223372036854775807.zms")),
        Err(GenerateError::Layout(_))
    ));
}

#[test]
fn isolated_builder_produces_the_committed_canonical_index() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("fixture segment id");
    let (index, configured_sources) = isolated_index(
        segment_id,
        ZMS.to_vec(),
        u64::try_from(ZMS.len()).expect("fixture length fits u64"),
    )
    .expect("build isolated index");
    assert_eq!(index, IDX);
    assert_eq!(configured_sources, SOURCE_OS);
    assert_eq!(
        super::configured_sources(&Index { blocks: Vec::new() }),
        SOURCE_OS
    );
    assert_eq!(
        super::configured_sources(&Index {
            blocks: vec![SeriesBlock::PostgresHealth(Vec::new())],
        }),
        SOURCE_OS | SOURCE_POSTGRESQL
    );
}

#[test]
fn report_is_self_contained_and_deterministic() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = fixture(directory.path());
    let first = directory.path().join("first.html");
    let second = directory.path().join("second.html");
    std::fs::write(&first, b"old report").expect("write existing report");
    generate(&input, &first).expect("first report");
    generate(&input, &second).expect("second report");

    let first = std::fs::read(first).expect("read first report");
    let second = std::fs::read(second).expect("read second report");
    assert_eq!(first, second);
    assert!(first.starts_with(b"<!doctype html>"));
    let html = std::str::from_utf8(&first).expect("report HTML is UTF-8");
    assert_eq!(script_blocks(html), Some(2));
    for external in [
        "src=\"http:",
        "src=\"https:",
        "src=\"//",
        "src='http:",
        "src='https:",
        "src='//",
        "href=\"http:",
        "href=\"https:",
        "href=\"//",
        "href='http:",
        "href='https:",
        "href='//",
    ] {
        assert!(!html.contains(external), "external asset {external}");
    }
    assert!(
        !first
            .windows(super::RUNTIME_MARKER.len())
            .any(|bytes| bytes == super::RUNTIME_MARKER)
    );
    assert!(
        first
            .windows(b"__KRONIKA_REPORT_RUNTIME__".len())
            .any(|bytes| bytes == b"__KRONIKA_REPORT_RUNTIME__")
    );
    assert!(html.contains("m=await WebAssembly.compile(r)"));
    assert!(html.contains("await KronikaReportWasm.initEmbedded(m)"));
    assert!(!html.contains("KronikaReportWasm.initSync"));
    assert!(!html.contains("new WebAssembly.Module"));
    for encoded in [STANDARD.encode(ZMS), STANDARD.encode(IDX)] {
        assert!(
            first
                .windows(encoded.len())
                .any(|bytes| bytes == encoded.as_bytes()),
            "embedded artifact"
        );
    }
    assert!(temporary_names(directory.path()).is_empty());
}

#[test]
fn failure_keeps_the_existing_output_and_removes_the_temporary() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join(format!("{SEGMENT_ID}.zms"));
    let output = directory.path().join("report.html");
    std::fs::write(&input, b"not a ZMS").expect("write invalid input");
    std::fs::write(&output, b"existing report").expect("write existing output");

    let error = generate(&input, &output).expect_err("invalid ZMS");
    assert!(matches!(error, GenerateError::Resource(_)));
    assert!(error.source().is_some());
    assert_eq!(
        std::fs::read(&output).expect("read existing output"),
        b"existing report"
    );
    assert!(temporary_names(directory.path()).is_empty());
}

#[test]
fn output_must_have_the_html_extension() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = fixture(directory.path());
    let output = directory.path().join("report.htm");
    assert!(matches!(
        generate(&input, &output),
        Err(GenerateError::InvalidOutputName(path)) if path == output
    ));
    assert!(!output.exists());
}
