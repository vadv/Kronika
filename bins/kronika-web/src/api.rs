//! Preparing blocking resource reads and streaming small self-describing records.

use std::error::Error;
use std::path::Path;

use hyper::StatusCode;
use kronika_reader::{Reader, ReaderError, SegmentRef};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::encoding::etag_matches;
use crate::route::{ActiveCursor, Route};

mod catalog;
mod events;
mod heatmap;
mod history;
mod hour;
mod index;
mod process_tree;
mod query;
mod render;
mod row_detail;
mod rows;
mod snapshot;
mod surface;

pub(crate) use catalog::{
    metric_source_bit as catalog_metric_source_bit, source_bit as catalog_source_bit,
    warning_value as catalog_warning_value,
};
pub(crate) use events::{
    EventPageError, EventPageRequest, EventSourceRequest, EventStopReason, read_event_page,
};
pub(crate) use row_detail::{RowDetailRequest, read_row_detail};
pub(crate) use snapshot::prepare_for_mcp as prepare_snapshot_for_mcp;
pub(crate) use surface::{LOCK_GRAPH_FIELDS, postgresql_order_tokens};

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use hour::process_summary::{
    operations as process_summary_operations, reset_operations as reset_process_summary_operations,
};
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
    Catalog(catalog::PreparedCatalog),
    Index(index::PreparedIndex),
    History(history::PreparedHistory),
    Heatmap(heatmap::PreparedHeatmap),
    Hour(hour::PreparedHour),
    Rows(rows::PreparedRows),
    Snapshot(snapshot::PreparedSnapshot),
    ProcessTree(process_tree::PreparedProcessTree),
    Empty(ResponseMeta),
}

impl Prepared {
    /// Response status and caching, available before the first body record.
    pub(crate) fn meta(&self) -> ResponseMeta {
        match self {
            Self::Catalog(_prepared) => catalog::PreparedCatalog::meta(),
            Self::Index(prepared) => prepared.meta(),
            Self::History(prepared) => prepared.meta(),
            Self::Heatmap(prepared) => prepared.meta(),
            Self::Hour(prepared) => prepared.meta(),
            Self::Rows(prepared) => prepared.meta(),
            Self::Snapshot(prepared) => prepared.meta(),
            Self::ProcessTree(prepared) => prepared.meta(),
            Self::Empty(meta) => meta.clone(),
        }
    }

    /// Emit newline-delimited JSON records until complete or the client leaves.
    pub(crate) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let mut failure = None;
        self.stream_values(
            &mut |value| match render::record(&value) {
                Ok(bytes) => emit(bytes),
                Err(error) => {
                    failure = Some(error);
                    false
                }
            },
            cancelled,
        )?;
        failure.map_or(Ok(()), Err)
    }

    /// Emit the exact typed records used by the HTTP representation.
    pub(crate) fn stream_values(
        self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        match self {
            Self::Catalog(prepared) => prepared.stream(emit, cancelled),
            Self::Index(prepared) => prepared.stream(emit, cancelled),
            Self::History(prepared) => prepared.stream(emit, cancelled),
            Self::Heatmap(prepared) => prepared.stream(emit, cancelled),
            Self::Hour(prepared) => prepared.stream(emit, cancelled),
            Self::Rows(prepared) => prepared.stream(emit, cancelled),
            Self::Snapshot(prepared) => prepared.stream(emit, cancelled),
            Self::ProcessTree(prepared) => prepared.stream(emit, cancelled),
            Self::Empty(_meta) => Ok(()),
        }
    }

    /// Retain typed records without crossing either exact NDJSON budget.
    pub(crate) fn collect_values(
        self,
        limits: ValueLimits,
        cancelled: &impl Fn() -> bool,
    ) -> Result<ValueCollection, ApiError> {
        let saw_cancel = std::cell::Cell::new(false);
        let tracked_cancel = || {
            let stopped = cancelled();
            saw_cancel.set(saw_cancel.get() || stopped);
            stopped
        };
        let mut collector = ValueCollector::new(limits);
        self.stream_values(&mut |value| collector.push(value), &tracked_cancel)?;
        collector.finish(saw_cancel.get())
    }
}

/// Exact caps for a retained typed record stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValueLimits {
    pub(crate) records: usize,
    pub(crate) ndjson_bytes: usize,
}

/// Why a bounded typed stream stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueStopReason {
    Complete,
    RecordLimit,
    ByteLimit,
    Cancelled,
}

impl ValueStopReason {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::RecordLimit => "record_limit",
            Self::ByteLimit => "byte_limit",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Bounded typed output shared by the direct API and MCP callers.
#[derive(Debug, PartialEq)]
pub(crate) struct ValueCollection {
    pub(crate) records: Vec<Value>,
    pub(crate) ndjson_bytes: usize,
    pub(crate) stop_reason: ValueStopReason,
}

struct ValueCollector {
    limits: ValueLimits,
    records: Vec<Value>,
    ndjson_bytes: usize,
    stop_reason: Option<ValueStopReason>,
    failure: Option<ApiError>,
}

impl ValueCollector {
    const fn new(limits: ValueLimits) -> Self {
        Self {
            limits,
            records: Vec::new(),
            ndjson_bytes: 0,
            stop_reason: None,
            failure: None,
        }
    }

