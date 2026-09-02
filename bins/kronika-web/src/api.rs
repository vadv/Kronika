//! Preparing blocking resource reads and streaming small self-describing records.

use std::error::Error;
use std::path::Path;

use hyper::StatusCode;
use kronika_query::{
    CatalogRequest, QueryContext, QueryIdentity, QueryRequest, QuerySink, QueryStability,
};
use kronika_reader::{Reader, ReaderError, SegmentRef};
use sha2::{Digest as _, Sha256};

use crate::encoding::etag_matches;
use crate::route::Route;

pub(crate) mod catalog;
pub(crate) mod history;
mod hour;
pub(crate) mod index;
mod query;
mod render;
pub(crate) mod row_detail;
pub(crate) mod row_key;
pub(crate) mod snapshot;
pub(crate) mod time;

#[cfg(test)]
pub(crate) use hour::process_summary::{
    operations as process_summary_operations, reset_operations as reset_process_summary_operations,
};
#[cfg(test)]
pub(crate) use hour::{operations as hour_operations, reset_operations as reset_hour_operations};
#[cfg(test)]
pub(crate) use snapshot::{
    context_operations, first_match_rows, history_operations, page_operations,
    relation_snapshot_operations, reset_context_operations, reset_first_match_rows,
    reset_history_operations, reset_page_operations, reset_relation_snapshot_operations,
    tablespace_moment_visits,
};

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
    const fn ok(cache: CachePolicy) -> Self {
        Self::ok_with_etag(cache, None)
    }

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
    Query(PreparedQuery),
    Hour(hour::PreparedHour),
    Snapshot(snapshot::PreparedSnapshot),
    RowDetail(row_detail::PreparedRowDetail),
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
            Self::Hour(prepared) => prepared.meta(),
            Self::Snapshot(prepared) => prepared.meta(),
            Self::RowDetail(_prepared) => row_detail::PreparedRowDetail::meta(),
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
            Self::Hour(prepared) => prepared.stream(emit, cancelled),
            Self::Snapshot(prepared) => prepared.stream(emit, cancelled),
            Self::RowDetail(prepared) => prepared.stream(emit, cancelled),
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

#[derive(Debug)]
struct HeatmapOpenError(String);

impl std::fmt::Display for HeatmapOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rankings[0]: {}", self.0)
    }
}

impl Error for HeatmapOpenError {}

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
    Prepared::Query(PreparedQuery { execution, meta })
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
#[expect(
    clippy::too_many_lines,
    reason = "route preparation keeps every blocking response family in one match"
)]
pub(crate) fn prepare_with_demo(
    root: &Path,
    sources: u32,
    synthetic_demo: bool,
    route: Route,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let prepared = match route {
        Route::Catalog(window) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(
                &context,
                QueryRequest::Catalog(CatalogRequest {
                    window: kronika_query::Window {
                        from: window.from,
                        to: window.to,
                    },
                }),
            )
            .map(prepared_query)
            .map_err(ApiError::from)
        }
        Route::Index(request) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(
                std::sync::Arc::<crate::query_adapter::NativeDataset>::clone(&dataset),
                sources,
                synthetic_demo,
            )
            .with_index_provider(dataset);
            kronika_query::execute(
                &context,
                QueryRequest::Index(kronika_query::IndexRequest {
                    segment_id: request.segment_id,
                    section: request.section,
                }),
            )
            .map(prepared_query)
            .map_err(ApiError::from)
        }
        Route::History(request) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(
                &context,
                QueryRequest::History(shared_data_request(request)),
            )
            .map(prepared_query)
            .map_err(ApiError::from)
        }
        Route::Heatmap(request) => {
            let to_exclusive = request
                .to
                .checked_add(1)
                .ok_or_else(|| ApiError::BadFilter("to".to_owned()))?;
            let range = kronika_query::TimeRange::new(request.from, to_exclusive)
                .map_err(|_error| ApiError::BadFilter("to".to_owned()))?;
            let query = kronika_query::HeatmapBatchQuery {
                range,
                items: vec![kronika_query::HeatmapItemQuery {
                    ranking: kronika_query::NormalizedRanking {
                        section: request.section,
                        fields: request.fields,
                        top: request.top,
                    },
                    view: kronika_query::HeatmapView::Grid {
                        columns: request.columns,
                        group: request.group,
                        type_id: request.type_id,
                    },
                }],
            };
            let query = kronika_query::validate_heatmap_request(query).map_err(ApiError::from)?;
            let dataset = std::sync::Arc::new(
                crate::query_adapter::NativeDataset::from_root(root).map_err(|error| {
                    ApiError::Unreadable(Box::new(HeatmapOpenError(error.to_string())))
                })?,
            );
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(&context, QueryRequest::Heatmap(query))
                .map(prepared_query)
                .map_err(ApiError::from)
        }
        Route::Events(request) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(&context, QueryRequest::Events(request))
                .map(prepared_query)
                .map_err(ApiError::from)
        }
        Route::RowDetail(detail_ref) => {
            row_detail::prepare(root, &detail_ref).map(Prepared::RowDetail)
        }
        Route::Hour(request) => {
            hour::prepare(root, request, sources, synthetic_demo).map(Prepared::Hour)
        }
        Route::Rows(request) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(
                &context,
                QueryRequest::Rows(kronika_query::RowsRequest {
                    data: shared_data_request(request.data),
                    order: match request.order {
                        crate::route::Order::Asc => kronika_query::Order::Asc,
                        crate::route::Order::Desc => kronika_query::Order::Desc,
                    },
                    page_size: request.page_size,
                    cursor: request.cursor,
                }),
            )
            .map(prepared_query)
            .map_err(ApiError::from)
        }
        Route::Snapshot(request) => snapshot::prepare(root, *request, if_none_match),
        // Answered directly in `main.rs`.
        Route::McpAccess | Route::InstanceLabel => return Err(ApiError::NoSuchSection),
    }?;
    let meta = prepared.meta();
    if let Some(not_modified) = conditional_not_modified(meta, if_none_match) {
        return Ok(not_modified);
    }
    Ok(prepared)
}

