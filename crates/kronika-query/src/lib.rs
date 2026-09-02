//! Storage-neutral execution of recorded-data queries.

mod catalog;
mod dataset;
mod error;
mod events;
mod finished_dataset;
mod history;
mod index;
mod index_provider;
mod projection;
mod render;
mod request;
mod row_key;
mod rows;
mod selection;
mod time;

pub use dataset::{
    CapturedCatalog, DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject,
    OpaqueCapture, PredecessorSelection, QueryDataset, SegmentBounds, SegmentSelection,
};
pub use error::QueryError;
pub use events::{
    EventGroup, EventOccurrence, EventsQuery, EventsQueryError, EventsRepresentation, EventsResult,
    MAX_EVENTS_LIMIT, MAX_EVENTS_WINDOW_MICROS,
};
pub use finished_dataset::FinishedDataset;
pub use index_provider::{IndexProvider, IndexResource};
pub use request::{
    ActiveCursor, CatalogRequest, DataRequest, Filter, IndexRequest, Order, QueryRequest,
    RowsRequest, SegmentRequest, Window,
};
pub use time::TimeRange;

use catalog::PreparedCatalog;
use events::PreparedEvents;
use history::PreparedHistory;
use rows::PreparedRows;

/// Source-family bit for recorded operating-system data.
pub const SOURCE_OS: u32 = 1 << 0;
/// Source-family bit for recorded `PostgreSQL` data.
pub const SOURCE_POSTGRESQL: u32 = 1 << 1;

/// Inputs shared by one synchronous query execution.
#[derive(Clone)]
pub struct QueryContext {
    dataset: std::sync::Arc<dyn QueryDataset>,
    indexes: Option<std::sync::Arc<dyn IndexProvider>>,
    configured_sources: u32,
    synthetic_demo: bool,
}

impl std::fmt::Debug for QueryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryContext")
            .field("dataset", &self.dataset)
            .field("indexes", &self.indexes)
            .field("configured_sources", &self.configured_sources)
            .field("synthetic_demo", &self.synthetic_demo)
            .finish()
    }
}

impl QueryContext {
    /// Bind a captured-data adapter and deployment facts to query execution.
    #[must_use]
    pub const fn new(
        dataset: std::sync::Arc<dyn QueryDataset>,
        configured_sources: u32,
        synthetic_demo: bool,
    ) -> Self {
        Self {
            dataset,
            indexes: None,
            configured_sources,
            synthetic_demo,
        }
    }

    /// Add the native derived-index/cache adapter used by indexed queries.
    #[must_use]
    pub fn with_index_provider(mut self, indexes: std::sync::Arc<dyn IndexProvider>) -> Self {
        self.indexes = Some(indexes);
        self
    }

    fn index_provider(&self) -> Result<&dyn IndexProvider, QueryError> {
        self.indexes.as_deref().ok_or_else(|| {
            QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "this query context has no derived-index provider",
            )))
        })
    }
}

/// Storage stability relevant to the native response adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStability {
    /// The captured data may change between requests.
    Mutable,
    /// The data is settled, but no reusable identity was derived.
    Revalidate,
    /// The response is bound to immutable captured data.
    Immutable,
}

/// Stable response identity whose wire representation belongs to the adapter.
#[derive(Debug, Clone, Copy)]
pub enum QueryIdentity<'a> {
    /// CRC32C validated from one immutable IDX container.
    IndexChecksum(u32),
    /// Exact immutable segment selection whose digest remains adapter-owned.
    SegmentSet {
        /// Stable response-family domain.
        resource: &'a str,
        /// Exact semantic request shape.
        shape: &'a str,
        /// Captured segment descriptors in selection order.
        segments: &'a [DatasetSegment],
    },
}

/// Transport-neutral facts known before records are emitted.
#[derive(Debug, Clone, Copy)]
pub struct QueryMetadata<'a> {
    stability: QueryStability,
    identity: Option<QueryIdentity<'a>>,
}

impl<'a> QueryMetadata<'a> {
    /// Stability of the selected recorded data.
    #[must_use]
    pub const fn stability(self) -> QueryStability {
        self.stability
    }

    /// Stable selected-data identity, when one is available.
    #[must_use]
    pub const fn identity(self) -> Option<QueryIdentity<'a>> {
        self.identity
    }

    const fn revalidate() -> Self {
        Self {
            stability: QueryStability::Revalidate,
            identity: None,
        }
    }
}

/// Receiver for already framed NDJSON records.
pub trait QuerySink {
    /// Accept one complete record including its trailing newline.
    fn record(&mut self, bytes: Vec<u8>) -> bool;

    /// Whether the caller no longer wants more work or output.
    fn cancelled(&self) -> bool;
}

/// One prepared execution with metadata available before streaming.
pub struct QueryExecution {
    prepared: Prepared,
}

enum Prepared {
    Catalog(PreparedCatalog),
    Index(index::PreparedIndex),
    History(PreparedHistory),
    Rows(PreparedRows),
    Events(PreparedEvents),
}

impl std::fmt::Debug for QueryExecution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryExecution").finish_non_exhaustive()
    }
}

impl QueryExecution {
    /// Facts the native adapter maps to cache and validator policy.
    #[must_use]
    pub fn metadata(&self) -> QueryMetadata<'_> {
        match &self.prepared {
            Prepared::Catalog(_) => QueryMetadata::revalidate(),
            Prepared::Index(prepared) => QueryMetadata {
                stability: match prepared.kind() {
                    kronika_reader::SegmentKind::Finished => QueryStability::Immutable,
                    kronika_reader::SegmentKind::Active => QueryStability::Mutable,
                },
                identity: prepared.checksum().map(QueryIdentity::IndexChecksum),
            },
            Prepared::History(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: None,
            },
            Prepared::Rows(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: None,
            },
            Prepared::Events(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: prepared
                    .validator_input()
                    .map(|(resource, shape, segments)| QueryIdentity::SegmentSet {
                        resource,
                        shape,
                        segments,
                    }),
            },
        }
    }

    /// Emit records until complete or the sink disconnects.
    ///
    /// # Errors
    ///
    /// Returns a semantic, decoding, or captured-source error.
    pub fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        match self.prepared {
            Prepared::Catalog(prepared) => prepared.stream(sink),
            Prepared::Index(prepared) => prepared.stream(sink),
            Prepared::History(prepared) => prepared.stream(sink),
            Prepared::Rows(prepared) => prepared.stream(sink),
            Prepared::Events(prepared) => prepared.stream(sink),
        }
    }
}

/// Validate and prepare one recorded-data query.
///
/// # Errors
///
/// Returns a semantic, decoding, or captured-source error.
pub fn execute(
    context: &QueryContext,
    request: &QueryRequest,
) -> Result<QueryExecution, QueryError> {
    let prepared = match request {
        QueryRequest::Catalog(request) => Prepared::Catalog(PreparedCatalog::prepare(
            context.dataset.as_ref(),
            *request,
            context.configured_sources,
            context.synthetic_demo,
        )?),
        QueryRequest::Index(request) => Prepared::Index(index::prepare(
            context.dataset.as_ref(),
            context.index_provider()?,
            request,
        )?),
        QueryRequest::History(request) => {
            Prepared::History(history::prepare(context.dataset.as_ref(), request.clone())?)
        }
        QueryRequest::Rows(request) => {
            Prepared::Rows(rows::prepare(context.dataset.as_ref(), request.clone())?)
        }
        QueryRequest::Events(request) => Prepared::Events(events::prepare(
            std::sync::Arc::clone(&context.dataset),
            request.clone(),
        )?),
    };
    Ok(QueryExecution { prepared })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
