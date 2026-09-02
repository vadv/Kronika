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

/// One hour response assembled from a captured segment set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourRequest {
    /// Inclusive recorded-time filter.
    pub window: Window,
    /// Optional full-resolution or derived series selection.
    pub series: Option<HourSeriesRequest>,
    /// Base/index and lane composition.
    pub part: HourPart,
    /// Exact segment identities pinned by a follow-up lane request.
    pub segments: Option<Vec<i64>>,
    /// Committed active prefix pinned by a follow-up lane request.
    pub active: Option<ActiveCursor>,
}

/// Portion of an hour response requested by the native route.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HourPart {
    /// Catalog, indexes, and reduced lanes.
    #[default]
    Combined,
    /// Catalog and indexes only.
    Base,
    /// Reduced lanes only, pinned to a prior base response.
    Lanes,
}

/// One full-resolution or derived series inside an hour.
#[derive(Clone, PartialEq, Eq)]
pub struct HourSeriesRequest {
    /// Registry logical-section name or stable derived-section name.
    pub section: String,
    /// Output fields in caller order.
    pub fields: Vec<String>,
    /// Typed equality predicates.
    pub filters: Vec<Filter>,
    /// Optional exact physical layout.
    pub type_id: Option<u32>,
    /// Optional relation aggregation level.
    pub group: Option<RelationGroup>,
}

impl std::fmt::Debug for HourSeriesRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeriesRequest")
            .field("section", &self.section)
            .field("fields", &self.fields)
            .field("filters", &self.filters)
            .field("type_id", &self.type_id)
            .field("group", &self.group)
            .finish()
    }
}

/// Aggregation level for recorded relation products.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationGroup {
    /// One aggregate per database.
    Database,
    /// One aggregate per schema.
    Schema,
    /// One aggregate per tablespace.
    Tablespace,
    /// One aggregate per relation object.
    Object,
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
    /// One ranked entity heatmap over a recorded window.
    Heatmap(crate::ValidatedHeatmapQuery),
    /// One allowlisted derived series from one exact segment.
    Index(IndexRequest),
    /// Full-resolution rows from one exact segment.
    History(DataRequest),
    /// One composed timeline hour.
    Hour(HourRequest),
    /// One stable bounded page from one exact segment.
    Rows(RowsRequest),
    /// Recorded event groups or physical occurrences over one window.
    Events(crate::EventsQuery),
    /// One exact stored row addressed by a validated opaque reference.
    RowDetail(crate::ValidatedRowDetailQuery),
}