    fn push(&mut self, value: Value) -> bool {
        if self.records.len() >= self.limits.records {
            self.stop_reason = Some(ValueStopReason::RecordLimit);
            return false;
        }
        let bytes = match render::record(&value) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.failure = Some(error);
                return false;
            }
        };
        let Some(total) = self.ndjson_bytes.checked_add(bytes.len()) else {
            self.stop_reason = Some(ValueStopReason::ByteLimit);
            return false;
        };
        if total > self.limits.ndjson_bytes {
            self.stop_reason = Some(ValueStopReason::ByteLimit);
            return false;
        }
        self.records.push(value);
        self.ndjson_bytes = total;
        true
    }

    fn finish(self, cancelled: bool) -> Result<ValueCollection, ApiError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        Ok(ValueCollection {
            records: self.records,
            ndjson_bytes: self.ndjson_bytes,
            stop_reason: self.stop_reason.unwrap_or(if cancelled {
                ValueStopReason::Cancelled
            } else {
                ValueStopReason::Complete
            }),
        })
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
    Product(Box<ProductError>),
    Unreadable(Box<dyn Error + Send + Sync>),
}

#[derive(Debug)]
pub(crate) struct ProductError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) parameter: Option<&'static str>,
    pub(crate) retryable: bool,
    pub(crate) status: StatusCode,
}

impl ApiError {
    pub(crate) const fn status(&self) -> StatusCode {
        match self {
            Self::NoSuchSegment | Self::NoSuchSection => StatusCode::NOT_FOUND,
            Self::NoSuchColumn(_) | Self::BadFilter(_) | Self::BadCursor => StatusCode::BAD_REQUEST,
            Self::Product(error) => error.status,
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
            Self::Product(error) => error.code,
            Self::Unreadable(_) => "unreadable",
        }
    }

    pub(crate) fn parameter(&self) -> Option<&str> {
        match self {
            Self::NoSuchColumn(column) | Self::BadFilter(column) => Some(column),
            Self::Product(error) => error.parameter,
            _ => None,
        }
    }

    pub(crate) fn source_changed_during_read(&self) -> bool {
        if let Self::Product(error) = self {
            return error.retryable;
        }
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
            Self::BadFilter(column) => write!(f, "invalid typed filter for {column:?}"),
            Self::BadCursor => write!(f, "invalid page cursor"),
            Self::Product(error) => f.write_str(&error.message),
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
    prepare_with_index_access(
        root,
        sources,
        synthetic_demo,
        route,
        if_none_match,
        hour::IndexAccess::Publishing,
    )
}

/// Prepare the same typed records without publishing selected derived indexes.
pub(crate) fn prepare_for_mcp(
    root: &Path,
    sources: u32,
    synthetic_demo: bool,
    route: Route,
) -> Result<Prepared, ApiError> {
    prepare_with_index_access(
        root,
        sources,
        synthetic_demo,
        route,
        None,
        hour::IndexAccess::ReadOnly,
    )
}

fn prepare_with_index_access(
    root: &Path,
    sources: u32,
    synthetic_demo: bool,
    route: Route,
    if_none_match: Option<&str>,
    index_access: hour::IndexAccess,
) -> Result<Prepared, ApiError> {
    let prepared = match route {
        Route::Catalog(window) => {
            catalog::prepare(root, window, sources, synthetic_demo).map(Prepared::Catalog)
        }
        Route::Index(request) => index::prepare(root, request).map(Prepared::Index),
        Route::History(request) => history::prepare(root, request).map(Prepared::History),
        Route::Heatmap(request) => heatmap::prepare(root, request).map(Prepared::Heatmap),
        Route::Hour(request) => {
            hour::prepare(root, request, sources, synthetic_demo, index_access).map(Prepared::Hour)
        }
        Route::Rows(request) => rows::prepare(root, request).map(Prepared::Rows),
        Route::Snapshot(mut request) => {
            let process_lens = request
                .process
                .is_some()
                .then(|| surface::resolve_process_surface(&mut request))
                .transpose()?;
            if process_lens == Some(crate::route::ProcessLens::Tree) {
                process_tree::prepare(root, *request, if_none_match)
            } else {
                snapshot::prepare(root, *request, if_none_match)
            }
        }
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

fn explicit_segment(root: &Path, id: i64) -> Result<(Reader, SegmentRef), ApiError> {
    let started = std::time::Instant::now();
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segment(id)?;
    log_warnings(&listing.warnings);
    let segment = listing
        .segments
        .into_iter()
        .next()
        .ok_or(ApiError::NoSuchSegment)?;
    log_segment_open(&segment, started.elapsed());
    Ok((reader, segment))
}

fn explicit_segment_with_listing(
    root: &Path,
    id: i64,
) -> Result<(Reader, SegmentRef, Vec<SegmentRef>), ApiError> {
    let started = std::time::Instant::now();
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments(..)?;
    log_warnings(&listing.warnings);
    let mut segments = listing.segments;
    let index = segments
        .iter()
        .position(|segment| segment.id() == id)
        .ok_or(ApiError::NoSuchSegment)?;
    let segment = segments.remove(index);
    log_segment_open(&segment, started.elapsed());
    Ok((reader, segment, segments))
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
