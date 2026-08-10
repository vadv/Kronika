use http_body_util::BodyExt as _;
use hyper::StatusCode;
use hyper::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, ETAG, HeaderMap, HeaderValue, VARY,
};

use super::{UI_GZIP, accepts_gzip, response};

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
    assert!(get.headers().contains_key(CONTENT_SECURITY_POLICY));
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
fn absent_gzip_and_wildcard_encodings_select_the_embedded_representation() {
    assert!(accepts_gzip(&HeaderMap::new()));
    for value in ["gzip", "br, GZip; q=0.4", "*;q=1", "gzip;q=0, *;q=1"] {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_str(value).expect("encoding"),
        );
        let expected = value != "gzip;q=0, *;q=1";
        assert_eq!(accepts_gzip(&headers), expected, "{value}");
    }
}

#[test]
fn disabled_or_unlisted_gzip_is_not_acceptable() {
    for value in ["", "identity", "br", "gzip;q=0", "*;q=0", "gzip;q=2"] {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_str(value).expect("encoding"),
        );
        assert!(!accepts_gzip(&headers), "{value}");
    }
}

#[test]
fn committed_bytes_have_the_reproducible_gzip_header() {
    assert_eq!(UI_GZIP.get(..4), Some([0x1f, 0x8b, 8, 0].as_slice()));
    assert_eq!(UI_GZIP.get(4..8), Some([0, 0, 0, 0].as_slice()));
}