fn shared_data_request(request: crate::route::DataRequest) -> kronika_query::DataRequest {
    kronika_query::DataRequest {
        segment: kronika_query::SegmentRequest {
            segment_id: request.segment.segment_id,
            section: request.segment.section,
        },
        fields: request.fields,
        filters: request
            .filters
            .into_iter()
            .map(|filter| kronika_query::Filter {
                column: filter.column,
                value: filter.value,
            })
            .collect(),
        type_id: request.type_id,
        after: request.after.map(|after| kronika_query::ActiveCursor {
            segment_id: after.segment_id,
            wal_position: after.wal_position,
        }),
    }
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

fn weak_etag<I, S>(resource: &str, shape: &str, segments: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: std::borrow::Borrow<SegmentRef>,
{
    let mut digest = Sha256::new();
    digest.update(resource.len().to_le_bytes());
    digest.update(resource.as_bytes());
    digest.update(shape.len().to_le_bytes());
    digest.update(shape.as_bytes());
    let mut found = false;
    for segment in segments {
        let segment = segment.borrow();
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

fn explicit_segment_with_listing(
    root: &Path,
    id: i64,
) -> Result<(Reader, SegmentRef, Vec<SegmentRef>, bool), ApiError> {
    let started = std::time::Instant::now();
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments(..)?;
    let clean = listing.warnings.is_empty();
    log_warnings(&listing.warnings);
    let mut segments = listing.segments;
    let index = segments
        .iter()
        .position(|segment| segment.id() == id)
        .ok_or(ApiError::NoSuchSegment)?;
    let segment = segments.remove(index);
    log_segment_open(&segment, started.elapsed());
    Ok((reader, segment, segments, clean))
}

fn log_segment_open(segment: &SegmentRef, elapsed: std::time::Duration) {
    eprintln!(
        "kronika-web: segment_open id={} kind={} sections={} elapsed_us={}",
        segment.id(),
        match segment.kind() {
            kronika_reader::SegmentKind::Finished => "finished",
            kronika_reader::SegmentKind::Active => "active",
        },
        segment.sections().len(),
        elapsed.as_micros(),
    );
}

fn log_warnings(warnings: &[kronika_reader::StoreWarning]) {
    for warning in warnings {
        eprintln!("kronika-web: store warning code={}", warning.reason.code());
    }
}
