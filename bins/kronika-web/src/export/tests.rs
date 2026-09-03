use std::io::{Read as _, Seek as _, Write as _};

use http_body_util::BodyExt as _;
use hyper::StatusCode;
use hyper::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, VARY,
};
use kronika_dump::{RangeError, SliceError, SliceRange, UtcSecond};

use super::{ExportError, PreparedExport, build, filename, parse, prepared_response};

const SECOND: i64 = 1_709_164_800;
const MICROS: i64 = SECOND * 1_000_000;

fn range(from: i64, to: i64) -> SliceRange {
    SliceRange::new(
        UtcSecond::from_unix_seconds(from).expect("valid first second"),
        UtcSecond::from_unix_seconds(to).expect("valid last second"),
    )
    .expect("ordered range")
}

#[test]
fn signed_whole_seconds_and_order_are_strict() {
    let parsed = parse("from=-1&to=0").expect("signed seconds");
    assert_eq!(parsed.from().unix_seconds(), -1);
    assert_eq!(parsed.to().unix_seconds(), 0);
    assert!(matches!(
        parse("from=1.5&to=2"),
        Err(crate::route::RouteError::BadParameter(name)) if name == "from"
    ));
    assert!(matches!(
        parse("from=2&to=1"),
        Err(crate::route::RouteError::BadParameter(name)) if name == "from"
    ));
}

#[test]
fn missing_duplicate_unknown_and_overflowing_bounds_are_named() {
    for (query, expected) in [
        ("", "from"),
        ("to=0", "from"),
        ("from=0", "to"),
        ("from=0&from=1&to=2", "from"),
        ("from=0&to=1&format=html", "format"),
        ("from=-62167219201&to=0", "from"),
        ("from=0&to=253402300800", "to"),
        ("from=9223372036854775807&to=1", "from"),
        ("from=0&to=9223372036854775807", "to"),
    ] {
        assert!(
            matches!(parse(query), Err(crate::route::RouteError::BadParameter(name)) if name == expected),
            "{query}"
        );
    }
}

#[test]
fn filename_uses_both_inclusive_utc_seconds() {
    assert_eq!(
        filename(range(SECOND, SECOND + 3_599)).as_deref(),
        Some("kronika-2024-02-29-000000-2024-02-29-005959-utc.html")
    );
}

#[test]
fn failures_have_stable_status_codes_without_internal_text() {
    let empty = ExportError::Slice(SliceError::NoRowsInRequestedRange);
    assert_eq!(
        empty.status_and_code(),
        (StatusCode::NOT_FOUND, "export_empty")
    );
    let bad_range = ExportError::Slice(SliceError::Range(RangeError::Reversed));
    assert_eq!(
        bad_range.status_and_code(),
        (StatusCode::BAD_REQUEST, "bad_parameter")
    );
    let temporary = ExportError::Temporary(std::io::Error::other("/private/storage/path"));
    assert_eq!(
        temporary.status_and_code(),
        (StatusCode::INTERNAL_SERVER_ERROR, "export_failed")
    );
}

#[tokio::test]
async fn response_is_an_identity_html_attachment_with_exact_length() {
    let mut file = tempfile::tempfile().expect("temporary HTML");
    file.write_all(b"<!doctype html><title>Kronika</title>")
        .expect("write HTML");
    file.rewind().expect("rewind HTML");
    let response = prepared_response(PreparedExport {
        file,
        len: 37,
        filename: "kronika-2024-02-29-000000-2024-02-29-005959-utc.html".to_owned(),
    })
    .expect("valid response headers");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(response.headers()[CONTENT_LENGTH], "37");
    assert_eq!(
        response.headers()[CONTENT_DISPOSITION],
        "attachment; filename=\"kronika-2024-02-29-000000-2024-02-29-005959-utc.html\""
    );
    assert_eq!(response.headers()[CACHE_CONTROL], "private,no-store");
    assert_eq!(
        response.headers()[VARY],
        "Authorization, Cookie, Accept-Encoding"
    );
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let data = frame
            .expect("response frame")
            .into_data()
            .expect("data frame");
        assert!(data.len() <= super::BODY_CHUNK_BYTES);
        bytes.extend_from_slice(&data);
    }
    assert_eq!(&bytes, b"<!doctype html><title>Kronika</title>");
}

#[tokio::test]
async fn streaming_never_exceeds_the_declared_length_and_reports_short_files() {
    let mut longer = tempfile::tempfile().expect("temporary HTML");
    longer.write_all(b"abcdef").expect("write HTML");
    longer.rewind().expect("rewind HTML");
    let response = prepared_response(PreparedExport {
        file: longer,
        len: 3,
        filename: "kronika-2024-02-29-000000-2024-02-29-000002-utc.html".to_owned(),
    })
    .expect("valid response headers");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .expect("bounded response")
            .to_bytes(),
        "abc"
    );

    let mut shorter = tempfile::tempfile().expect("temporary HTML");
    shorter.write_all(b"abc").expect("write HTML");
    shorter.rewind().expect("rewind HTML");
    let response = prepared_response(PreparedExport {
        file: shorter,
        len: 4,
        filename: "kronika-2024-02-29-000000-2024-02-29-000003-utc.html".to_owned(),
    })
    .expect("valid response headers");
    assert!(response.into_body().collect().await.is_err());
}

#[test]
fn a_small_recording_becomes_a_standalone_offline_html_file() {
    let mut fixture = crate::tests::artifacts::Fixture::new();
    fixture.append_process_gauge_rows(&[(MICROS, 42, 1_024, "postgres")]);
    fixture.finish();

    let mut prepared = build(fixture.root(), range(SECOND, SECOND)).expect("build export");
    let mut html = Vec::new();
    prepared
        .file
        .read_to_end(&mut html)
        .expect("read generated HTML");
    assert_eq!(prepared.len, html.len() as u64);
    assert!(html.starts_with(b"<!doctype html>"));
    let text = std::str::from_utf8(&html).expect("UTF-8 HTML");
    assert!(text.contains("__KRONIKA_REPORT_RUNTIME__"));
    assert!(text.contains("WebAssembly.compile"));
    for external in [
        "src=\"http:",
        "src=\"https:",
        "href=\"http:",
        "href=\"https:",
    ] {
        assert!(!text.contains(external), "external reference {external}");
    }
}

#[test]
fn an_empty_selected_second_is_a_typed_error() {
    let fixture = crate::tests::artifacts::Fixture::new();
    let error = build(fixture.root(), range(SECOND, SECOND)).expect_err("empty export");
    assert!(matches!(
        error,
        ExportError::Slice(SliceError::NoRowsInRequestedRange)
    ));
}

#[tokio::test]
async fn an_empty_selected_second_returns_only_the_stable_public_error() {
    let fixture = crate::tests::artifacts::Fixture::new();
    let private_root = fixture.root().display().to_string();
    let response = super::response(fixture.root().to_path_buf(), range(SECOND, SECOND)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("error response")
        .to_bytes();
    assert_eq!(&body[..], br#"{"error":"export_empty"}"#);
    assert!(!String::from_utf8_lossy(&body).contains(&private_root));
}
