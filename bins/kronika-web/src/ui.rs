//! The compiled self-contained forensic interface representation.

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, ETAG, HeaderMap, HeaderValue, VARY, X_CONTENT_TYPE_OPTIONS,
};
use hyper::{Response, StatusCode};

use crate::WebBody;

const UI_GZIP: &[u8] = include_bytes!("../ui/kronika-ui.html.gz");
const UI_ETAG: &str = env!("KRONIKA_UI_ETAG");
const UI_CSP: &str = env!("KRONIKA_UI_CSP");
const UI_GZIP_LEN: &str = env!("KRONIKA_UI_GZIP_LEN");
const UI_VARY: &str = "Authorization, Accept-Encoding";

pub(crate) fn is_path(path: &str) -> bool {
    matches!(path, "/" | "/index.html")
}

pub(crate) fn accepts_gzip(headers: &HeaderMap) -> bool {
    let mut saw_header = false;
    let mut gzip = None;
    let mut wildcard = None;
    for value in headers.get_all(ACCEPT_ENCODING) {
        saw_header = true;
        let Ok(value) = value.to_str() else {
            return false;
        };
        for item in value.split(',') {
            let coding = item.split(';').next().unwrap_or_default().trim();
            if coding.is_empty() {
                continue;
            }
            let quality = quality(item).unwrap_or(0.0);
            if coding.eq_ignore_ascii_case("gzip") {
                gzip = Some(quality);
            } else if coding == "*" {
                wildcard = Some(quality);
            }
        }
    }
    !saw_header || gzip.or(wildcard).is_some_and(|quality| quality > 0.0)
}

fn quality(item: &str) -> Option<f32> {
    let mut quality = 1.0_f32;
    for parameter in item.split(';').skip(1) {
        let (name, value) = parameter.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("q") {
            return None;
        }
        quality = value.trim().parse().ok()?;
        if !(0.0..=1.0).contains(&quality) {
            return None;
        }
    }
    Some(quality)
}

pub(crate) fn response(head: bool, if_none_match: Option<&str>) -> Response<WebBody> {
    let not_modified = if_none_match.is_some_and(|offered| etag_matches(offered, UI_ETAG));
    let body = if head || not_modified {
        Bytes::new()
    } else {
        Bytes::from_static(UI_GZIP)
    };
    let mut response = Response::new(
        Full::new(body)
            .map_err(crate::body::BodyError::from)
            .boxed_unsync(),
    );
    *response.status_mut() = if not_modified {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static(UI_GZIP_LEN));
    headers.insert(ETAG, HeaderValue::from_static(UI_ETAG));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private,no-cache"));
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(UI_CSP));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    set_vary(&mut response);
    response
}

pub(crate) fn set_vary(response: &mut Response<WebBody>) {
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static(UI_VARY));
}

fn etag_matches(offered: &str, current: &str) -> bool {
    offered.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate).trim() == current
    })
}

#[cfg(test)]
mod tests;
