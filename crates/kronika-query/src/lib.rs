//! Storage-neutral execution of recorded-data queries.

mod catalog;
mod dataset;
mod error;
mod render;
mod request;

pub use dataset::{
    CapturedCatalog, DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject,
    OpaqueCapture, PredecessorSelection, QueryDataset, SegmentBounds, SegmentSelection,
};
pub use error::QueryError;
pub use request::{CatalogRequest, QueryRequest, Window};

use catalog::PreparedCatalog;

/// Source-family bit for recorded operating-system data.
pub const SOURCE_OS: u32 = 1 << 0;
/// Source-family bit for recorded `PostgreSQL` data.
pub const SOURCE_POSTGRESQL: u32 = 1 << 1;

/// Inputs shared by one synchronous query execution.
#[derive(Clone)]
pub struct QueryContext {
    dataset: std::sync::Arc<dyn QueryDataset>,
    configured_sources: u32,
    synthetic_demo: bool,
}

impl std::fmt::Debug for QueryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryContext")
            .field("dataset", &self.dataset)
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
            configured_sources,
            synthetic_demo,
        }
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

/// Transport-neutral facts known before records are emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryMetadata {
    stability: QueryStability,
}

impl QueryMetadata {
    /// Stability of the selected recorded data.
    #[must_use]
    pub const fn stability(self) -> QueryStability {
        self.stability
    }

    const fn revalidate() -> Self {
        Self {
            stability: QueryStability::Revalidate,
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
}

impl std::fmt::Debug for QueryExecution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryExecution").finish_non_exhaustive()
    }
}

impl QueryExecution {
    /// Facts the native adapter maps to cache and validator policy.
    #[must_use]
    pub const fn metadata(&self) -> QueryMetadata {
        match self.prepared {
            Prepared::Catalog(_) => QueryMetadata::revalidate(),
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
    };
    Ok(QueryExecution { prepared })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
