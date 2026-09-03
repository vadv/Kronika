//! Preparing blocking resource reads and streaming small self-describing records.

use std::error::Error;
use std::path::Path;

use hyper::StatusCode;
use kronika_query::{QueryContext, QueryIdentity, QueryRequest, QuerySink, QueryStability};
use kronika_reader::ReaderError;
use sha2::{Digest as _, Sha256};

use crate::encoding::etag_matches;
use crate::route::Route;

/// Cache policy applied centrally after preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CachePolicy {
    /// Mutable catalog, active data, and errors.
    NoStore,
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
    const fn ok_with_etag(cache: CachePolicy, etag: Option<String>) -> Self {
        Self {
            status: StatusCode::OK,
            cache,
            etag,
        }
    }
}

/// A prepared response whose disk/Parquet work remains on the blocking thread.
pub(crate) enum Prepared {
    Query(Box<PreparedQuery>),
    Empty(ResponseMeta),
}

pub(crate) struct PreparedQuery {
    execution: kronika_query::QueryExecution,
    meta: ResponseMeta,
}

impl Prepared {
    /// Response status and caching, available before the first body record.
    pub(crate) fn meta(&self) -> ResponseMeta {
        match self {
            Self::Query(prepared) => prepared.meta.clone(),
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
            Self::Query(prepared) => {
                let mut sink = NativeSink { emit, cancelled };
                prepared.execution.stream(&mut sink).map_err(ApiError::from)
            }
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
    MixedUnits(String),
    BadFilter(String),
    BadCursor,
    BadLocator(String),
    Cancelled,
    Unreadable(Box<dyn Error + Send + Sync>),
}

impl ApiError {
    pub(crate) const fn status(&self) -> StatusCode {
        match self {
            Self::NoSuchSegment | Self::NoSuchSection => StatusCode::NOT_FOUND,
            Self::NoSuchColumn(_)
            | Self::MixedUnits(_)
            | Self::BadFilter(_)
            | Self::BadCursor
            | Self::BadLocator(_) => StatusCode::BAD_REQUEST,
            Self::Cancelled | Self::Unreadable(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::NoSuchSegment => "no_such_segment",
            Self::NoSuchSection => "no_such_section",
            Self::NoSuchColumn(_) => "no_such_column",
            Self::MixedUnits(_) => "mixed_units",
            Self::BadFilter(_) => "bad_filter",
            Self::BadCursor => "bad_cursor",
            Self::BadLocator(_) => "bad_locator",
            Self::Cancelled => "cancelled",
            Self::Unreadable(_) => "unreadable",
        }
    }

    pub(crate) fn parameter(&self) -> Option<&str> {
        match self {
            Self::NoSuchColumn(column) | Self::MixedUnits(column) | Self::BadFilter(column) => {
                Some(column)
            }
            _ => None,
        }
    }

    pub(crate) fn source_changed_during_read(&self) -> bool {
        let Self::Unreadable(error) = self else {
            return false;
        };
        let mut source: &(dyn Error + 'static) = error.as_ref();
        loop {
            if let Some(reader) = source.downcast_ref::<ReaderError>() {
                return reader.source_changed_during_read();
            }
            let Some(next) = source.source() else {
                return false;
            };
            source = next;
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSegment => write!(f, "no such segment"),
            Self::NoSuchSection => write!(f, "no such logical section"),
            Self::NoSuchColumn(column) => write!(f, "no such column {column:?}"),
            Self::MixedUnits(fields) => write!(f, "fields carry different units: {fields}"),
            Self::BadFilter(column) => write!(f, "invalid typed filter for {column:?}"),
            Self::BadCursor => write!(f, "invalid page cursor"),
            Self::BadLocator(message) => message.fmt(f),
            Self::Cancelled => write!(f, "request cancelled"),
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

impl From<kronika_query::QueryError> for ApiError {
    fn from(error: kronika_query::QueryError) -> Self {
        match error {
            kronika_query::QueryError::NoSuchSegment => Self::NoSuchSegment,
            kronika_query::QueryError::NoSuchSection => Self::NoSuchSection,
            kronika_query::QueryError::NoSuchColumn(column) => Self::NoSuchColumn(column),
            kronika_query::QueryError::MixedUnits(fields) => Self::MixedUnits(fields),
            kronika_query::QueryError::BadFilter(column) => Self::BadFilter(column),
            kronika_query::QueryError::BadCursor => Self::BadCursor,
            kronika_query::QueryError::BadLocator(message) => Self::BadLocator(message),
            kronika_query::QueryError::Cancelled => Self::Cancelled,
            kronika_query::QueryError::Unreadable(error) => Self::Unreadable(error),
            other => Self::Unreadable(Box::new(other)),
        }
    }
}

struct NativeSink<'a, E, C> {
    emit: &'a mut E,
    cancelled: &'a C,
}

impl<E, C> QuerySink for NativeSink<'_, E, C>
where
    E: FnMut(Vec<u8>) -> bool,
    C: Fn() -> bool,
{
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        (self.emit)(bytes)
    }

    fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
}

fn query_meta(metadata: kronika_query::QueryMetadata<'_>) -> ResponseMeta {
    let cache = match metadata.stability() {
        QueryStability::Mutable => CachePolicy::NoStore,
        QueryStability::Revalidate => CachePolicy::Revalidate,
        QueryStability::Immutable => CachePolicy::Immutable,
    };
    let etag = metadata.identity().and_then(|identity| match identity {
        QueryIdentity::IndexChecksum(checksum) => Some(format!("W/\"{checksum:08x}\"")),
        QueryIdentity::SegmentSet {
            resource,
            shape,
            segments,
        } => weak_dataset_etag(resource, shape, segments),
    });
    ResponseMeta::ok_with_etag(cache, etag)
}

fn prepared_query(execution: kronika_query::QueryExecution) -> Prepared {
    let meta = query_meta(execution.metadata());
    Prepared::Query(Box::new(PreparedQuery { execution, meta }))
}

/// Perform request validation and initial I/O outside the Tokio worker.
#[cfg(test)]
pub(crate) fn prepare(
    root: &Path,
    sources: u32,
    route: Route,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    prepare_with_demo(root, sources, false, route, if_none_match)
}

/// Prepare a response with the deployment identity exposed in its catalog.
pub(crate) fn prepare_with_demo(
    root: &Path,
    sources: u32,
    synthetic_demo: bool,
    route: Route,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let Route::Recorded(route) = route else {
        // Answered directly in `main.rs`.
        return Err(ApiError::NoSuchSection);
    };
    let request = route.into_query().map_err(ApiError::from)?;
    let prepared = match request {
        request @ (QueryRequest::Catalog(_)
        | QueryRequest::History(_)
        | QueryRequest::Events(_)
        | QueryRequest::RowDetail(_)
        | QueryRequest::Rows(_)) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(&context, request)
                .map(prepared_query)
                .map_err(ApiError::from)
        }
        request @ (QueryRequest::Index(_) | QueryRequest::Hour(_)) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(
                std::sync::Arc::<crate::query_adapter::NativeDataset>::clone(&dataset),
                sources,
                synthetic_demo,
            )
            .with_index_provider(dataset);
            kronika_query::execute(&context, request)
                .map(prepared_query)
                .map_err(ApiError::from)
        }
        request @ QueryRequest::Heatmap(_) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(&context, request)
                .map(prepared_query)
                .map_err(ApiError::from)
        }
        QueryRequest::Snapshot(request) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            let preparation = kronika_query::snapshot::prepare_snapshot(&context, request)
                .map_err(ApiError::from)?;
            let meta = query_meta(preparation.metadata());
            let concrete_validator = if_none_match.filter(|offered| offered.trim() != "*");
            if let Some(not_modified) = conditional_not_modified(meta, concrete_validator) {
                return Ok(not_modified);
            }
            preparation
                .finish()
                .map(prepared_query)
                .map_err(ApiError::from)
        }
        _ => return Err(ApiError::NoSuchSection),
    }?;
    let meta = prepared.meta();
    if let Some(not_modified) = conditional_not_modified(meta, if_none_match) {
        return Ok(not_modified);
    }
    Ok(prepared)
}

fn conditional_not_modified(meta: ResponseMeta, if_none_match: Option<&str>) -> Option<Prepared> {
    meta.etag
        .as_deref()
        .zip(if_none_match)
        .is_some_and(|(current, offered)| etag_matches(offered, current))
        .then(|| {
            Prepared::Empty(ResponseMeta {
                status: StatusCode::NOT_MODIFIED,
                ..meta
            })
        })
}

fn weak_dataset_etag(
    resource: &str,
    shape: &str,
    segments: &[kronika_query::DatasetSegment],
) -> Option<String> {
    let mut digest = Sha256::new();
    digest.update(resource.len().to_le_bytes());
    digest.update(resource.as_bytes());
    digest.update(shape.len().to_le_bytes());
    digest.update(shape.as_bytes());
    let mut found = false;
    for segment in segments {
        if segment.kind() == kronika_reader::SegmentKind::Active {
            return None;
        }
        found = true;
        digest.update(segment.id().to_le_bytes());
        digest.update(segment.min_ts().to_le_bytes());
        digest.update(segment.max_ts().to_le_bytes());
        digest.update(segment.sections().len().to_le_bytes());
        for section in segment.sections() {
            digest.update(section.type_id.to_le_bytes());
            digest.update(section.rows.to_le_bytes());
            digest.update(section.bytes.to_le_bytes());
        }
    }
    found.then(|| format!("W/\"{:x}\"", digest.finalize()))
}
