use std::io::{Read as _, Seek as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};

use http_body_util::BodyExt as _;
use hyper::StatusCode;
use hyper::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, VARY,
};
use kronika_dump::{RangeError, SliceError, SliceRange, UtcSecond};

use super::{
    ExportError, PreparedExport, build, filename, parse, prepared_response, response_with,
};

const SECOND: i64 = 1_709_164_800;
const MICROS: i64 = SECOND * 1_000_000;

fn range(from: i64, to: i64) -> SliceRange {
    SliceRange::new(
        UtcSecond::from_unix_seconds(from).expect("valid first second"),
        UtcSecond::from_unix_seconds(to).expect("valid last second"),
    )
    .expect("ordered range")
}

fn test_build_failure() -> ExportError {
    ExportError::Temporary(std::io::Error::other("test export preparation"))
}

fn test_prepared_export() -> PreparedExport {
    let mut file = tempfile::tempfile().expect("temporary HTML");
    file.write_all(b"x").expect("write HTML");
    file.rewind().expect("rewind HTML");
    PreparedExport {
        file,
        len: 1,
        filename: "kronika-test.html".to_owned(),
    }
}

fn poll_ready<F: Future>(future: Pin<&mut F>) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    match future.poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("busy export response must not queue"),
    }
}

async fn assert_error_response(
    response: hyper::Response<crate::WebBody>,
    status: StatusCode,
    body: &[u8],
) {
    assert_eq!(response.status(), status);
    let actual = response
        .into_body()
        .collect()
        .await
        .expect("error response")
        .to_bytes();
    assert_eq!(actual.as_ref(), body);
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

#[tokio::test]
async fn a_second_export_is_refused_immediately_without_starting_a_build() {
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let builds = Arc::new(AtomicUsize::new(0));
    let (started_send, started_receive) = tokio::sync::oneshot::channel();
    let (release_send, release_receive) = mpsc::channel();
    let first_gate = Arc::clone(&gate);
    let first_builds = Arc::clone(&builds);
    let first = tokio::spawn(async move {
        response_with(first_gate, move || {
            first_builds.fetch_add(1, Ordering::SeqCst);
            started_send.send(()).expect("test waits for build");
            release_receive.recv().expect("test releases build");
            Err(test_build_failure())
        })
        .await
    });
    started_receive.await.expect("build started");

    let second_builds = Arc::clone(&builds);
    let mut second = Box::pin(response_with(Arc::clone(&gate), move || {
        second_builds.fetch_add(1, Ordering::SeqCst);
        Err(test_build_failure())
    }));
    let busy = poll_ready(second.as_mut());
    assert_error_response(
        busy,
        StatusCode::SERVICE_UNAVAILABLE,
        br#"{"error":"export_busy"}"#,
    )
    .await;
    assert_eq!(builds.load(Ordering::SeqCst), 1);

    release_send.send(()).expect("release first build");
    assert_eq!(
        first.await.expect("first response task").status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn caller_cancellation_does_not_release_a_running_build() {
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let builds = Arc::new(AtomicUsize::new(0));
    let (started_send, started_receive) = tokio::sync::oneshot::channel();
    let (release_send, release_receive) = mpsc::channel();
    let first_gate = Arc::clone(&gate);
    let first_builds = Arc::clone(&builds);
    let first = tokio::spawn(async move {
        response_with(first_gate, move || {
            first_builds.fetch_add(1, Ordering::SeqCst);
            started_send.send(()).expect("test waits for build");
            release_receive.recv().expect("test releases build");
            Err(test_build_failure())
        })
        .await
    });
    started_receive.await.expect("build started");
    first.abort();
    assert!(
        first
            .await
            .expect_err("caller task is cancelled")
            .is_cancelled()
    );

    let second_builds = Arc::clone(&builds);
    let mut second = Box::pin(response_with(Arc::clone(&gate), move || {
        second_builds.fetch_add(1, Ordering::SeqCst);
        Err(test_build_failure())
    }));
    let busy = poll_ready(second.as_mut());
    assert_error_response(
        busy,
        StatusCode::SERVICE_UNAVAILABLE,
        br#"{"error":"export_busy"}"#,
    )
    .await;
    assert_eq!(builds.load(Ordering::SeqCst), 1);

    release_send.send(()).expect("release abandoned build");
    let released = Arc::clone(&gate)
        .acquire_owned()
        .await
        .expect("export gate remains open");
    drop(released);
    let after_builds = Arc::clone(&builds);
    let after = response_with(Arc::clone(&gate), move || {
        after_builds.fetch_add(1, Ordering::SeqCst);
        Err(test_build_failure())
    })
    .await;
    assert_eq!(after.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(builds.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn admission_is_released_after_success_failure_and_panic() {
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let builds = Arc::new(AtomicUsize::new(0));

    let successful_builds = Arc::clone(&builds);
    let successful = response_with(Arc::clone(&gate), move || {
        successful_builds.fetch_add(1, Ordering::SeqCst);
        Ok(test_prepared_export())
    })
    .await;
    assert_eq!(successful.status(), StatusCode::OK);
    assert_eq!(
        successful
            .into_body()
            .collect()
            .await
            .expect("successful response")
            .to_bytes(),
        "x"
    );

    let failed_builds = Arc::clone(&builds);
    let failed = response_with(Arc::clone(&gate), move || {
        failed_builds.fetch_add(1, Ordering::SeqCst);
        Err(test_build_failure())
    })
    .await;
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let panicked_builds = Arc::clone(&builds);
    let panicked = response_with(Arc::clone(&gate), move || {
        panicked_builds.fetch_add(1, Ordering::SeqCst);
        panic!("test export preparation panic")
    })
    .await;
    assert_error_response(
        panicked,
        StatusCode::INTERNAL_SERVER_ERROR,
        br#"{"error":"export_failed"}"#,
    )
    .await;

    let final_builds = Arc::clone(&builds);
    let final_response = response_with(gate, move || {
        final_builds.fetch_add(1, Ordering::SeqCst);
        Err(test_build_failure())
    })
    .await;
    assert_eq!(final_response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(builds.load(Ordering::SeqCst), 4);
}

#[test]
fn a_small_recording_becomes_a_standalone_offline_html_with_the_slice_identity() {
    let mut fixture = crate::tests::artifacts::Fixture::new();
    let first_row = MICROS + 123_456;
    fixture.append_process_gauge_rows(&[(first_row, 42, 1_024, "postgres")]);
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
    assert!(text.contains(&format!(
        "new KronikaReportWasm.ReportSession(\"{MICROS}\","
    )));
    assert_ne!(
        MICROS, first_row,
        "fixture identity must differ from min_ts"
    );
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
fn export_does_not_need_write_access_to_the_recording_root() {
    let mut fixture = crate::tests::artifacts::Fixture::new();
    fixture.append_process_gauge_rows(&[(MICROS, 42, 1_024, "postgres")]);
    fixture.finish();

    let original = std::fs::metadata(fixture.root())
        .expect("recording root metadata")
        .permissions();
    let mut read_only = original.clone();
    read_only.set_mode(original.mode() & !0o222);
    std::fs::set_permissions(fixture.root(), read_only).expect("make recording root read-only");
    let result = build(fixture.root(), range(SECOND, SECOND));
    std::fs::set_permissions(fixture.root(), original).expect("restore recording permissions");

    let prepared = result.expect("build export without recording-root writes");
    assert!(prepared.len > 0);
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
    let response = super::response(
        fixture.root().to_path_buf(),
        range(SECOND, SECOND),
        Arc::new(tokio::sync::Semaphore::new(1)),
    )
    .await;
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
