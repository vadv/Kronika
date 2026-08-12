//! The compiled self-contained forensic interface representation.

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
    HeaderValue, VARY, X_CONTENT_TYPE_OPTIONS,
};
use hyper::{Response, StatusCode};

use crate::WebBody;
use crate::encoding::etag_matches;

const UI_GZIP: &[u8] = include_bytes!("../ui/kronika-ui.html.gz");
const UI_ETAG: &str = env!("KRONIKA_UI_ETAG");
const UI_CSP: &str = env!("KRONIKA_UI_CSP");
const UI_GZIP_LEN: &str = env!("KRONIKA_UI_GZIP_LEN");
const UI_VARY: &str = "Authorization, Accept-Encoding";

pub(crate) fn is_path(path: &str) -> bool {
    matches!(path, "/" | "/index.html")
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

#[cfg(test)]
mod tests;
