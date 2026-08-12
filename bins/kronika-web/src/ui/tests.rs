use http_body_util::BodyExt as _;
use hyper::StatusCode;
use hyper::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
    HeaderValue, VARY,
};

use flate2::read::GzDecoder;
use std::io::Read as _;

use super::{UI_CSP, UI_GZIP, response};

#[tokio::test]
async fn get_and_head_share_the_exact_gzip_representation_headers() {
    let get = response(false, None);
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(
        get.headers().get(CONTENT_TYPE),
        Some(&HeaderValue::from_static("text/html; charset=utf-8"))
    );
    assert_eq!(
        get.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    assert_eq!(
        get.headers().get(CONTENT_LENGTH),
        Some(&HeaderValue::from_str(&UI_GZIP.len().to_string()).expect("content length"))
    );
    assert_eq!(
        get.headers().get(CACHE_CONTROL),
        Some(&HeaderValue::from_static("private,no-cache"))
    );
    assert_eq!(
        get.headers().get(VARY),
        Some(&HeaderValue::from_static("Authorization, Accept-Encoding"))
    );
    assert_eq!(
        get.headers()
            .get(CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').any(|directive| {
                directive
                    .trim()
                    .split_ascii_whitespace()
                    .eq(["form-action", "'none'"])
            })),
        Some(true)
    );
    assert!(get.headers().contains_key(ETAG));
    assert_eq!(
        get.into_body()
            .collect()
            .await
            .expect("GET body")
            .to_bytes(),
        UI_GZIP
    );

    let head = response(true, None);
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers().get(CONTENT_LENGTH),
        Some(&HeaderValue::from_str(&UI_GZIP.len().to_string()).expect("content length"))
    );
    assert!(
        head.into_body()
            .collect()
            .await
            .expect("HEAD body")
            .to_bytes()
            .is_empty()
    );
}

#[tokio::test]
async fn matching_entity_tags_return_the_same_empty_304_representation() {
    let current = response(false, None)
        .headers()
        .get(ETAG)
        .expect("ETag")
        .to_str()
        .expect("ASCII ETag")
        .to_owned();
    for offered in [
        current.clone(),
        format!("W/{current}"),
        format!("\"old\", {current}"),
        "*".to_owned(),
    ] {
        let not_modified = response(false, Some(&offered));
        assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED, "{offered}");
        assert_eq!(
            not_modified
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok()),
            Some(current.as_str())
        );
        assert_eq!(
            not_modified.headers().get(CONTENT_ENCODING),
            Some(&HeaderValue::from_static("gzip"))
        );
        assert_eq!(
            not_modified.headers().get(CONTENT_LENGTH),
            Some(&HeaderValue::from_str(&UI_GZIP.len().to_string()).expect("content length"))
        );
        assert!(
            not_modified
                .into_body()
                .collect()
                .await
                .expect("304 body")
                .to_bytes()
                .is_empty()
        );
    }
    assert_eq!(response(false, Some("\"old\"")).status(), StatusCode::OK);
}

#[test]
fn committed_bytes_have_the_reproducible_gzip_header() {
    assert_eq!(UI_GZIP.get(..4), Some([0x1f, 0x8b, 8, 0].as_slice()));
    assert_eq!(UI_GZIP.get(4..8), Some([0, 0, 0, 0].as_slice()));
}

#[test]
fn content_security_policy_allows_both_inline_scripts() {
    let mut html = String::new();
    GzDecoder::new(UI_GZIP)
        .read_to_string(&mut html)
        .expect("decode embedded UI");
    let mut scripts = Vec::new();
    let mut tail = html.as_str();
    while let Some(start) = tail.find("<script>") {
        let body = tail
            .get(start..)
            .and_then(|script| script.strip_prefix("<script>"))
            .expect("script starts at a UTF-8 boundary");
        let (script, rest) = body
            .split_once("</script>")
            .expect("complete inline script");
        scripts.push(script);
        tail = rest;
    }
    assert_eq!(scripts.len(), 2);
    let script_sources = UI_CSP
        .split(';')
        .find(|directive| directive.trim_start().starts_with("script-src "))
        .expect("script-src directive")
        .split_ascii_whitespace()
        .skip(1)
        .collect::<Vec<_>>();
    assert_eq!(script_sources.len(), 2);
    assert!(
        script_sources
            .iter()
            .all(|source| source.starts_with("'sha256-") && source.ends_with('\''))
    );
}
