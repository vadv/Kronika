//! The compiled self-contained forensic interface representation.

use std::io::{self, ErrorKind, Read as _};
use std::sync::LazyLock;

use flate2::read::GzDecoder;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
    HeaderValue, VARY, X_CONTENT_TYPE_OPTIONS,
};
use hyper::{Response, StatusCode};

use crate::WebBody;
use crate::encoding::{ContentCoding, etag_matches};

const UI_GZIP: &[u8] = include_bytes!("../ui/kronika-ui.html.gz");
const UI_GZIP_ETAG: &str = env!("KRONIKA_UI_GZIP_ETAG");
const UI_IDENTITY_ETAG: &str = env!("KRONIKA_UI_IDENTITY_ETAG");
const UI_CSP: &str = env!("KRONIKA_UI_CSP");
const UI_GZIP_LEN: &str = env!("KRONIKA_UI_GZIP_LEN");
const UI_IDENTITY_LEN: &str = env!("KRONIKA_UI_IDENTITY_LEN");
const UI_VARY: &str = "Authorization, Accept-Encoding";
static UI_IDENTITY: LazyLock<Result<Box<[u8]>, String>> =
    LazyLock::new(|| load_identity().map_err(|error| error.to_string()));

pub(crate) fn initialize() -> io::Result<()> {
    identity_bytes().map(|_bytes| ())
}

pub(crate) fn is_path(path: &str) -> bool {
    matches!(path, "/" | "/index.html")
}

pub(crate) fn response(
    head: bool,
    if_none_match: Option<&str>,
    coding: ContentCoding,
) -> io::Result<Response<WebBody>> {
    let (bytes, length, etag) = match coding {
        ContentCoding::Identity => (identity_bytes()?, UI_IDENTITY_LEN, UI_IDENTITY_ETAG),
        ContentCoding::Gzip => (UI_GZIP, UI_GZIP_LEN, UI_GZIP_ETAG),
    };
    let not_modified = if_none_match.is_some_and(|offered| etag_matches(offered, etag));
    let body = if head || not_modified {
        Bytes::new()
    } else {
        Bytes::from_static(bytes)
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
    if let Some(content_encoding) = coding.header() {
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static(content_encoding));
    }
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static(length));
    headers.insert(ETAG, HeaderValue::from_static(etag));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private,no-cache"));
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(UI_CSP));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    set_vary(&mut response);
    Ok(response)
}

fn identity_bytes() -> io::Result<&'static [u8]> {
    match &*UI_IDENTITY {
        Ok(bytes) => Ok(bytes),
        Err(message) => Err(io::Error::new(ErrorKind::InvalidData, message.as_str())),
    }
}

fn load_identity() -> io::Result<Box<[u8]>> {
    let expected_len = UI_IDENTITY_LEN.parse::<usize>().map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid embedded UI identity length: {error}"),
        )
    })?;
    decode_identity(UI_GZIP, expected_len)
}

fn decode_identity(gzip: &[u8], expected_len: usize) -> io::Result<Box<[u8]>> {
    let mut identity = Vec::with_capacity(expected_len);
    GzDecoder::new(gzip).read_to_end(&mut identity)?;
    if identity.len() != expected_len {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "embedded UI identity length is {}, expected {expected_len}",
                identity.len()
            ),
        ));
    }
    Ok(identity.into_boxed_slice())
}

pub(crate) fn set_vary(response: &mut Response<WebBody>) {
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static(UI_VARY));
}

#[cfg(test)]
mod tests;
