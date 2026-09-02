//! Injected access to derived index blocks.

use kronika_index::{SeriesKey, TargetedIndex};

use crate::{DatasetSegment, QueryError};

/// Selected derived blocks for one captured segment.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexResource {
    /// Validated selected blocks.
    pub index: TargetedIndex,
}

/// Small boundary around loading derived index blocks.
pub trait IndexProvider: std::fmt::Debug + Send + Sync {
    /// Load only the selected derived blocks for one exact captured segment.
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
