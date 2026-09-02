//! Typed inputs accepted by query execution.

/// Optional inclusive timestamp bounds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
    /// Earliest included timestamp, Unix microseconds.
    pub from: Option<i64>,
    /// Latest included timestamp, Unix microseconds.
    pub to: Option<i64>,
}

impl Window {
    /// Whether one timestamp falls inside these bounds.
    #[must_use]
    pub fn contains(self, timestamp: i64) -> bool {
        self.from.is_none_or(|from| timestamp >= from) && self.to.is_none_or(|to| timestamp <= to)
    }
}

/// Request for the recorded segment catalog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CatalogRequest {
    /// Inclusive recorded-time filter.
    pub window: Window,
}

/// One logical section in one exact recorded segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRequest {
    /// Stable segment identity.
    pub segment_id: i64,
    /// Registry logical-section name.
    pub section: String,
}

/// One logical indexed series in one exact recorded segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRequest {
    /// Stable segment identity.
    pub segment_id: i64,
    /// Registry logical-section name.
    pub section: String,
}

/// One exact typed equality predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Registry column name.
    pub column: String,
    /// Native route adapter's decoded textual value.
    pub value: String,
}

/// Committed prefix of one active segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveCursor {
    /// Stable active segment identity.
    pub segment_id: i64,
    /// Committed journal position.
    pub wal_position: u64,
}

/// Projection and predicates shared by history and row pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRequest {
    /// Exact logical section selection.
    pub segment: SegmentRequest,
    /// Output fields in caller order; empty selects the stable union.
    pub fields: Vec<String>,
    /// Typed equality predicates.
    pub filters: Vec<Filter>,
    /// Optional exact physical layout.
    pub type_id: Option<u32>,
    /// Optional earlier committed prefix to exclude from active results.
    pub after: Option<ActiveCursor>,
}

/// Physical row ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Increasing physical ordinal.
    Asc,
    /// Decreasing physical ordinal.
    Desc,
}

/// One bounded physical-row page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowsRequest {
    /// Projection, filters, section, and active-tail selection.
    pub data: DataRequest,
    /// Requested physical order.
    pub order: Order,
    /// Maximum matching rows emitted in this page.
    pub page_size: usize,
    /// Opaque continuation produced by an earlier page.
    pub cursor: Option<String>,
}

/// Semantic query selected by the native route adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueryRequest {
    /// Actual finished/current segment inventory.
    Catalog(CatalogRequest),
    /// One allowlisted derived series from one exact segment.
    Index(IndexRequest),
    /// Full-resolution rows from one exact segment.
    History(DataRequest),
    /// One stable bounded page from one exact segment.
    Rows(RowsRequest),
    /// Recorded event groups or physical occurrences over one window.
    Events(crate::EventsQuery),
}
