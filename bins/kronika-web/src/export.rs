//! Authenticated construction and bounded delivery of standalone HTML exports.

use std::fs::File;
use std::io::{self, BufWriter, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use http_body_util::BodyExt as _;
use hyper::Response;
use hyper::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue, VARY};
use kronika_dump::{SliceError, SliceRange, UtcSecond, slice_to_zms};
use kronika_reader::{Reader, ReaderError};
use kronika_report::{HtmlReportError, write_html_from_file};
use tokio::sync::mpsc;

use crate::api::CachePolicy;
use crate::body::{BodyError, BodyItem, ChannelBody};
use crate::route::{MAX_QUERY_BYTES, RouteError};
use crate::{WebBody, common_headers, refused};

const FILE_BUFFER_BYTES: usize = 64 * 1_024;
const BODY_CHUNK_BYTES: usize = 8 * 1_024;

pub(crate) fn parse(query: &str) -> Result<SliceRange, RouteError> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(RouteError::BadParameter("query".to_owned()));
    }
    let mut from = None;
    let mut to = None;
    if !query.is_empty() {
        for part in query.split('&') {
            if part.is_empty() {
                return Err(RouteError::BadParameter("query".to_owned()));
            }
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            match name {
                "from" if from.is_none() => from = Some(second("from", value)?),
                "to" if to.is_none() => to = Some(second("to", value)?),
                _ => return Err(RouteError::BadParameter(name.to_owned())),
            }
        }
    }
    let from = from.ok_or_else(|| RouteError::BadParameter("from".to_owned()))?;
    let to = to.ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    SliceRange::new(from, to).map_err(|_error| RouteError::BadParameter("from".to_owned()))
}

fn second(name: &str, value: &str) -> Result<UtcSecond, RouteError> {
    let value = value
        .parse::<i64>()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
    UtcSecond::from_unix_seconds(value).map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

#[derive(Debug)]
struct PreparedExport {
    file: File,
    len: u64,
    filename: String,
}

#[derive(Debug)]
enum ExportError {
    Reader(ReaderError),
    Slice(SliceError),
    Report(HtmlReportError),
    Temporary(io::Error),
    InvalidTime,
    InvalidHeader(hyper::header::InvalidHeaderValue),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reader(error) => write!(f, "open recorded data: {error}"),
            Self::Slice(error) => write!(f, "build standalone ZMS: {error}"),
            Self::Report(error) => write!(f, "build standalone HTML: {error}"),
            Self::Temporary(error) => write!(f, "use temporary export file: {error}"),
            Self::InvalidTime => f.write_str("format export time"),
            Self::InvalidHeader(error) => {
                write!(f, "build export response header: {error}")
            }
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reader(error) => Some(error),
            Self::Slice(error) => Some(error),
            Self::Report(error) => Some(error),
            Self::Temporary(error) => Some(error),
            Self::InvalidTime => None,
            Self::InvalidHeader(error) => Some(error),
        }
    }
}

impl ExportError {
    const fn status_and_code(&self) -> (hyper::StatusCode, &'static str) {
        match self {
            Self::Slice(SliceError::NoRowsInRequestedRange) => {
                (hyper::StatusCode::NOT_FOUND, "export_empty")
            }
            Self::Slice(SliceError::Range(_)) => (hyper::StatusCode::BAD_REQUEST, "bad_parameter"),
            Self::Reader(_)
            | Self::Slice(_)
            | Self::Report(_)
            | Self::Temporary(_)
            | Self::InvalidTime
            | Self::InvalidHeader(_) => (hyper::StatusCode::INTERNAL_SERVER_ERROR, "export_failed"),
        }
    }
}

impl From<ReaderError> for ExportError {
    fn from(error: ReaderError) -> Self {
        Self::Reader(error)
    }
}

impl From<SliceError> for ExportError {
    fn from(error: SliceError) -> Self {
        Self::Slice(error)
    }
}

impl From<HtmlReportError> for ExportError {
    fn from(error: HtmlReportError) -> Self {
        Self::Report(error)
    }
}

