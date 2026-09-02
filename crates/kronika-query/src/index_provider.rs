//! Injected access to derived index blocks.

use kronika_index::{SeriesKey, TargetedIndex};

use crate::{DatasetSegment, QueryError};

/// Selected derived blocks and their native cache provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexResource {
    /// Validated selected blocks.
    pub index: TargetedIndex,
    /// Whether the native adapter loaded or published an immutable sidecar.
    pub persisted: bool,
}

/// Small boundary around native index loading and caching.
pub trait IndexProvider: std::fmt::Debug + Send + Sync {
    /// Load only the selected derived blocks for one exact captured segment.
    ///
    /// The native implementation retains sibling `.idx` placement, locking,
    /// rebuilding, and active non-persistence without exposing those details.
    ///
    /// # Errors
    ///
    /// Returns a captured-source, decoding, build, or cache error.
    fn load(
        &self,
        segment: &DatasetSegment,
        logical_name: &str,
        keys: &[SeriesKey],
    ) -> Result<IndexResource, QueryError>;
}
