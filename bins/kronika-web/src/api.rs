//! Preparing blocking resource reads and streaming small self-describing records.

use std::error::Error;
use std::path::Path;

use hyper::StatusCode;
use kronika_reader::{Reader, ReaderError, SegmentRef};

use crate::route::{ActiveCursor, Route};

mod catalog;
mod history;
mod hour;
mod index;
mod query;
mod render;
mod rows;
mod snapshot;

/// Cache policy applied centrally after preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CachePolicy {
    /// Mutable catalog, active data, and errors.
    NoStore,
    /// Mutable catalog or finished IDX, revalidated before reuse.
    Revalidate,
    /// Immutable finished history or rows.
    Immutable,
}

impl CachePolicy {
    pub(crate) const fn header(self) -> &'static str {
        match self {
            Self::NoStore => "private,no-store",
            Self::Revalidate => "private,no-cache",
            Self::Immutable => "private,max-age=31536000,immutable",
        }
    }
}

/// Headers known before a streamed body starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponseMeta {
    pub(crate) status: StatusCode,
    pub(crate) cache: CachePolicy,
    pub(crate) etag: Option<String>,
}

impl ResponseMeta {
    const fn ok(cache: CachePolicy) -> Self {
        Self {
            status: StatusCode::OK,
            cache,
            etag: None,
        }
    }
}

/// A prepared response whose disk/Parquet work remains on the blocking thread.
pub(crate) enum Prepared {
    Catalog(catalog::PreparedCatalog),
    Index(index::PreparedIndex),
    History(history::PreparedHistory),
    Hour(hour::PreparedHour),
    Rows(rows::PreparedRows),
    Snapshot(snapshot::PreparedSnapshot),
    Empty(ResponseMeta),
}

impl Prepared {
    /// Response status and caching, available before the first body record.
    pub(crate) fn meta(&self) -> ResponseMeta {
        match self {
            Self::Catalog(_prepared) => catalog::PreparedCatalog::meta(),
            Self::Index(prepared) => prepared.meta(),
            Self::History(prepared) => prepared.meta(),
            Self::Hour(prepared) => prepared.meta(),
            Self::Rows(prepared) => prepared.meta(),
            Self::Snapshot(prepared) => prepared.meta(),
            Self::Empty(meta) => meta.clone(),
        }
    }

    /// Emit newline-delimited JSON records until complete or the client leaves.
    pub(crate) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        match self {
            Self::Catalog(prepared) => prepared.stream(emit, cancelled),
            Self::Index(prepared) => prepared.stream(emit, cancelled),
            Self::History(prepared) => prepared.stream(emit, cancelled),
            Self::Hour(prepared) => prepared.stream(emit, cancelled),
            Self::Rows(prepared) => prepared.stream(emit, cancelled),
            Self::Snapshot(prepared) => prepared.stream(emit, cancelled),
            Self::Empty(_meta) => Ok(()),
        }
    }
}

/// Why a resource could not be prepared or streamed.
#[derive(Debug)]
pub(crate) enum ApiError {
    NoSuchSegment,
    NoSuchSection,
    NoSuchColumn(String),
    BadFilter(String),
    BadCursor,
    Unreadable(Box<dyn Error + Send + Sync>),
}

impl ApiError {
    pub(crate) const fn status(&self) -> StatusCode {
        match self {
            Self::NoSuchSegment | Self::NoSuchSection => StatusCode::NOT_FOUND,
            Self::NoSuchColumn(_) | Self::BadFilter(_) | Self::BadCursor => StatusCode::BAD_REQUEST,
            Self::Unreadable(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::NoSuchSegment => "no_such_segment",
            Self::NoSuchSection => "no_such_section",
            Self::NoSuchColumn(_) => "no_such_column",
            Self::BadFilter(_) => "bad_filter",
            Self::BadCursor => "bad_cursor",
            Self::Unreadable(_) => "unreadable",
        }
    }

    pub(crate) fn parameter(&self) -> Option<&str> {
        match self {
            Self::NoSuchColumn(column) | Self::BadFilter(column) => Some(column),
            _ => None,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSegment => write!(f, "no such segment"),
            Self::NoSuchSection => write!(f, "no such logical section"),
            Self::NoSuchColumn(column) => write!(f, "no such column {column:?}"),
            Self::BadFilter(column) => write!(f, "invalid typed filter for {column:?}"),
            Self::BadCursor => write!(f, "invalid page cursor"),
            Self::Unreadable(error) => error.fmt(f),
        }
    }
}

impl Error for ApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unreadable(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<ReaderError> for ApiError {
    fn from(error: ReaderError) -> Self {
        Self::Unreadable(Box::new(error))
    }
}

impl From<kronika_index::LoadError> for ApiError {
    fn from(error: kronika_index::LoadError) -> Self {
        Self::Unreadable(Box::new(error))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::Unreadable(Box::new(error))
    }
}

/// Perform request validation and initial I/O outside the Tokio worker.
pub(crate) fn prepare(
    root: &Path,
    sources: u32,
    route: Route,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    match route {
        Route::Catalog(window) => catalog::prepare(root, window, sources).map(Prepared::Catalog),
        Route::Index(request) => index::prepare(root, request, if_none_match),
        Route::History(request) => history::prepare(root, request).map(Prepared::History),
        Route::Hour(window) => hour::prepare(root, window, sources).map(Prepared::Hour),
        Route::Rows(request) => rows::prepare(root, request).map(Prepared::Rows),
        Route::Snapshot(request) => snapshot::prepare(root, request).map(Prepared::Snapshot),
    }
}

fn explicit_segment(root: &Path, id: i64) -> Result<(Reader, SegmentRef), ApiError> {
    let started = std::time::Instant::now();
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments(..)?;
    log_warnings(&listing.warnings);
    let segment = listing
        .segments
        .into_iter()
        .find(|segment| segment.id() == id)
        .ok_or(ApiError::NoSuchSegment)?;
    eprintln!(
        "kronika-web: segment_open id={} kind={} sections={} elapsed_us={}",
        segment.id(),
        match segment.kind() {
            kronika_reader::SegmentKind::Finished => "finished",
            kronika_reader::SegmentKind::Active => "active",
        },
        segment.sections().len(),
        started.elapsed().as_micros(),
    );
    Ok((reader, segment))
}

fn active_tail(
    current: &SegmentRef,
    after: Option<ActiveCursor>,
) -> Result<Option<SegmentRef>, ApiError> {
    let Some(after) = after else {
        return Ok(None);
    };
    if current.kind() != kronika_reader::SegmentKind::Active || current.id() != after.segment_id {
        return Err(ApiError::BadCursor);
    }
    current
        .at_active_position(after.wal_position)
        .map(Some)
        .map_err(|_error| ApiError::BadCursor)
}

fn log_warnings(warnings: &[kronika_reader::StoreWarning]) {
    for warning in warnings {
        eprintln!("kronika-web: store warning code={}", warning.reason.code());
    }
}