pub(crate) async fn response(data_root: PathBuf, range: SliceRange) -> Response<WebBody> {
    match tokio::task::spawn_blocking(move || build(&data_root, range)).await {
        Ok(Ok(prepared)) => {
            prepared_response(prepared).unwrap_or_else(|error| export_failure(&error))
        }
        Ok(Err(error)) => export_failure(&error),
        Err(error) => {
            eprintln!("kronika-web: export task failed: {error}");
            refused(
                hyper::StatusCode::INTERNAL_SERVER_ERROR,
                "export_failed",
                None,
            )
        }
    }
}

fn export_failure(error: &ExportError) -> Response<WebBody> {
    eprintln!("kronika-web: export failed: {error}");
    let (status, code) = error.status_and_code();
    refused(status, code, None)
}

fn build(data_root: &Path, range: SliceRange) -> Result<PreparedExport, ExportError> {
    let reader = Reader::open(data_root)?;
    let mut html = tempfile::tempfile_in(data_root).map_err(ExportError::Temporary)?;
    let mut zms = tempfile::tempfile_in(data_root).map_err(ExportError::Temporary)?;
    let summary = slice_to_zms(&reader, range, &mut html, &mut zms)?;
    zms.flush().map_err(ExportError::Temporary)?;
    let zms_len = zms.metadata().map_err(ExportError::Temporary)?.len();
    if zms_len != summary.bytes_written {
        return Err(ExportError::Temporary(io::Error::other(
            "standalone ZMS length changed",
        )));
    }

    html.set_len(0).map_err(ExportError::Temporary)?;
    html.seek(SeekFrom::Start(0))
        .map_err(ExportError::Temporary)?;
    {
        let mut output = BufWriter::with_capacity(FILE_BUFFER_BYTES, &mut html);
        write_html_from_file(zms, summary.bytes_written, &mut output)?;
        output.flush().map_err(ExportError::Temporary)?;
    }
    let len = html.metadata().map_err(ExportError::Temporary)?.len();
    html.seek(SeekFrom::Start(0))
        .map_err(ExportError::Temporary)?;
    Ok(PreparedExport {
        file: html,
        len,
        filename: filename(range).ok_or(ExportError::InvalidTime)?,
    })
}

fn filename(range: SliceRange) -> Option<String> {
    let from = utc_filename_second(range.from())?;
    let to = utc_filename_second(range.to())?;
    Some(format!("kronika-{from}-{to}-utc.html"))
}

fn utc_filename_second(value: UtcSecond) -> Option<String> {
    DateTime::<Utc>::from_timestamp(value.unix_seconds(), 0)
        .map(|time| time.format("%Y-%m-%d-%H%M%S").to_string())
}

fn prepared_response(prepared: PreparedExport) -> Result<Response<WebBody>, ExportError> {
    let PreparedExport {
        mut file,
        len,
        filename,
    } = prepared;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(ExportError::InvalidHeader)?;
    let (sender, receiver) = mpsc::channel::<BodyItem>(8);
    let handle = tokio::task::spawn_blocking(move || stream_file(&mut file, len, &sender));
    drop(handle);

    let body = ChannelBody { receiver }.boxed_unsync();
    let mut response = Response::new(body);
    common_headers(&mut response, CachePolicy::NoStore);
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
    );
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from(len));
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, disposition);
    Ok(response)
}

fn stream_file(file: &mut File, len: u64, sender: &mpsc::Sender<BodyItem>) {
    let mut buffer = [0_u8; BODY_CHUNK_BYTES];
    let mut remaining = len;
    while remaining > 0 {
        let chunk_len =
            usize::try_from(remaining.min(BODY_CHUNK_BYTES as u64)).unwrap_or(BODY_CHUNK_BYTES);
        let read = match file.read(&mut buffer[..chunk_len]) {
            Ok(0) => {
                eprintln!(
                    "kronika-web: generated export ended {remaining} bytes before declared length"
                );
                let _sent = sender.blocking_send(Err(BodyError));
                return;
            }
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("kronika-web: read generated export: {error}");
                let _sent = sender.blocking_send(Err(BodyError));
                return;
            }
        };
        debug_assert!(
            read <= BODY_CHUNK_BYTES,
            "one export response frame must fit the bounded buffer"
        );
        let Ok(read_len) = u64::try_from(read) else {
            eprintln!("kronika-web: generated export read length does not fit u64");
            let _sent = sender.blocking_send(Err(BodyError));
            return;
        };
        remaining -= read_len;
        if sender.blocking_send(Ok(buffer[..read].to_vec())).is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "export/tests.rs"]
mod tests;
