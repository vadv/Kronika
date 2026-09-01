//! Storage-neutral identities and immutable segment access.

use std::fmt::Debug;
use std::sync::Arc;

use kronika_format::ReadAt;
use kronika_layout::SegmentId;

use crate::{CatalogSummary, StoreError, StoreWarning};

/// The product-visible kind of one stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ResourceKind {
    /// One immutable, self-contained ZMS segment.
    FinishedSegment,
}

/// Stable product identity of one stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceIdentity {
    segment_id: SegmentId,
    kind: ResourceKind,
}

impl ResourceIdentity {
    /// Identity of one immutable finished segment.
    #[must_use]
    pub const fn finished(segment_id: SegmentId) -> Self {
        Self {
            segment_id,
            kind: ResourceKind::FinishedSegment,
        }
    }

    /// Explicit segment identity supplied by the catalog source.
    #[must_use]
    pub const fn segment_id(self) -> SegmentId {
        self.segment_id
    }

    /// Product-visible resource kind.
    #[must_use]
    pub const fn kind(self) -> ResourceKind {
        self.kind
    }
}

/// One resource returned by catalog discovery.
///
/// `R` is an adapter-owned open token. Product code uses the identity and
/// catalog summary and passes the complete value back to its source to open
/// bytes; it does not interpret the token.
#[derive(Debug, Clone)]
pub struct SegmentResource<R> {
    identity: ResourceIdentity,
    captured_bytes: u64,
    summary: Arc<CatalogSummary>,
    handle: R,
}

impl<R> SegmentResource<R> {
    pub(crate) const fn new(
        identity: ResourceIdentity,
        captured_bytes: u64,
        summary: Arc<CatalogSummary>,
        handle: R,
    ) -> Self {
        Self {
            identity,
            captured_bytes,
            summary,
            handle,
        }
    }

    /// Stable product identity and kind.
    #[must_use]
    pub const fn identity(&self) -> ResourceIdentity {
        self.identity
    }

    /// Complete immutable object length in bytes.
    #[must_use]
    pub const fn captured_bytes(&self) -> u64 {
        self.captured_bytes
    }

    /// Compact validated catalog metadata.
    #[must_use]
    pub fn summary(&self) -> &CatalogSummary {
        &self.summary
    }

    /// Adapter-owned token used only to open this listed object.
    #[must_use]
    pub const fn handle(&self) -> &R {
        &self.handle
    }
}

/// Immutable resources and non-fatal discovery diagnostics from one catalog pass.
#[derive(Debug)]
pub struct ResourceListing<R> {
    /// Immutable resources in source-defined stable order.
    pub resources: Vec<SegmentResource<R>>,
    /// Objects excluded by catalog discovery.
    pub warnings: Vec<StoreWarning>,
}

/// Discovers immutable segment identities and compact catalogs.
///
/// Discovery is deliberately separate from opening section bytes. A future
/// remote adapter can prepare an object range or local cache before returning
/// its synchronous [`ReadAt`] implementation.
pub trait ResourceCatalog {
    /// Adapter-owned token carried by a discovered resource.
    type Resource: Clone + Debug + Send + Sync + 'static;

    /// List immutable resources without retaining their complete bytes.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a bounded catalog pass cannot complete.
    fn resources(&self) -> Result<ResourceListing<Self::Resource>, StoreError>;
}

/// Opens positional bytes for one immutable resource returned by a catalog.
pub trait ImmutableSegmentSource: ResourceCatalog {
    /// Synchronous byte reader handed to the existing decoder.
    type Bytes: ReadAt + Send + Sync + 'static;

    /// Open exactly one listed immutable object.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the identity is foreign, stale, or unreadable.
    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, StoreError>;
}
