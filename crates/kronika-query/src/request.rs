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

/// Semantic query selected by the native route adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueryRequest {
    /// Actual finished/current segment inventory.
    Catalog(CatalogRequest),
}
